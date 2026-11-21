use crate::BrandStandards;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    pub fn error(rule: &str, message: impl Into<String>) -> Self {
        Self { rule: rule.into(), severity: Severity::Error, message: message.into() }
    }
    pub fn warn(rule: &str, message: impl Into<String>) -> Self {
        Self { rule: rule.into(), severity: Severity::Warn, message: message.into() }
    }
}

/// Token gate for generated CSS: every colour must be a brand token (or a
/// `var(--color-*)` reference) and every font family must be a brand font.
pub fn lint_css(brand: &BrandStandards, css: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let allowed: Vec<String> = brand.colors.iter().map(|c| c.hex.to_ascii_lowercase()).collect();
    let hex = regex::Regex::new(r"#(?:[0-9a-fA-F]{6}|[0-9a-fA-F]{3})\b").unwrap();
    for m in hex.find_iter(css) {
        let v = normalise_hex(m.as_str());
        if !allowed.contains(&v) {
            out.push(Finding::error("brand.color.off_palette", format!("colour {} is not a brand token; use var(--color-…)", m.as_str())));
        }
    }
    let rgb = regex::Regex::new(r"\b(?:rgb|rgba|hsl|hsla|oklch|lab)\(").unwrap();
    if rgb.is_match(css) {
        out.push(Finding::warn("brand.color.functional", "functional colour notation found; prefer brand tokens"));
    }
    let ff = regex::Regex::new(r"font-family\s*:\s*([^;}]+)").unwrap();
    let fonts: Vec<String> = brand.font_families().iter().map(|f| f.to_ascii_lowercase()).collect();
    for cap in ff.captures_iter(css) {
        let first = cap[1].split(',').next().unwrap_or("").trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
        if first.starts_with("var(") || fonts.contains(&first) {
            continue;
        }
        out.push(Finding::error("brand.font.off_brand", format!("font `{first}` is not a brand font")));
    }
    out
}

fn normalise_hex(h: &str) -> String {
    let h = h.to_ascii_lowercase();
    if h.len() == 4 {
        let c: Vec<char> = h.chars().collect();
        format!("#{0}{0}{1}{1}{2}{2}", c[1], c[2], c[3])
    } else {
        h
    }
}

/// WCAG 2.x contrast ratio between two `#rrggbb` colours.
pub fn contrast_ratio(a: &str, b: &str) -> Option<f64> {
    fn lum(hex: &str) -> Option<f64> {
        let h = hex.trim_start_matches('#');
        if h.len() != 6 {
            return None;
        }
        let ch = |i: usize| -> Option<f64> {
            let v = u8::from_str_radix(&h[i..i + 2], 16).ok()? as f64 / 255.0;
            Some(if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
        };
        Some(0.2126 * ch(0)? + 0.7152 * ch(2)? + 0.0722 * ch(4)?)
    }
    let (la, lb) = (lum(a)?, lum(b)?);
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_off_palette_and_off_brand_font() {
        let b = BrandStandards::default();
        let css = ".a{color:#1B1A17;background:#ff00ff;font-family:'Comic Sans MS',cursive}.b{font-family:var(--font-body)}";
        let f = lint_css(&b, css);
        assert!(f.iter().any(|x| x.rule == "brand.color.off_palette" && x.message.contains("#ff00ff")));
        assert!(f.iter().any(|x| x.rule == "brand.font.off_brand"));
        assert_eq!(f.iter().filter(|x| x.rule == "brand.color.off_palette").count(), 1, "#1B1A17 is a token");
    }

    #[test]
    fn contrast_known_values() {
        assert!((contrast_ratio("#000000", "#ffffff").unwrap() - 21.0).abs() < 0.01);
        assert!((contrast_ratio("#777777", "#ffffff").unwrap() - 4.48).abs() < 0.02);
    }
}
