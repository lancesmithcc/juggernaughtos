//! The brand gate: every asset is checked here before the owner sees it as
//! ready. Errors → `rejected` (with the report, so the agent can fix it);
//! warnings only → `review`. Owner approval moves it to `final`.

use crate::lint::{Finding, Severity};
use crate::BrandStandards;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateReport {
    pub passed: bool,
    pub summary: String,
    pub findings: Vec<Finding>,
}

impl GateReport {
    fn from(findings: Vec<Finding>, what: &str) -> Self {
        let errors = findings.iter().filter(|f| f.severity == Severity::Error).count();
        let warns = findings.iter().filter(|f| f.severity == Severity::Warn).count();
        let summary = match (errors, warns) {
            (0, 0) => format!("{what} matches the brand."),
            (0, w) => format!("{what} is on brand with {w} note(s)."),
            (e, _) => format!("{what} breaks {e} brand rule(s)."),
        };
        Self { passed: errors == 0, summary, findings }
    }
}

/// How strict colour checks are for this kind of asset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strictness {
    /// Logos, icons, cards, signatures: every colour must be a brand colour.
    Exact,
    /// Photos and textures: a natural photo can't be on-palette; only note it.
    Loose,
}

pub fn check_file(brand: &BrandStandards, path: &std::path::Path, strict: Strictness) -> GateReport {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let Ok(bytes) = std::fs::read(path) else {
        return GateReport::from(vec![Finding::error("asset.unreadable", "file could not be read")], "This file");
    };
    match ext.as_str() {
        "svg" => check_svg(brand, &String::from_utf8_lossy(&bytes), strict),
        "png" | "jpg" | "jpeg" | "webp" => check_raster(brand, &bytes, strict),
        "pdf" => check_pdf(&bytes),
        "html" | "htm" => check_html(brand, &String::from_utf8_lossy(&bytes)),
        "css" => GateReport::from(crate::lint_css(brand, &String::from_utf8_lossy(&bytes)), "This stylesheet"),
        _ => GateReport { passed: true, summary: "No brand checks apply to this file type.".into(), findings: vec![] },
    }
}

// ------------------------------------------------------------------ colour science

fn hex_rgb(h: &str) -> Option<[f64; 3]> {
    let h = h.trim_start_matches('#');
    let h = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect::<String>() } else { h.to_string() };
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| v as f64 / 255.0);
    Some([p(0)?, p(2)?, p(4)?])
}

