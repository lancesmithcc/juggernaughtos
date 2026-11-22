//! The publish gate. Every rule has an id, a severity and whether the owner
//! may override it. Overrides are allowed (the owner is in charge) but are
//! always warned about and recorded in `standards_overrides` + the audit log.

use crate::{Block, Page, Tone};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Blocks publishing unless overridden.
    Error,
    /// Shown, never blocks.
    Warn,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Rule {
    pub id: &'static str,
    pub level: Level,
    /// `false` = cannot be overridden from chat; needs Settings → Advanced
    /// with typed confirmation (security-critical rules).
    pub chat_overridable: bool,
    pub why: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule { id: "seo.title", level: Level::Error, chat_overridable: true, why: "Pages need a 10–60 character title for search and tabs." },
    Rule { id: "seo.description", level: Level::Warn, chat_overridable: true, why: "A 50–160 character description improves search snippets." },
    Rule { id: "a11y.single_h1", level: Level::Error, chat_overridable: true, why: "Exactly one hero heading (h1) per page keeps screen readers oriented." },
    Rule { id: "a11y.alt_text", level: Level::Error, chat_overridable: true, why: "Every image needs alt text (WCAG 2.2 1.1.1)." },
    Rule { id: "a11y.link_text", level: Level::Error, chat_overridable: true, why: "Link labels must describe the destination, not 'click here'." },
    Rule { id: "design.tone_rhythm", level: Level::Warn, chat_overridable: true, why: "Adjacent sections should alternate tone for visual rhythm." },
    Rule { id: "design.single_primary_cta", level: Level::Warn, chat_overridable: true, why: "One primary call to action per view converts better." },
    Rule { id: "perf.image_weight", level: Level::Error, chat_overridable: true, why: "Images over 300 KB slow the page; the pipeline re-encodes them." },
    Rule { id: "perf.page_weight", level: Level::Error, chat_overridable: true, why: "Total page weight budget is 1 MB." },
    Rule { id: "sec.external_resource", level: Level::Error, chat_overridable: false, why: "Hotlinked images/scripts leak visitor data and break CSP." },
    Rule { id: "sec.link_scheme", level: Level::Error, chat_overridable: false, why: "Only https:, mailto:, tel: and site-relative links are allowed." },
    Rule { id: "sec.raw_html", level: Level::Warn, chat_overridable: true, why: "Raw HTML bypasses the block system; it is sanitized and logged." },
    Rule { id: "site.internal_link", level: Level::Warn, chat_overridable: true, why: "Links to pages that don't exist frustrate visitors and hurt search ranking." },
    Rule { id: "site.unapproved_media", level: Level::Error, chat_overridable: true, why: "Only images you approved in Assets go on the public site." },
];

pub(crate) fn violation(id: &str, message: String, overrides: &[Override]) -> Violation {
    let r = rule(id);
    Violation { rule: id.into(), level: r.level, message, overridden: r.chat_overridable && overrides.iter().any(|o| o.rule == id) }
}

pub fn rule_level(id: &str) -> Level {
    rule(id).level
}

