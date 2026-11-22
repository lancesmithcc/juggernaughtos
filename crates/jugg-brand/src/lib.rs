//! Brand standards are the single source of truth for every asset and page.
//!
//! * `brand-standards.json` (this struct) is what tools consume.
//! * [`BrandStandards::to_css`] emits the only CSS variables generated sites may use.
//! * [`BrandStandards::to_design_md`] produces the DESIGN.md open-design expects.
//! * [`BrandStandards::image_prompt_preamble`] constrains image generation.
//! * [`lint_css`] is the token gate: raw colours / fonts outside the brand fail.

pub mod gate;
mod lint;

pub use lint::{contrast_ratio, lint_css, Finding, Severity};

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrandStandards {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub tagline: Option<String>,
    pub colors: Vec<ColorToken>,
    pub typography: Typography,
    #[serde(default)]
    pub spacing: Spacing,
    #[serde(default)]
    pub radius: Radius,
    #[serde(default)]
    pub motion: Motion,
    #[serde(default)]
    pub logo: LogoRules,
    #[serde(default)]
    pub imagery: ImageryRules,
    #[serde(default)]
    pub voice: Voice,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColorToken {
    /// Token slug: `ink`, `surface`, `primary`, `accent`, ...
    pub slug: String,
    pub hex: String,
    /// Where this colour may be used (fed to the AI and to reviewers).
    #[serde(default)]
    pub usage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Typography {
    pub heading: FontToken,
    pub body: FontToken,
    #[serde(default)]
    pub mono: Option<FontToken>,
    /// Modular scale ratio for type sizes (1.2 minor third, 1.25 major third...).
    pub scale_ratio: f32,
    pub base_px: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FontToken {
    /// Google Fonts family name; served from the local mirror, never Google's CDN.
    pub family: String,
    pub weights: Vec<u16>,
    pub fallback: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Spacing {
    pub unit_px: u16,
    pub scale: Vec<u16>,
}
impl Default for Spacing {
    fn default() -> Self {
        Self { unit_px: 4, scale: vec![0, 4, 8, 12, 16, 24, 32, 48, 64, 96, 128] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Radius {
    pub sm: u16,
    pub md: u16,
    pub lg: u16,
    pub pill: u16,
}
impl Default for Radius {
    fn default() -> Self {
        Self { sm: 6, md: 12, lg: 20, pill: 999 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Motion {
    pub fast_ms: u16,
    pub base_ms: u16,
    pub slow_ms: u16,
    pub easing: String,
}
impl Default for Motion {
    fn default() -> Self {
        Self { fast_ms: 140, base_ms: 240, slow_ms: 420, easing: "cubic-bezier(0.22, 1, 0.36, 1)".into() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LogoRules {
    pub primary_asset: Option<String>,
    pub mono_asset: Option<String>,
    pub mark_asset: Option<String>,
    /// Clear space as a fraction of logo height.
    pub clearspace: f32,
    pub min_px: u16,
    pub dont: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageryRules {
    /// Positive style words: "natural light", "warm grade", "editorial".
    pub style: Vec<String>,
    /// Hard bans: "stock-photo handshakes", "purple gradients", "text in image".
    pub avoid: Vec<String>,
    pub icon_style: String,
    pub texture_style: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Voice {
    pub traits: Vec<String>,
    pub reading_level: String,
    pub banned_phrases: Vec<String>,
    pub sample: String,
}

impl Default for BrandStandards {
    /// The out-of-the-box Juggernaught look: calm neutrals, one confident
    /// accent, Plus Jakarta Sans throughout. Replaced by the setup wizard.
    fn default() -> Self {
        let jakarta = |w: Vec<u16>| FontToken {
            family: "Plus Jakarta Sans".into(),
            weights: w,
            fallback: "ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif".into(),
        };
        Self {
            schema_version: SCHEMA_VERSION,
            name: "Juggernaught".into(),
            tagline: None,
            colors: vec![
                ColorToken { slug: "ink".into(), hex: "#1b1a17".into(), usage: "body text, icons".into() },
                ColorToken { slug: "ink-soft".into(), hex: "#5e5a52".into(), usage: "secondary text".into() },
                ColorToken { slug: "surface".into(), hex: "#faf8f4".into(), usage: "page background".into() },
                ColorToken { slug: "surface-raised".into(), hex: "#ffffff".into(), usage: "cards, composer".into() },
                ColorToken { slug: "line".into(), hex: "#e7e2d8".into(), usage: "borders, dividers".into() },
                ColorToken { slug: "primary".into(), hex: "#c2502a".into(), usage: "primary actions, focus".into() },
                ColorToken { slug: "primary-ink".into(), hex: "#ffffff".into(), usage: "text on primary".into() },
            ],
            typography: Typography { heading: jakarta(vec![600, 700]), body: jakarta(vec![400, 500]), mono: None, scale_ratio: 1.2, base_px: 16.0 },
            spacing: Spacing::default(),
            radius: Radius::default(),
            motion: Motion::default(),
            logo: LogoRules { clearspace: 0.25, min_px: 24, ..Default::default() },
            imagery: ImageryRules::default(),
            voice: Voice { traits: vec!["clear".into(), "warm".into(), "direct".into()], reading_level: "grade 8".into(), ..Default::default() },
        }
    }
}

impl BrandStandards {
    pub fn color(&self, slug: &str) -> Option<&str> {
        self.colors.iter().find(|c| c.slug == slug).map(|c| c.hex.as_str())
    }

    pub fn font_families(&self) -> Vec<&str> {
        let mut v = vec![self.typography.heading.family.as_str(), self.typography.body.family.as_str()];
        if let Some(m) = &self.typography.mono {
            v.push(m.family.as_str());
        }
        v.dedup();
        v
    }

    /// CSS custom properties: the *only* values generated CSS may reference.
    pub fn to_css(&self) -> String {
        let mut s = String::from(":root{");
        for c in &self.colors {
            s.push_str(&format!("--color-{}:{};", c.slug, c.hex));
        }
        let t = &self.typography;
        s.push_str(&format!("--font-heading:'{}',{};", t.heading.family, t.heading.fallback));
        s.push_str(&format!("--font-body:'{}',{};", t.body.family, t.body.fallback));
        for step in -2i32..=6 {
            let px = t.base_px * t.scale_ratio.powi(step);
            let name = if step < 0 { format!("n{}", -step) } else { step.to_string() };
            s.push_str(&format!("--text-{name}:{:.3}rem;", px / 16.0));
        }
        for (i, v) in self.spacing.scale.iter().enumerate() {
            s.push_str(&format!("--space-{i}:{}px;", v));
        }
        let r = &self.radius;
        s.push_str(&format!("--radius-sm:{}px;--radius-md:{}px;--radius-lg:{}px;--radius-pill:{}px;", r.sm, r.md, r.lg, r.pill));
        let m = &self.motion;
        s.push_str(&format!("--motion-fast:{}ms;--motion-base:{}ms;--motion-slow:{}ms;--ease:{};", m.fast_ms, m.base_ms, m.slow_ms, m.easing));
        s.push('}');
        s
    }

    /// DESIGN.md for open-design (its brand-system format) and as agent context.
    pub fn to_design_md(&self) -> String {
        let mut s = format!("# {} — Design System\n\n", self.name);
        if let Some(t) = &self.tagline {
            s.push_str(&format!("> {t}\n\n"));
        }
        s.push_str("## Color\n\n| Token | Hex | Usage |\n|---|---|---|\n");
        for c in &self.colors {
            s.push_str(&format!("| `{}` | `{}` | {} |\n", c.slug, c.hex, c.usage));
        }
        let t = &self.typography;
        s.push_str(&format!(
            "\n## Typography\n\n- Headings: **{}** ({:?})\n- Body: **{}** ({:?})\n- Scale: {}px base × {}\n",
            t.heading.family, t.heading.weights, t.body.family, t.body.weights, t.base_px, t.scale_ratio
        ));
        s.push_str(&format!(
            "\n## Shape & motion\n\n- Radius: sm {} / md {} / lg {}\n- Motion: {}ms / {}ms / {}ms, easing `{}`; always honour prefers-reduced-motion\n",
            self.radius.sm, self.radius.md, self.radius.lg, self.motion.fast_ms, self.motion.base_ms, self.motion.slow_ms, self.motion.easing
        ));
        s.push_str(&format!(
            "\n## Logo\n\n- Clear space: {}× logo height\n- Minimum size: {}px\n- Never: {}\n",
            self.logo.clearspace,
            self.logo.min_px,
            if self.logo.dont.is_empty() { "stretch, recolour, add effects".into() } else { self.logo.dont.join(", ") }
        ));
        s.push_str(&format!(
            "\n## Imagery\n\n- Style: {}\n- Avoid: {}\n- Icons: {}\n- Textures: {}\n",
            self.imagery.style.join(", "),
            self.imagery.avoid.join(", "),
            self.imagery.icon_style,
            self.imagery.texture_style
        ));
        s.push_str(&format!(
            "\n## Voice\n\n- Traits: {}\n- Reading level: {}\n- Never say: {}\n",
            self.voice.traits.join(", "),
            self.voice.reading_level,
            self.voice.banned_phrases.join(", ")
        ));
        s
    }

    /// Prepended to every image-generation prompt (logos, textures, icons, heroes).
    pub fn image_prompt_preamble(&self) -> String {
        let palette: Vec<String> = self.colors.iter().map(|c| format!("{} {}", c.slug, c.hex)).collect();
        format!(
            "Brand: {}. Palette (use only these): {}. Style: {}. Avoid: {}. No text unless asked.",
            self.name,
            palette.join(", "),
            if self.imagery.style.is_empty() { "clean, considered, modern".into() } else { self.imagery.style.join(", ") },
            if self.imagery.avoid.is_empty() { "generic stock look, clutter".into() } else { self.imagery.avoid.join(", ") },
        )
    }

    /// Structural checks run on save (wizard, chat edits, imports).
    pub fn validate(&self) -> Vec<Finding> {
        let mut f = Vec::new();
        let hex = regex::Regex::new(r"^#[0-9a-fA-F]{6}$").unwrap();
        for c in &self.colors {
            if !hex.is_match(&c.hex) {
                f.push(Finding::error("brand.color.format", format!("{} is not #rrggbb: {}", c.slug, c.hex)));
            }
        }
        for required in ["ink", "surface", "primary"] {
            if self.color(required).is_none() {
                f.push(Finding::error("brand.color.required", format!("missing required colour token `{required}`")));
            }
        }
        if let (Some(ink), Some(surface)) = (self.color("ink"), self.color("surface")) {
            if let Some(r) = contrast_ratio(ink, surface) {
                if r < 4.5 {
                    f.push(Finding::error("a11y.contrast.body", format!("ink on surface is {r:.2}:1, needs ≥ 4.5:1 (WCAG AA)")));
                }
            }
        }
        if let (Some(p), Some(pi)) = (self.color("primary"), self.color("primary-ink")) {
            if let Some(r) = contrast_ratio(pi, p) {
                if r < 4.5 {
                    f.push(Finding::warn("a11y.contrast.primary", format!("text on primary is {r:.2}:1, needs ≥ 4.5:1 for button text")));
                }
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_brand_is_valid_and_accessible() {
        let b = BrandStandards::default();
        let findings = b.validate();
        assert!(findings.iter().all(|f| f.severity != Severity::Error), "{findings:?}");
        assert!(b.to_css().contains("--font-body:'Plus Jakarta Sans'"));
        assert!(b.to_design_md().contains("| `primary` |"));
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<BrandStandards>(&json).unwrap(), b);
    }
}
