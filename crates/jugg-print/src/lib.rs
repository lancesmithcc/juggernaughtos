//! Print and email assets made directly from brand tokens.
//!
//! Layouts are generated as SVG (exact, token-driven), then:
//! * SVG → print PDF with svg2pdf (vector text and shapes, fonts embedded),
//!   one page per side, MediaBox = trim + bleed, TrimBox/BleedBox set so print
//!   shops cut in the right place;
//! * SVG → PNG preview with resvg for the asset library.

mod book;
mod card;
mod eps;
pub use eps::eps_from_svg;
pub mod logo;
mod signature;
mod social;

pub use card::{business_card, CardInput, CARD_BLEED_PT, CARD_TRIM_PT};
pub use book::brand_book;
pub use signature::{email_signature, SignatureInput};
pub use social::{social_post, SocialInput, SocialSize, SOCIAL_SIZES};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use svg2pdf::usvg;

#[derive(Debug, thiserror::Error)]
pub enum PrintError {
    #[error("layout: {0}")]
    Svg(String),
    #[error("pdf: {0}")]
    Pdf(String),
    #[error("render: {0}")]
    Render(String),
}

pub type Result<T> = std::result::Result<T, PrintError>;

/// Font sources: bundled Plus Jakarta Sans, the local Google Fonts mirror
/// (`JUGG_FONT_DIRS`, colon-separated), then system fonts.
fn db_cell() -> &'static RwLock<Arc<usvg::fontdb::Database>> {
    static DB: OnceLock<RwLock<Arc<usvg::fontdb::Database>>> = OnceLock::new();
    DB.get_or_init(|| RwLock::new(initial_db()))
}

fn fontdb() -> Arc<usvg::fontdb::Database> {
    db_cell().read().map(|d| d.clone()).unwrap_or_else(|_| initial_db())
}

/// Make freshly downloaded brand fonts available to all later renders.
pub fn load_fonts(dir: &std::path::Path) {
    if !dir.exists() {
        return;
    }
    if let Ok(mut db) = db_cell().write() {
        let mut next = (**db).clone();
        next.load_fonts_dir(dir);
        *db = Arc::new(next);
    }
}

fn initial_db() -> Arc<usvg::fontdb::Database> {
    {
        let mut db = usvg::fontdb::Database::new();
        for dir in std::env::var("JUGG_FONT_DIRS").unwrap_or_default().split(':').filter(|d| !d.is_empty()) {
            db.load_fonts_dir(dir);
        }
        for candidate in [concat!(env!("CARGO_MANIFEST_DIR"), "/../../public/fonts/static"), "public/fonts/static", "/opt/juggernaught/current/site/fonts/static"] {
            let p = PathBuf::from(candidate);
            if p.exists() {
                db.load_fonts_dir(p);
            }
        }
        db.load_system_fonts();
        db.set_sans_serif_family("Plus Jakarta Sans");
        Arc::new(db)
    }
}

fn options() -> usvg::Options<'static> {
    let mut o = usvg::Options::default();
    o.fontdb = fontdb();
    o.font_family = "Plus Jakarta Sans".into();
    o
}

pub(crate) fn tree(svg: &str) -> Result<usvg::Tree> {
    usvg::Tree::from_str(svg, &options()).map_err(|e| PrintError::Svg(e.to_string()))
}

/// Pages (same size, in points) → one print PDF. `bleed` is trimmed off by the printer.
pub fn pdf_from_svgs(pages: &[String], width_pt: f32, height_pt: f32, bleed_pt: f32, title: &str) -> Result<Vec<u8>> {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, TextStr};
    let mut next = 1i32;
    let mut alloc = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let catalog = alloc();
    let tree_id = alloc();
    let info = alloc();
    let mut pdf = Pdf::new();
    let mut page_ids = Vec::new();
    let mut parts = Vec::new();
    for svg in pages {
        let (chunk, svg_ref) = svg2pdf::to_chunk(&tree(svg)?, svg2pdf::ConversionOptions::default()).map_err(|e| PrintError::Pdf(e.to_string()))?;
        let mut map = HashMap::new();
        let chunk = chunk.renumber(|old| *map.entry(old).or_insert_with(&mut alloc));
        let svg_ref = *map.get(&svg_ref).ok_or_else(|| PrintError::Pdf("missing xobject".into()))?;
        let (page_id, content_id) = (alloc(), alloc());
        page_ids.push(page_id);
        parts.push((page_id, content_id, svg_ref, chunk));
    }
    pdf.catalog(catalog).pages(tree_id);
    pdf.pages(tree_id).kids(page_ids.iter().copied()).count(page_ids.len() as i32);
    pdf.document_info(info).title(TextStr(title)).producer(TextStr("Juggernaught OS"));
    let media = Rect::new(0.0, 0.0, width_pt, height_pt);
    let trim = Rect::new(bleed_pt, bleed_pt, width_pt - bleed_pt, height_pt - bleed_pt);
    for (page_id, content_id, svg_ref, chunk) in parts {
        let mut page = pdf.page(page_id);
        page.media_box(media).bleed_box(media).trim_box(trim).parent(tree_id).contents(content_id);
        page.resources().x_objects().pair(Name(b"S1"), svg_ref);
        page.finish();
        let mut content = Content::new();
        content.save_state().transform([width_pt, 0.0, 0.0, height_pt, 0.0, 0.0]).x_object(Name(b"S1")).restore_state();
        pdf.stream(content_id, &content.finish());
        pdf.extend(&chunk);
    }
    Ok(pdf.finish())
}

