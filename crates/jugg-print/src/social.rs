//! Social post templates at every platform size, from the brand.

use crate::book::embed;
use crate::{esc, png_from_svg, Result};
use jugg_brand::BrandStandards;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialSize {
    Square,
    Portrait,
    Story,
    Link,
}

pub const SOCIAL_SIZES: [(SocialSize, &str, u32, u32); 4] = [
    (SocialSize::Square, "Square post (Instagram, Facebook, LinkedIn)", 1080, 1080),
    (SocialSize::Portrait, "Portrait post (Instagram feed)", 1080, 1350),
    (SocialSize::Story, "Story / Reel cover", 1080, 1920),
    (SocialSize::Link, "Link preview (website, X, LinkedIn)", 1200, 630),
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SocialInput {
    pub headline: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub cta: Option<String>,
    /// `dark` (ink background) or `accent` (primary background).
    #[serde(default)]
    pub tone: Option<String>,
    /// Optional photo (JPEG/PNG bytes) behind the text, darkened for contrast.
    #[serde(skip)]
    pub photo: Option<(Vec<u8>, String)>,
    /// Outlined logo icon SVG for the corner.
    #[serde(skip)]
    pub icon_svg: Option<String>,
}

pub fn social_post(b: &BrandStandards, input: &SocialInput, size: SocialSize) -> Result<(String, Vec<u8>)> {
    let (_, _, w, h) = SOCIAL_SIZES.iter().find(|s| s.0 == size).copied().unwrap_or(SOCIAL_SIZES[0]);
    let (w, h) = (w as f32, h as f32);
    let c = |s: &str, f: &str| b.color(s).unwrap_or(f).to_string();
    let accent = input.tone.as_deref() == Some("accent");
    let (bg, fg, cta_bg, cta_fg) = if accent { (c("primary", "#c2502a"), c("primary-ink", "#ffffff"), c("ink", "#1b1a17"), c("surface", "#faf8f4")) } else { (c("ink", "#1b1a17"), c("surface", "#faf8f4"), c("primary", "#c2502a"), c("primary-ink", "#ffffff")) };
    let hf = esc(&b.typography.heading.family);
    let bf = esc(&b.typography.body.family);
    let m = w * 0.08;
    let base = w.min(h);
    let hsize = match size {
        SocialSize::Link => h * 0.12,
        SocialSize::Story => base * 0.105,
        _ => base * 0.085,
    };
    let chars = ((w - 2.0 * m) / (hsize * 0.52)) as usize;
    let lines = crate::book::balanced(&input.headline, chars.max(8));
    let lines: Vec<String> = lines.into_iter().take(4).collect();
    let block_h = lines.len() as f32 * hsize * 1.08;
    let body_size = hsize * 0.38;
    let body_lines: Vec<String> = input.body.as_deref().map(|t| crate::book::balanced(t, ((w - 2.0 * m) / (body_size * 0.5)) as usize)).unwrap_or_default().into_iter().take(4).collect();
    let total = block_h + if body_lines.is_empty() { 0.0 } else { body_lines.len() as f32 * body_size * 1.5 + body_size } + if input.cta.is_some() { hsize * 1.2 } else { 0.0 };
    let mut y = ((h - total) / 2.0).max(m * 1.6) + hsize * 0.85;
    let mut out = String::new();
    if let Some((bytes, mime)) = &input.photo {
        use base64::Engine;
        out.push_str(&format!(r#"<image x="0" y="0" width="{w}" height="{h}" preserveAspectRatio="xMidYMid slice" href="data:{mime};base64,{}"/>"#, base64::engine::general_purpose::STANDARD.encode(bytes)));
        out.push_str(&format!(r#"<rect width="{w}" height="{h}" fill="{bg}" fill-opacity="0.62"/>"#));
    }
    out.push_str(&format!(r#"<text x="{m}" y="{}" font-family="{bf}" font-weight="600" font-size="{}" letter-spacing="{}" fill="{fg}" fill-opacity="0.75">{}</text>"#, m * 1.1, base * 0.026, base * 0.004, esc(&b.name.to_uppercase())));
    for l in &lines {
        out.push_str(&format!(r#"<text x="{m}" y="{y}" font-family="{hf}" font-weight="700" font-size="{hsize}" letter-spacing="{}" fill="{fg}">{}</text>"#, -hsize * 0.025, esc(l)));
        y += hsize * 1.08;
    }
    if !body_lines.is_empty() {
        y += body_size * 0.6;
        for l in &body_lines {
            out.push_str(&format!(r#"<text x="{m}" y="{y}" font-family="{bf}" font-size="{body_size}" fill="{fg}" fill-opacity="0.85">{}</text>"#, esc(l)));
            y += body_size * 1.5;
        }
    }
    if let Some(cta) = &input.cta {
        let ch = hsize * 0.72;
        let cw = cta.chars().count() as f32 * ch * 0.3 + ch * 1.4;
        let cy = y + ch * 0.2;
        out.push_str(&format!(r#"<rect x="{m}" y="{cy}" width="{cw}" height="{ch}" rx="{}" fill="{cta_bg}"/><text x="{}" y="{}" text-anchor="middle" font-family="{bf}" font-weight="700" font-size="{}" fill="{cta_fg}">{}</text>"#, ch / 2.0, m + cw / 2.0, cy + ch * 0.64, ch * 0.42, esc(cta)));
    }
    if let Some(icon) = &input.icon_svg {
        let s = base * 0.1;
        out.push_str(&embed(icon, w - m - s, h - m - s, s, s));
    }
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><rect width="{w}" height="{h}" fill="{bg}"/>{out}</svg>"#);
    let png = png_from_svg(&svg, 1.0)?;
    Ok((svg, png))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logo::{variants, Concept, Variant};

    #[test]
    fn all_sizes_render_and_book_builds() {
        let mut b = BrandStandards::default();
        b.name = "Harbour Bakery".into();
        b.tagline = Some("Small-batch sourdough on the waterfront".into());
        let logos = variants(&b, Concept::Monogram).unwrap();
        let icon = logos.iter().find(|l| l.variant == Variant::Icon).unwrap().svg.clone();
        let input = SocialInput { headline: "Saturday rye is back".into(), body: Some("Fresh from the oven at 7am. First come, first served.".into()), cta: Some("Order ahead".into()), tone: None, photo: None, icon_svg: Some(icon) };
        for (size, _, w, h) in SOCIAL_SIZES {
            let (_, png) = social_post(&b, &input, size).unwrap();
            let img = image_size(&png);
            assert_eq!(img, (w, h), "{size:?}");
            if let Ok(dir) = std::env::var("JUGG_PRINT_DUMP") {
                std::fs::write(format!("{dir}/social-{size:?}.png"), &png).unwrap();
            }
        }
        let (pdf, cover) = crate::brand_book(&b, Concept::Monogram, &logos).unwrap();
        assert!(pdf.starts_with(b"%PDF-") && cover.starts_with(&[0x89, b'P']));
        assert_eq!(String::from_utf8_lossy(&pdf).matches("/TrimBox").count(), 6, "six pages");
        if let Ok(dir) = std::env::var("JUGG_PRINT_DUMP") {
            std::fs::write(format!("{dir}/brand-book.pdf"), &pdf).unwrap();
        }
    }

    fn image_size(png: &[u8]) -> (u32, u32) {
        (u32::from_be_bytes([png[16], png[17], png[18], png[19]]), u32::from_be_bytes([png[20], png[21], png[22], png[23]]))
    }
}
