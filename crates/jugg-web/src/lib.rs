//! How Juggernaught builds websites.
//!
//! Sites are **data, not code**: a [`Page`] is a list of typed [`Block`]s.
//! The AI edits blocks; it never writes raw HTML, CSS or JavaScript. The
//! renderer turns blocks into static HTML whose only styling comes from brand
//! tokens and whose only script is none. That is what keeps every site secure,
//! accessible and on-brand by construction. See `docs/WEB_STANDARDS.md`.

mod render;
mod validate;

pub use render::{render_page, site_css, Csp, SECURITY_HEADERS};
pub use validate::{validate_page, Override, Report, Rule, Violation, RULES};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub slug: String,
    pub title: String,
    /// ≤ 160 chars; used for meta description and social cards.
    pub description: String,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub lang: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Hero { eyebrow: Option<String>, heading: String, body: Option<String>, cta: Option<Link>, secondary: Option<Link>, image: Option<Image>, tone: Tone },
    Section { eyebrow: Option<String>, heading: String, body: String, tone: Tone },
    Cards { eyebrow: Option<String>, heading: String, cards: Vec<Card>, tone: Tone },
    Gallery { heading: Option<String>, images: Vec<Image>, tone: Tone },
    Cta { heading: String, body: Option<String>, cta: Link, tone: Tone },
    Contact { heading: String, form: Form, tone: Tone },
    /// Escape hatch: owner-approved raw HTML. Always produces a warning-level
    /// override record and is sanitized at render (no script, no handlers).
    RawHtml { html: String, tone: Tone },
}

impl Block {
    pub fn tone(&self) -> Tone {
        match self {
            Block::Hero { tone, .. }
            | Block::Section { tone, .. }
            | Block::Cards { tone, .. }
            | Block::Gallery { tone, .. }
            | Block::Cta { tone, .. }
            | Block::Contact { tone, .. }
            | Block::RawHtml { tone, .. } => *tone,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Block::Hero { .. } => "hero",
            Block::Section { .. } => "section",
            Block::Cards { .. } => "cards",
            Block::Gallery { .. } => "gallery",
            Block::Cta { .. } => "cta",
            Block::Contact { .. } => "contact",
            Block::RawHtml { .. } => "raw_html",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Light,
    Dark,
    Accent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub href: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    /// Asset id or site-relative path. External hotlinks are not allowed.
    pub src: String,
    pub alt: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub heading: String,
    pub body: String,
    pub link: Option<Link>,
    pub image: Option<Image>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Form {
    pub name: String,
    pub fields: Vec<Field>,
    pub submit_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Email,
    Tel,
    Textarea,
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
