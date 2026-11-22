//! Logo concepts generated as vectors straight from the brand: no image model
//! needed, always on-palette, and outlined (text → paths) so the files render
//! identically everywhere without fonts.

use crate::{esc, png_from_svg, tree, PrintError, Result};
use jugg_brand::BrandStandards;
use serde::{Deserialize, Serialize};
use svg2pdf::usvg;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Concept {
    Monogram,
    Wordmark,
    Badge,
    Initials,
}

impl Concept {
    pub const ALL: [Concept; 4] = [Concept::Monogram, Concept::Wordmark, Concept::Badge, Concept::Initials];
    pub fn label(self) -> &'static str {
        match self {
            Concept::Monogram => "Monogram",
            Concept::Wordmark => "Wordmark",
            Concept::Badge => "Badge",
            Concept::Initials => "Initials",
        }
    }
}

/// Colour roles; variants just swap the palette.
#[derive(Clone)]
struct Pal {
    mark: String,
    on_mark: String,
    text: String,
    sub: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    /// Full colour, for light backgrounds.
    Primary,
    /// For dark backgrounds.
    Reversed,
    /// One colour (ink), for stamps, embossing, faxes, single-colour print.
    Mono,
    /// The mark alone, square: app icons, avatars, favicons.
    Icon,
}

impl Variant {
    pub const ALL: [Variant; 4] = [Variant::Primary, Variant::Reversed, Variant::Mono, Variant::Icon];
    pub fn label(self) -> &'static str {
        match self {
            Variant::Primary => "Primary",
            Variant::Reversed => "Reversed (dark backgrounds)",
            Variant::Mono => "One colour",
            Variant::Icon => "Icon",
        }
    }
}

fn palette(b: &BrandStandards, v: Variant) -> Pal {
    let c = |s: &str, f: &str| b.color(s).unwrap_or(f).to_string();
    let (ink, surface, primary, pink, soft) = (c("ink", "#1b1a17"), c("surface", "#faf8f4"), c("primary", "#c2502a"), c("primary-ink", "#ffffff"), c("ink-soft", "#5e5a52"));
    match v {
        Variant::Primary | Variant::Icon => Pal { mark: primary, on_mark: pink, text: ink, sub: soft },
        Variant::Reversed => Pal { mark: primary, on_mark: pink, text: surface.clone(), sub: surface },
        Variant::Mono => Pal { mark: ink.clone(), on_mark: surface, text: ink.clone(), sub: ink },
    }
}

pub fn initials(name: &str, max: usize) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().next().is_some_and(|c| c.is_alphanumeric())).collect();
    let s: String = words.iter().take(max).filter_map(|w| w.chars().next()).flat_map(|c| c.to_uppercase()).collect();
    if s.is_empty() {
        "•".into()
    } else {
        s
    }
}

/// The mark part of a concept, centred on (cx, cy), `size` across.
fn mark(concept: Concept, b: &BrandStandards, p: &Pal, cx: f32, cy: f32, size: f32) -> String {
    let font = esc(&b.typography.heading.family);
    let r = size / 2.0;
    let one = esc(&initials(&b.name, 1));
    match concept {
        Concept::Monogram | Concept::Wordmark => format!(
            r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{m}"/><text x="{cx}" y="{ty}" text-anchor="middle" font-family="{font}" font-weight="700" font-size="{fs}" fill="{o}">{one}</text>"#,
            m = p.mark, o = p.on_mark, ty = cy + size * 0.19, fs = size * 0.56
        ),
        Concept::Badge => format!(
            r#"<rect x="{x}" y="{y}" width="{size}" height="{size}" rx="{rx}" fill="{m}"/><text x="{cx}" y="{ty}" text-anchor="middle" font-family="{font}" font-weight="700" font-size="{fs}" fill="{o}">{one}</text>"#,
            x = cx - r, y = cy - r, rx = size * 0.24, m = p.text, o = if p.text == p.on_mark { p.mark.clone() } else { p.sub.replace(&p.sub, &p.on_mark) }, ty = cy + size * 0.19, fs = size * 0.56
        ),
        Concept::Initials => {
            let two = esc(&initials(&b.name, 2));
            let d = r * 0.98;
            format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{w}" rx="{rx}" fill="{m}" transform="rotate(45 {cx} {cy})"/><text x="{cx}" y="{ty}" text-anchor="middle" font-family="{font}" font-weight="700" font-size="{fs}" letter-spacing="-1" fill="{o}">{two}</text>"#,
                x = cx - d * 0.72, y = cy - d * 0.72, w = d * 1.44, rx = size * 0.14, m = p.mark, o = p.on_mark, ty = cy + size * 0.13, fs = size * if two.chars().count() > 1 { 0.36 } else { 0.5 }
            )
        }
    }
}

