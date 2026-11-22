//! Brand book: a 16:9 PDF the owner can share with printers, designers and staff.

use crate::logo::{Concept, LogoFile, Variant};
use crate::{esc, pdf_from_svgs, png_from_svg, Result};
use jugg_brand::BrandStandards;

const W: f32 = 960.0;
const H: f32 = 540.0;

/// Place an outlined logo SVG inside a box (nested <svg>, aspect kept).
pub(crate) fn embed(svg: &str, x: f32, y: f32, w: f32, h: f32) -> String {
    let head = &svg[..svg.find('>').unwrap_or(0)];
    let attr = |name: &str| head.split(&format!(" {name}=\"")).nth(1).and_then(|s| s.split('"').next()).map(str::to_string);
    // usvg's writer emits width/height without a viewBox.
    let vb = attr("viewBox").unwrap_or_else(|| format!("0 0 {} {}", attr("width").unwrap_or_else(|| "100".into()), attr("height").unwrap_or_else(|| "100".into())));
    let start = svg.find('>').map(|i| i + 1).unwrap_or(0);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    format!(r#"<svg x="{x}" y="{y}" width="{w}" height="{h}" viewBox="{vb}" preserveAspectRatio="xMidYMid meet">{}</svg>"#, &svg[start..end])
}

/// Like `wrap_text`, but evens out line lengths so a headline never ends on one lonely word.
pub(crate) fn balanced(text: &str, max_chars: usize) -> Vec<String> {
    let first = wrap_text(text, max_chars);
    if first.len() < 2 {
        return first;
    }
    let total = text.chars().count();
    let mut target = total.div_ceil(first.len());
    while target <= max_chars {
        let lines = wrap_text(text, target);
        if lines.len() == first.len() {
            return lines;
        }
        target += 1;
    }
    first
}

pub(crate) fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max_chars {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn page(bg: &str, body: String) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}"><rect width="{W}" height="{H}" fill="{bg}"/>{body}</svg>"#)
}

fn rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    (p(0), p(2), p(4))
}

fn cmyk(hex: &str) -> String {
    let (r, g, b) = rgb(hex);
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let k = 1.0 - r.max(g).max(b);
    if k >= 0.999 {
        return "0 0 0 100".into();
    }
    let f = |c: f32| ((1.0 - c - k) / (1.0 - k) * 100.0).round();
    format!("{} {} {} {}", f(r), f(g), f(b), (k * 100.0).round())
}

