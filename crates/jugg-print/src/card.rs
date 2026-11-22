//! Standard North American business card: 3.5 × 2 in trim, 0.125 in bleed,
//! 0.125 in safe margin inside the trim. Front = brand mark, back = details.

use crate::{esc, pdf_from_svgs, png_from_svg, Result};
use jugg_brand::BrandStandards;
use serde::{Deserialize, Serialize};

/// Trim size in points (3.5 × 2 in).
pub const CARD_TRIM_PT: (f32, f32) = (252.0, 144.0);
pub const CARD_BLEED_PT: f32 = 9.0;
const SAFE: f32 = 9.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardInput {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    /// Logo as SVG markup (from the brand's vectorized logo); monogram if absent.
    #[serde(skip)]
    pub logo_svg: Option<String>,
}

pub struct Card {
    pub pdf: Vec<u8>,
    pub front_png: Vec<u8>,
    pub back_png: Vec<u8>,
    pub front_svg: String,
    pub back_svg: String,
}

fn color<'a>(b: &'a BrandStandards, slug: &str, fallback: &'a str) -> &'a str {
    b.color(slug).unwrap_or(fallback)
}

fn mark(b: &BrandStandards, input: &CardInput, cx: f32, cy: f32, size: f32, on_dark: bool) -> String {
    if let Some(svg) = &input.logo_svg {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
        return format!(r#"<image x="{x}" y="{y}" width="{size}" height="{size}" preserveAspectRatio="xMidYMid meet" href="data:image/svg+xml;base64,{data}"/>"#, x = cx - size / 2.0, y = cy - size / 2.0);
    }
    let initial: String = b.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "•".into());
    let (fill, ink) = if on_dark { (color(b, "surface", "#faf8f4"), color(b, "ink", "#1b1a17")) } else { (color(b, "primary", "#c2502a"), color(b, "primary-ink", "#ffffff")) };
    format!(
        r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{fill}"/><text x="{cx}" y="{ty}" text-anchor="middle" font-family="{font}" font-weight="700" font-size="{fs}" fill="{ink}">{initial}</text>"#,
        r = size / 2.0,
        ty = cy + size * 0.18,
        fs = size * 0.5,
        font = esc(&b.typography.heading.family),
        initial = esc(&initial)
    )
}

pub fn business_card(b: &BrandStandards, input: &CardInput) -> Result<Card> {
    let (tw, th) = CARD_TRIM_PT;
    let (w, h) = (tw + 2.0 * CARD_BLEED_PT, th + 2.0 * CARD_BLEED_PT);
    let (x0, y0) = (CARD_BLEED_PT + SAFE, CARD_BLEED_PT + SAFE);
    let heading = esc(&b.typography.heading.family);
    let body = esc(&b.typography.body.family);
    let ink = color(b, "ink", "#1b1a17");
    let ink_soft = color(b, "ink-soft", "#5e5a52");
    let surface = color(b, "surface", "#faf8f4");
    let primary = color(b, "primary", "#c2502a");

    // Front: dark field, centred mark, business name and tagline.
    let tagline = b.tagline.as_deref().map(|t| {
        let t: String = t.chars().take(70).collect();
        format!(r#"<text x="{cx}" y="{y}" text-anchor="middle" font-family="{body}" font-size="6.5" fill="{surface}" fill-opacity="0.75">{t}</text>"#, cx = w / 2.0, y = h / 2.0 + 34.0, t = esc(&t))
    });
    let front = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="{w}" height="{h}" fill="{ink}"/>
{mark}
<text x="{cx}" y="{ny}" text-anchor="middle" font-family="{heading}" font-weight="700" font-size="13" letter-spacing="-0.2" fill="{surface}">{name}</text>
{tagline}
</svg>"#,
        mark = mark(b, input, w / 2.0, h / 2.0 - 16.0, 34.0, true),
        cx = w / 2.0,
        ny = h / 2.0 + 20.0,
        name = esc(&b.name),
        tagline = tagline.unwrap_or_default(),
    );

    // Back: light field, person + title, accent rule, contact lines.
    let mut lines = Vec::new();
    for v in [&input.phone, &input.email, &input.website, &input.address].into_iter().flatten() {
        let v = v.trim();
        if !v.is_empty() {
            lines.push(esc(v));
        }
    }
    let contact: String = lines
        .iter()
        .enumerate()
        .map(|(i, l)| format!(r#"<text x="{x0}" y="{y}" font-family="{body}" font-size="7" fill="{ink}">{l}</text>"#, y = h - y0 - (lines.len() - 1 - i) as f32 * 10.5))
        .collect();
    let back = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="{w}" height="{h}" fill="{surface}"/>
<rect x="0" y="0" width="{w}" height="{band}" fill="{primary}"/>
{mark}
<text x="{x0}" y="{ny}" font-family="{heading}" font-weight="700" font-size="12" letter-spacing="-0.2" fill="{ink}">{name}</text>
<text x="{x0}" y="{ty}" font-family="{body}" font-size="7" fill="{ink_soft}">{title}</text>
<rect x="{x0}" y="{ry}" width="22" height="1.6" fill="{primary}"/>
{contact}
</svg>"#,
        band = CARD_BLEED_PT + 4.0,
        mark = mark(b, input, w - x0 - 11.0, y0 + 15.0, 22.0, false),
        ny = y0 + 22.0,
        ty = y0 + 33.0,
        ry = y0 + 41.0,
        name = esc(input.name.trim()),
        title = esc(input.title.as_deref().unwrap_or("")),
    );

    let pdf = pdf_from_svgs(&[front.clone(), back.clone()], w, h, CARD_BLEED_PT, &format!("{} — business card ({})", b.name, input.name))?;
    Ok(Card { pdf, front_png: png_from_svg(&front, 2.0)?, back_png: png_from_svg(&back, 2.0)?, front_svg: front, back_svg: back })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_is_a_two_page_print_pdf_on_brand() {
        let mut b = BrandStandards::default();
        b.name = "Harbour Bakery".into();
        b.tagline = Some("Small-batch sourdough on the waterfront".into());
        let card = business_card(&b, &CardInput { name: "Ada Baker".into(), title: Some("Head baker".into()), email: Some("ada@harbour.test".into()), phone: Some("+1 250 555 0100".into()), ..Default::default() }).unwrap();
        assert!(card.pdf.starts_with(b"%PDF-"));
        let s = String::from_utf8_lossy(&card.pdf);
        assert_eq!(s.matches("/Type /Page\n").count() + s.matches("/Type /Page ").count() + s.matches("/Type /Page/").count(), 2, "two pages");
        assert!(s.contains("/TrimBox"), "trim box set for the printer");
        assert!(card.front_png.starts_with(&[0x89, b'P', b'N', b'G']));
        if let Ok(dir) = std::env::var("JUGG_PRINT_DUMP") {
            std::fs::write(format!("{dir}/card.pdf"), &card.pdf).unwrap();
            std::fs::write(format!("{dir}/front.png"), &card.front_png).unwrap();
            std::fs::write(format!("{dir}/back.png"), &card.back_png).unwrap();
        }
        // Only brand colours in the layout.
        for svg in [&card.front_svg, &card.back_svg] {
            let findings = jugg_brand::lint_css(&b, &svg.replace("fill=\"", "fill:").replace("\"/>", ";"));
            assert!(findings.iter().all(|f| f.rule != "brand.color.off_palette"), "{findings:?}");
        }
        // Text is escaped.
        let evil = business_card(&b, &CardInput { name: "<script>x</script>".into(), ..Default::default() }).unwrap();
        assert!(!evil.back_svg.contains("<script>"));
    }
}