/// Unpadded content for a concept/variant, laid out on a generous canvas.
fn content(concept: Concept, b: &BrandStandards, v: Variant) -> String {
    let p = palette(b, v);
    let font = esc(&b.typography.heading.family);
    let name = esc(&b.name);
    if v == Variant::Icon {
        let c = if concept == Concept::Wordmark { Concept::Badge } else { concept };
        let p = if c == Concept::Badge { Pal { text: p.mark.clone(), on_mark: p.on_mark.clone(), ..p } } else { p };
        return mark(c, b, &p, 128.0, 128.0, 200.0);
    }
    match concept {
        Concept::Monogram => format!(
            r#"{m}<text x="168" y="138" font-family="{font}" font-weight="700" font-size="54" letter-spacing="-1.2" fill="{t}">{name}</text>"#,
            m = mark(concept, b, &p, 88.0, 120.0, 112.0), t = p.text
        ),
        Concept::Wordmark => format!(
            r#"<text x="32" y="150" font-family="{font}" font-weight="700" font-size="72" letter-spacing="-2" fill="{t}">{name}<tspan fill="{m}">.</tspan></text>"#,
            t = p.text, m = p.mark
        ),
        Concept::Badge => {
            let p2 = Pal { text: if v == Variant::Reversed { p.mark.clone() } else { p.text.clone() }, ..p.clone() };
            format!(
                r#"{m}<text x="400" y="292" text-anchor="middle" font-family="{font}" font-weight="700" font-size="44" letter-spacing="-0.8" fill="{t}">{name}</text>"#,
                m = mark(concept, b, &p2, 400.0, 150.0, 140.0), t = p.text
            )
        }
        Concept::Initials => {
            // Tagline only if it fits whole (at a word boundary); never cut mid-word.
            let tag = b.tagline.as_deref().and_then(|t| {
                let mut out = String::new();
                for w in t.split_whitespace() {
                    if out.chars().count() + w.chars().count() + 1 > 42 {
                        return (out.chars().count() >= 18).then_some(out);
                    }
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(w);
                }
                Some(out)
            }).map(|t| {
                format!(r#"<text x="196" y="168" font-family="{body}" font-weight="500" font-size="20" fill="{s}">{t}</text>"#, body = esc(&b.typography.body.family), s = p.sub, t = esc(&t))
            });
            format!(
                r#"{m}<text x="196" y="{ny}" font-family="{font}" font-weight="700" font-size="50" letter-spacing="-1" fill="{t}">{name}</text>{tag}"#,
                m = mark(concept, b, &p, 100.0, 120.0, 128.0), t = p.text, ny = if tag.is_some() { 128 } else { 138 }, tag = tag.unwrap_or_default()
            )
        }
    }
}

fn wrap(content: &str, vb: (f32, f32, f32, f32)) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}">{content}</svg>"#, x = vb.0, y = vb.1, w = vb.2, h = vb.3)
}

/// Text → paths, tight viewBox with padding. The result needs no fonts.
fn outline(content: &str, pad: f32, square: bool) -> Result<String> {
    let probe = tree(&wrap(content, (0.0, 0.0, 2400.0, 800.0)))?;
    let bb = probe.root().abs_stroke_bounding_box();
    let (mut x, mut y, mut w, mut h) = (bb.x() - pad, bb.y() - pad, bb.width() + 2.0 * pad, bb.height() + 2.0 * pad);
    if square {
        let s = w.max(h);
        x -= (s - w) / 2.0;
        y -= (s - h) / 2.0;
        w = s;
        h = s;
    }
    let t = tree(&wrap(content, (x, y, w, h)))?;
    Ok(t.to_string(&usvg::WriteOptions::default()))
}

#[derive(Debug, Clone)]
pub struct LogoFile {
    pub concept: Concept,
    pub variant: Variant,
    pub svg: String,
    pub png: Vec<u8>,
}

pub fn logo(b: &BrandStandards, concept: Concept, variant: Variant) -> Result<LogoFile> {
    let svg = outline(&content(concept, b, variant), if variant == Variant::Icon { 16.0 } else { 24.0 }, variant == Variant::Icon)?;
    // Reversed and light-mono previews sit on ink so they're visible in the library.
    let preview_svg = if variant == Variant::Reversed {
        let bg = b.color("ink").unwrap_or("#1b1a17");
        svg.replacen('>', &format!("><rect x=\"-10000\" y=\"-10000\" width=\"20000\" height=\"20000\" fill=\"{bg}\"/>"), 1)
    } else {
        svg.clone()
    };
    let scale = if variant == Variant::Icon { 2.0 } else { 1.5 };
    let png = png_from_svg(&preview_svg, scale).map_err(|e| PrintError::Render(e.to_string()))?;
    Ok(LogoFile { concept, variant, svg, png })
}

/// Four concepts in primary colours, for the owner to choose from.
pub fn concepts(b: &BrandStandards) -> Result<Vec<LogoFile>> {
    Concept::ALL.iter().map(|c| logo(b, *c, Variant::Primary)).collect()
}

/// The full set for the chosen concept.
pub fn variants(b: &BrandStandards, concept: Concept) -> Result<Vec<LogoFile>> {
    Variant::ALL.iter().map(|v| logo(b, concept, *v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concepts_are_outlined_and_on_palette() {
        let mut b = BrandStandards::default();
        b.name = "Harbour Bakery".into();
        b.tagline = Some("Small-batch sourdough".into());
        for l in concepts(&b).unwrap() {
            assert!(!l.svg.contains("<text"), "{:?} still has live text", l.concept);
            assert!(l.svg.contains("<path"));
            let r = jugg_brand::gate::check_svg(&b, &l.svg, jugg_brand::gate::Strictness::Exact);
            assert!(r.passed, "{:?}: {:?}", l.concept, r.findings);
            assert!(l.png.starts_with(&[0x89, b'P']));
            if let Ok(dir) = std::env::var("JUGG_PRINT_DUMP") {
                std::fs::write(format!("{dir}/concept-{:?}.png", l.concept), &l.png).unwrap();
            }
        }
        let vs = variants(&b, Concept::Monogram).unwrap();
        assert_eq!(vs.len(), 4);
        let icon = &vs[3];
        let t = crate::tree(&icon.svg).unwrap();
        assert!((t.size().width() - t.size().height()).abs() < 0.5, "icon is square");
        assert_eq!(initials("harbour bakery & co", 2), "HB");
    }
}