pub fn brand_book(b: &BrandStandards, concept: Concept, logos: &[LogoFile]) -> Result<(Vec<u8>, Vec<u8>)> {
    let c = |s: &str, f: &str| b.color(s).unwrap_or(f).to_string();
    let (ink, soft, surface, raised, line, primary) = (c("ink", "#1b1a17"), c("ink-soft", "#5e5a52"), c("surface", "#faf8f4"), c("surface-raised", "#ffffff"), c("line", "#e7e2d8"), c("primary", "#c2502a"));
    let hf = esc(&b.typography.heading.family);
    let bf = esc(&b.typography.body.family);
    let get = |v: Variant| logos.iter().find(|l| l.variant == v).map(|l| l.svg.clone()).unwrap_or_default();
    let title = |eyebrow: &str, t: &str, color: &str| {
        format!(
            r#"<text x="64" y="84" font-family="{bf}" font-weight="600" font-size="13" letter-spacing="2.4" fill="{primary}">{e}</text><text x="64" y="126" font-family="{hf}" font-weight="700" font-size="38" letter-spacing="-0.8" fill="{color}">{t}</text>"#,
            e = esc(&eyebrow.to_uppercase()),
            t = esc(t)
        )
    };
    let para = |x: f32, y: f32, text: &str, size: f32, color: &str, chars: usize| -> String {
        wrap_text(text, chars).iter().enumerate().map(|(i, l)| format!(r#"<text x="{x}" y="{}" font-family="{bf}" font-size="{size}" fill="{color}">{}</text>"#, y + i as f32 * size * 1.5, esc(l))).collect()
    };
    let mut pages = Vec::new();

    // 1. Cover
    pages.push(page(&ink, format!(
        r#"{logo}<text x="{cx}" y="430" text-anchor="middle" font-family="{bf}" font-weight="600" font-size="14" letter-spacing="3" fill="{surface}" fill-opacity="0.7">BRAND GUIDELINES</text>{tag}"#,
        logo = embed(&get(Variant::Reversed), 180.0, 150.0, 600.0, 190.0),
        cx = W / 2.0,
        tag = b.tagline.as_deref().map(|t| format!(r#"<text x="{}" y="460" text-anchor="middle" font-family="{bf}" font-size="15" fill="{surface}" fill-opacity="0.55">{}</text>"#, W / 2.0, esc(t))).unwrap_or_default()
    )));

    // 2. Logo
    pages.push(page(&surface, format!(
        r#"{t}<rect x="64" y="160" width="400" height="220" rx="16" fill="{raised}" stroke="{line}"/>{p}<rect x="496" y="160" width="400" height="220" rx="16" fill="{ink}"/>{r}
        <rect x="64" y="400" width="110" height="100" rx="16" fill="{raised}" stroke="{line}"/>{i}
        {note}"#,
        t = title(&format!("Logo · {}", concept.label()), "Your logo", &ink),
        p = embed(&get(Variant::Primary), 96.0, 190.0, 336.0, 160.0),
        r = embed(&get(Variant::Reversed), 528.0, 190.0, 336.0, 160.0),
        i = embed(&get(Variant::Icon), 89.0, 410.0, 60.0, 80.0),
        note = para(200.0, 430.0, &format!("Clear space: keep {:.0}% of the logo's height free on every side. Smallest size: {} px wide on screen, 25 mm in print. Use the icon where space is tight (app icons, avatars, favicons).", b.logo.clearspace.max(0.25) * 100.0, b.logo.min_px.max(24)), 13.0, &soft, 82),
    )));

    // 3. Don'ts
    let dont = |x: f32, label: &str, transform: &str, fill_override: Option<&str>| {
        let mut svg = get(Variant::Primary);
        if let Some(f) = fill_override {
            svg = svg.replace(&primary, f);
        }
        format!(
            r#"<rect x="{x}" y="170" width="190" height="190" rx="14" fill="{raised}" stroke="{line}"/><g transform="{transform}">{logo}</g><line x1="{x1}" y1="188" x2="{x2}" y2="342" stroke="{primary}" stroke-width="3"/><text x="{cx}" y="392" text-anchor="middle" font-family="{bf}" font-size="13" fill="{soft}">{l}</text>"#,
            logo = embed(&svg, x + 20.0, 225.0, 150.0, 80.0),
            x1 = x + 18.0,
            x2 = x + 172.0,
            cx = x + 95.0,
            l = esc(label)
        )
    };
    pages.push(page(&surface, format!(
        "{}{}{}{}{}{}",
        title("Logo", "Please don't", &ink),
        dont(64.0, "Stretch or squash it", &format!("translate({} 265) scale(1.35 0.7) translate({} -265)", 64.0 + 95.0, -(64.0 + 95.0)), None),
        dont(278.0, "Recolour it", "", Some("#3a7bd5")),
        dont(492.0, "Rotate it", &format!("rotate(-18 {} 265)", 492.0 + 95.0), None),
        dont(706.0, "Add effects or outlines", "", None).replace("<g transform=\"\">", "<g opacity=\"0.45\">"),
        para(64.0, 450.0, &if b.logo.dont.is_empty() { "Also avoid busy photo backgrounds behind the logo; use the reversed version on dark colours.".to_string() } else { b.logo.dont.join(" · ") }, 14.0, &soft, 110),
    )));

    // 4. Colour
    let n = b.colors.len().max(1) as f32;
    let sw = ((W - 128.0 - (n - 1.0) * 14.0) / n).min(200.0);
    let swatches: String = b
        .colors
        .iter()
        .enumerate()
        .map(|(i, col)| {
            let x = 64.0 + i as f32 * (sw + 14.0);
            let (r, g, bl) = rgb(&col.hex);
            format!(
                r#"<rect x="{x}" y="160" width="{sw}" height="150" rx="14" fill="{hex}" stroke="{line}"/><text x="{x}" y="336" font-family="{hf}" font-weight="700" font-size="14" fill="{ink}">{slug}</text><text x="{x}" y="356" font-family="{bf}" font-size="12" fill="{soft}">{hex}</text><text x="{x}" y="374" font-family="{bf}" font-size="12" fill="{soft}">RGB {r} {g} {bl}</text><text x="{x}" y="392" font-family="{bf}" font-size="12" fill="{soft}">CMYK {cmyk}</text>{usage}"#,
                hex = col.hex,
                slug = esc(&col.slug),
                cmyk = cmyk(&col.hex),
                usage = wrap_text(&col.usage, (sw / 6.5) as usize).iter().take(3).enumerate().map(|(j, l)| format!(r#"<text x="{x}" y="{}" font-family="{bf}" font-size="11.5" fill="{soft}" fill-opacity="0.85">{}</text>"#, 416.0 + j as f32 * 16.0, esc(l))).collect::<String>()
            )
        })
        .collect();
    pages.push(page(&surface, format!("{}{swatches}{}", title("Colour", "Palette", &ink), para(64.0, 500.0, "CMYK values are approximate conversions; ask your printer for a proof on your chosen paper.", 11.5, &soft, 120))));

    // 5. Typography
    let t = &b.typography;
    let sizes: String = (0..5)
        .map(|i| {
            let px = t.base_px * t.scale_ratio.powi(4 - i);
            let y = 200.0 + (0..i).map(|j| t.base_px * t.scale_ratio.powi(4 - j) * 1.5).sum::<f32>();
            format!(r#"<text x="520" y="{y}" font-family="{f}" font-weight="{w}" font-size="{px:.1}" fill="{ink}">{label} · {px:.0}px</text>"#, f = if i < 3 { &hf } else { &bf }, w = if i < 3 { 700 } else { 400 }, label = ["Display", "Heading", "Subheading", "Body", "Small"][i as usize])
        })
        .collect();
    pages.push(page(&surface, format!(
        r#"{t0}<text x="64" y="230" font-family="{hf}" font-weight="700" font-size="88" fill="{ink}">Aa</text><text x="64" y="270" font-family="{hf}" font-weight="700" font-size="18" fill="{ink}">{hfam}</text><text x="64" y="292" font-family="{bf}" font-size="13" fill="{soft}">Headings · weights {hw}</text>
        <text x="64" y="380" font-family="{bf}" font-size="40" fill="{ink}">Aa Bb Cc 123</text><text x="64" y="414" font-family="{bf}" font-weight="600" font-size="16" fill="{ink}">{bfam}</text><text x="64" y="436" font-family="{bf}" font-size="13" fill="{soft}">Body text · weights {bw}</text>{sizes}"#,
        t0 = title("Typography", "Type", &ink),
        hfam = esc(&t.heading.family),
        bfam = esc(&t.body.family),
        hw = t.heading.weights.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(", "),
        bw = t.body.weights.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(", "),
    )));

    // 6. Voice & imagery
    let traits = if b.voice.traits.is_empty() { "clear, warm, direct".into() } else { b.voice.traits.join(", ") };
    let imagery = if b.imagery.style.is_empty() { "Natural light, real people and products, honest colour. Grade photos toward the palette.".into() } else { b.imagery.style.join(", ") };
    let avoid = if b.imagery.avoid.is_empty() { "Generic stock poses, heavy filters, clutter.".into() } else { b.imagery.avoid.join(", ") };
    pages.push(page(&surface, format!(
        "{}{}{}{}{}{}{}",
        title("Voice & imagery", "How we sound and look", &ink),
        format!(r#"<text x="64" y="190" font-family="{hf}" font-weight="700" font-size="18" fill="{ink}">Voice</text>"#),
        para(64.0, 218.0, &format!("We are {traits}. We write at about a {} reading level: short sentences, everyday words, the useful thing first.{}", if b.voice.reading_level.is_empty() { "grade 8" } else { &b.voice.reading_level }, if b.voice.banned_phrases.is_empty() { String::new() } else { format!(" We never say: {}.", b.voice.banned_phrases.join(", ")) }), 15.0, &soft, 52),
        format!(r#"<text x="520" y="190" font-family="{hf}" font-weight="700" font-size="18" fill="{ink}">Imagery</text>"#),
        para(520.0, 218.0, &imagery, 15.0, &soft, 48),
        format!(r#"<text x="520" y="330" font-family="{hf}" font-weight="700" font-size="16" fill="{ink}">Avoid</text>"#),
        para(520.0, 356.0, &avoid, 15.0, &soft, 48),
    )));

    let pdf = pdf_from_svgs(&pages, W, H, 0.0, &format!("{} — brand guidelines", b.name))?;
    let cover_png = png_from_svg(&pages[0], 0.6)?;
    Ok((pdf, cover_png))
}