fn lab(rgb: [f64; 3]) -> [f64; 3] {
    let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE76 ΔE: ~2 is barely visible, ~10 clearly different.
fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn palette(brand: &BrandStandards) -> Vec<(String, [f64; 3])> {
    brand.colors.iter().filter_map(|c| hex_rgb(&c.hex).map(|rgb| (c.slug.clone(), lab(rgb)))).collect()
}

fn nearest(p: &[(String, [f64; 3])], c: [f64; 3]) -> (String, f64) {
    p.iter().map(|(s, l)| (s.clone(), delta_e(*l, c))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or_default()
}

// ------------------------------------------------------------------ per type

pub fn check_svg(brand: &BrandStandards, svg: &str, strict: Strictness) -> GateReport {
    let mut f = Vec::new();
    if svg.contains("<script") || svg.contains("javascript:") || regex::Regex::new(r#"\son[a-z]+\s*="#).unwrap().is_match(svg) {
        f.push(Finding::error("asset.svg.active_content", "SVG contains scripts or event handlers"));
    }
    let pal = palette(brand);
    let hex = regex::Regex::new(r"#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b").unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for m in hex.find_iter(svg) {
        let v = m.as_str().to_ascii_lowercase();
        if !seen.insert(v.clone()) {
            continue;
        }
        let Some(rgb) = hex_rgb(&v) else { continue };
        let (slug, d) = nearest(&pal, lab(rgb));
        if d > 10.0 {
            let msg = format!("{v} is not a brand colour (closest: {slug}, ΔE {d:.0})");
            f.push(if strict == Strictness::Exact { Finding::error("brand.color.off_palette", msg) } else { Finding::warn("brand.color.off_palette", msg) });
        } else if d > 2.5 {
            f.push(Finding::warn("brand.color.near_palette", format!("{v} is close to {slug} (ΔE {d:.1}); snap it to the exact token")));
        }
    }
    let ff = regex::Regex::new(r#"font-family\s*[=:]\s*["']?([^;"'>,]+)"#).unwrap();
    let fonts: Vec<String> = brand.font_families().iter().map(|s| s.to_ascii_lowercase()).collect();
    for c in ff.captures_iter(svg) {
        let fam = c[1].trim().trim_matches('\'').to_ascii_lowercase();
        if !fam.is_empty() && !fonts.contains(&fam) {
            f.push(Finding::error("brand.font.off_brand", format!("font “{}” is not a brand font", c[1].trim())));
        }
    }
    GateReport::from(f, "This artwork")
}

pub fn check_raster(brand: &BrandStandards, bytes: &[u8], strict: Strictness) -> GateReport {
    let Ok(img) = image::load_from_memory(bytes) else {
        return GateReport::from(vec![Finding::error("asset.image.corrupt", "image could not be decoded")], "This image");
    };
    let mut f = Vec::new();
    if img.width() < 64 || img.height() < 64 {
        f.push(Finding::warn("asset.image.small", format!("only {}×{} px", img.width(), img.height())));
    }
    let small = img.thumbnail(64, 64).to_rgba8();
    let pal = palette(brand);
    let (mut total, mut off) = (0u32, 0u32);
    for p in small.pixels() {
        if p[3] < 128 {
            continue;
        }
        total += 1;
        let (_, d) = nearest(&pal, lab([p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0]));
        if d > 20.0 {
            off += 1;
        }
    }
    if total > 0 {
        let share = off as f64 / total as f64;
        let msg = format!("{:.0}% of the image is far from the brand palette", share * 100.0);
        match strict {
            Strictness::Exact if share > 0.25 => f.push(Finding::error("brand.color.off_palette", msg)),
            Strictness::Loose if share > 0.6 => f.push(Finding::warn("brand.color.off_palette", format!("{msg}; consider a brand-colour grade"))),
            _ => {}
        }
    }
    GateReport::from(f, "This image")
}

pub fn check_pdf(bytes: &[u8]) -> GateReport {
    let mut f = Vec::new();
    if !bytes.starts_with(b"%PDF-") {
        f.push(Finding::error("asset.pdf.invalid", "not a PDF file"));
    } else {
        let s = String::from_utf8_lossy(bytes);
        if !s.contains("/TrimBox") {
            f.push(Finding::warn("print.trim_box", "no TrimBox: printers may not know where to cut"));
        }
        if s.contains("/JavaScript") || s.contains("/JS ") || s.contains("/Launch") {
            f.push(Finding::error("asset.pdf.active_content", "PDF contains scripts or launch actions"));
        }
    }
    GateReport::from(f, "This PDF")
}

/// Email HTML uses inline styles by necessity; colours and fonts still must be brand.
pub fn check_html(brand: &BrandStandards, html: &str) -> GateReport {
    let mut f = Vec::new();
    let lower = html.to_ascii_lowercase();
    if lower.contains("<script") || lower.contains("javascript:") {
        f.push(Finding::error("asset.html.active_content", "HTML contains scripts"));
    }
    let styles: String = regex::Regex::new(r#"style="([^"]*)""#).unwrap().captures_iter(html).map(|c| format!("x{{{}}}", c[1].replace("&#39;", "'").replace("&quot;", "\""))).collect();
    f.extend(crate::lint_css(brand, &styles));
    GateReport::from(f, "This email design")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_gate() {
        let b = BrandStandards::default();
        let good = r##"<svg><rect fill="#c2502a"/><text font-family="Plus Jakarta Sans">Hi</text></svg>"##;
        assert!(check_svg(&b, good, Strictness::Exact).passed);
        let near = r##"<svg><rect fill="#cc5a34"/></svg>"##;
        let r = check_svg(&b, near, Strictness::Exact);
        assert!(r.passed && r.findings.iter().any(|f| f.rule == "brand.color.near_palette"));
        let bad = r##"<svg onload="x()"><rect fill="#00ff00"/><text font-family="Papyrus">x</text></svg>"##;
        let r = check_svg(&b, bad, Strictness::Exact);
        assert!(!r.passed && r.findings.len() >= 3, "{r:?}");
    }

    #[test]
    fn raster_gate() {
        let b = BrandStandards::default();
        let mut on = image::RgbaImage::from_pixel(128, 128, image::Rgba([194, 80, 42, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        on.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        assert!(check_raster(&b, buf.get_ref(), Strictness::Exact).passed);
        for p in on.pixels_mut() {
            *p = image::Rgba([0, 200, 0, 255]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        on.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        assert!(!check_raster(&b, buf.get_ref(), Strictness::Exact).passed);
        assert!(check_raster(&b, buf.get_ref(), Strictness::Loose).passed, "photos only get a note");
    }

    #[test]
    fn pdf_gate() {
        assert!(!check_pdf(b"hello").passed);
        assert!(!check_pdf(b"%PDF-1.7 /TrimBox /JavaScript").passed);
        assert!(check_pdf(b"%PDF-1.7 /TrimBox").passed);
    }
}