/// SVG → PNG at `scale` (preview thumbnails).
pub fn png_from_svg(svg: &str, scale: f32) -> Result<Vec<u8>> {
    let t = tree(svg)?;
    let size = t.size().to_int_size().scale_by(scale).ok_or_else(|| PrintError::Render("empty size".into()))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).ok_or_else(|| PrintError::Render("pixmap".into()))?;
    resvg::render(&t, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| PrintError::Render(e.to_string()))
}

/// SVG → PNG whose longest side is `max_px` (transparent where the SVG is).
pub fn png_fit(svg: &str, max_px: u32) -> Result<Vec<u8>> {
    let t = tree(svg)?;
    let long = t.size().width().max(t.size().height());
    if long <= 0.0 {
        return Err(PrintError::Render("empty size".into()));
    }
    png_from_svg(svg, max_px as f32 / long)
}

/// Crop an SVG to its artwork plus `margin` (share of the longer side). Only for
/// SVGs whose root is `width=W height=H viewBox="0 0 W H"` (what Vectorverter
/// writes); anything else comes back unchanged.
pub fn trim_to_artwork(svg: &str, margin: f32) -> Result<String> {
    let Some(start) = svg.find("<svg") else { return Ok(svg.to_string()) };
    let Some(end) = svg[start..].find('>').map(|e| start + e) else { return Ok(svg.to_string()) };
    let root = &svg[start..end];
    let attr = |name: &str| -> Option<f32> {
        let key = format!(" {name}=\"");
        let i = root.find(&key)? + key.len();
        root[i..].split('"').next()?.trim().parse().ok()
    };
    let (Some(w), Some(h)) = (attr("width"), attr("height")) else { return Ok(svg.to_string()) };
    if !root.contains(&format!("viewBox=\"0 0 {} {}\"", fmt_num(w), fmt_num(h))) {
        return Ok(svg.to_string());
    }
    let bb = tree(svg)?.root().abs_stroke_bounding_box();
    if !(bb.width() > 0.0 && bb.height() > 0.0) {
        return Ok(svg.to_string());
    }
    let pad = bb.width().max(bb.height()) * margin;
    let (x, y, vw, vh) = (bb.x() - pad, bb.y() - pad, bb.width() + 2.0 * pad, bb.height() + 2.0 * pad);
    let new_root = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\"",
        fmt_num(vw.round()), fmt_num(vh.round()), fmt_num(x), fmt_num(y), fmt_num(vw), fmt_num(vh)
    );
    Ok(format!("{}{}{}", &svg[..start], new_root, &svg[end..]))
}

fn fmt_num(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A logo as a one-page vector PDF, longest side 8 inches.
pub fn logo_pdf(svg: &str, title: &str) -> Result<Vec<u8>> {
    let t = tree(svg)?;
    let (w, h) = (t.size().width(), t.size().height());
    if !(w > 0.0 && h > 0.0) {
        return Err(PrintError::Render("empty size".into()));
    }
    let k = 576.0 / w.max(h);
    pdf_from_svgs(&[svg.to_string()], w * k, h * k, 0.0, title)
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

#[cfg(test)]
mod logo_file_tests {
    use super::*;

    #[test]
    fn trims_to_the_artwork() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" version="1.1" width="200" height="100" viewBox="0 0 200 100"><rect x="50" y="20" width="100" height="60" fill="#123456"/></svg>"##;
        let out = trim_to_artwork(svg, 0.0).unwrap();
        assert!(out.contains(r#"viewBox="50 20 100 60""#), "{out}");
        assert!(out.contains(r#"width="100" height="60""#));
        let other = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="5" height="5"/></svg>"#;
        assert_eq!(trim_to_artwork(other, 0.1).unwrap(), other, "unknown layouts are left alone");
    }

    #[test]
    fn logo_pdf_and_pngs() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100"><rect width="200" height="100" fill="#123456"/></svg>"##;
        assert!(logo_pdf(svg, "Logo").unwrap().starts_with(b"%PDF"));
        let png = png_fit(svg, 1000).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }
}

