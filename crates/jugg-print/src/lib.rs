//! Print and email assets made directly from brand tokens.
//!
//! Layouts are generated as SVG (exact, token-driven), then:
//! * SVG → print PDF with svg2pdf (vector text and shapes, fonts embedded),
//!   one page per side, MediaBox = trim + bleed, TrimBox/BleedBox set so print
//!   shops cut in the right place;
//! * SVG → PNG preview with resvg for the asset library.

mod card;
mod signature;

pub use card::{business_card, CardInput, CARD_BLEED_PT, CARD_TRIM_PT};
pub use signature::{email_signature, SignatureInput};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
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
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
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
    })
    .clone()
}

fn options() -> usvg::Options<'static> {
    let mut o = usvg::Options::default();
    o.fontdb = fontdb();
    o.font_family = "Plus Jakarta Sans".into();
    o
}

fn tree(svg: &str) -> Result<usvg::Tree> {
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

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