pub fn rule(id: &str) -> &'static Rule {
    RULES.iter().find(|r| r.id == id).expect("unknown rule id")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Violation {
    pub rule: String,
    pub level: Level,
    pub message: String,
    pub overridden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Override {
    pub rule: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub violations: Vec<Violation>,
}

impl Report {
    /// True when nothing blocks publishing.
    pub fn publishable(&self) -> bool {
        !self.violations.iter().any(|v| v.level == Level::Error && !v.overridden)
    }
    pub fn warnings(&self) -> impl Iterator<Item = &Violation> {
        self.violations.iter().filter(|v| v.level == Level::Warn || v.overridden)
    }
}

const IMAGE_BUDGET: u64 = 300 * 1024;
const PAGE_BUDGET: u64 = 1024 * 1024;

pub fn validate_page(page: &Page, overrides: &[Override]) -> Report {
    let mut v: Vec<(String, String)> = Vec::new();
    let mut push = |id: &str, msg: String| v.push((id.to_string(), msg));

    let tl = page.title.chars().count();
    if !(10..=60).contains(&tl) {
        push("seo.title", format!("title is {tl} characters"));
    }
    let dl = page.description.chars().count();
    if !(50..=160).contains(&dl) {
        push("seo.description", format!("description is {dl} characters"));
    }

    let heroes = page.blocks.iter().filter(|b| matches!(b, Block::Hero { .. })).count();
    if heroes != 1 {
        push("a11y.single_h1", format!("page has {heroes} hero blocks; needs exactly 1"));
    }

    let mut images = Vec::new();
    let mut links = Vec::new();
    let mut primary_ctas = 0;
    for b in &page.blocks {
        match b {
            Block::Hero { cta, secondary, image, .. } => {
                images.extend(image.iter());
                links.extend(cta.iter().chain(secondary.iter()));
                primary_ctas += cta.is_some() as usize;
            }
            Block::Cards { cards, .. } => {
                for c in cards {
                    images.extend(c.image.iter());
                    links.extend(c.link.iter());
                }
            }
            Block::Gallery { images: imgs, .. } => images.extend(imgs.iter()),
            Block::Cta { cta, .. } => {
                links.push(cta);
                primary_ctas += 1;
            }
            Block::RawHtml { .. } => push("sec.raw_html", "page contains a raw HTML block".into()),
            _ => {}
        }
    }
    if primary_ctas > 2 {
        push("design.single_primary_cta", format!("{primary_ctas} primary calls to action"));
    }

    let mut weight = 0u64;
    for img in &images {
        if img.alt.trim().is_empty() {
            push("a11y.alt_text", format!("image {} has no alt text", img.src));
        }
        if img.src.starts_with("http://") || img.src.starts_with("https://") || img.src.starts_with("//") {
            push("sec.external_resource", format!("image {} is hotlinked", img.src));
        }
        if let Some(b) = img.bytes {
            weight += b;
            if b > IMAGE_BUDGET {
                push("perf.image_weight", format!("{} is {} KB", img.src, b / 1024));
            }
        }
    }
    if weight > PAGE_BUDGET {
        push("perf.page_weight", format!("images total {} KB", weight / 1024));
    }

    for l in &links {
        let lower = l.label.trim().to_lowercase();
        if ["click here", "here", "read more", "more", "link"].contains(&lower.as_str()) {
            push("a11y.link_text", format!("link label \"{}\"", l.label));
        }
        let href = l.href.trim().to_lowercase();
        let ok = href.starts_with("https://") || href.starts_with("mailto:") || href.starts_with("tel:") || (href.starts_with('/') && !href.starts_with("//")) || href.starts_with('#');
        if !ok {
            push("sec.link_scheme", format!("link {} uses a disallowed scheme", l.href));
        }
    }

    for w in page.blocks.windows(2) {
        if w[0].tone() == w[1].tone() && w[0].tone() != Tone::Accent {
            push("design.tone_rhythm", format!("{} and {} share the same tone", w[0].kind(), w[1].kind()));
        }
    }

    let violations = v
        .into_iter()
        .map(|(id, message)| {
            let r = rule(&id);
            let overridden = r.chat_overridable && overrides.iter().any(|o| o.rule == id);
            Violation { rule: id, level: r.level, message, overridden }
        })
        .collect();
    Report { violations }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Image, Link};

    fn page() -> Page {
        Page {
            slug: "/".into(),
            title: "Acme Bakery — Fresh bread daily".into(),
            description: "Small-batch sourdough and pastries baked every morning in downtown Nanaimo.".into(),
            lang: None,
            nav: None,
            blocks: vec![
                Block::Hero {
                    eyebrow: Some("Since 1998".into()),
                    heading: "Bread worth waking up for".into(),
                    body: None,
                    cta: Some(Link { label: "See today's menu".into(), href: "/menu".into() }),
                    secondary: None,
                    image: Some(Image { src: "/media/hero.webp".into(), alt: "Loaves cooling on a rack".into(), width: 1600, height: 900, bytes: Some(180_000) }),
                    tone: Tone::Dark,
                },
                Block::Section { eyebrow: None, heading: "Our story".into(), body: "…".into(), tone: Tone::Light },
            ],
        }
    }

    #[test]
    fn good_page_is_publishable() {
        let r = validate_page(&page(), &[]);
        assert!(r.publishable(), "{:?}", r.violations);
    }

    #[test]
    fn security_rules_cannot_be_overridden_from_chat() {
        let mut p = page();
        if let Block::Hero { image: Some(img), cta: Some(cta), .. } = &mut p.blocks[0] {
            img.src = "https://evil.example/x.png".into();
            cta.label = "click here".into();
        }
        let ov = vec![
            Override { rule: "sec.external_resource".into(), reason: "owner insisted".into() },
            Override { rule: "a11y.link_text".into(), reason: "brand voice".into() },
        ];
        let r = validate_page(&p, &ov);
        assert!(!r.publishable(), "hotlink must still block");
        assert!(r.violations.iter().any(|v| v.rule == "a11y.link_text" && v.overridden));
    }
}
