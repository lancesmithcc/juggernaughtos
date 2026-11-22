//! Block → static HTML. No inline scripts, no inline styles, no external
//! origins: the CSP below is the whole contract.

use crate::{esc, Block, Card, Field, FieldKind, Image, Link, Page, Tone};
use jugg_brand::BrandStandards;

pub struct Csp;

impl Csp {
    /// Public-site policy. Nothing outside the site's own origin, ever.
    /// `connect-src 'self'` only lets same-origin fetches (crawlers' robots.txt
    /// checks) through; the page itself has no scripts.
    pub const PUBLIC: &'static str = "default-src 'none'; img-src 'self' data:; style-src 'self'; font-src 'self'; connect-src 'self'; \
        form-action 'self'; base-uri 'none'; frame-ancestors 'none'; upgrade-insecure-requests";
}

pub const SECURITY_HEADERS: &[(&str, &str)] = &[
    ("strict-transport-security", "max-age=63072000; includeSubDomains; preload"),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    ("permissions-policy", "camera=(), microphone=(), geolocation=(), interest-cohort=()"),
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-resource-policy", "same-origin"),
];

/// Site stylesheet: brand tokens + the fixed component layer (tokens only).
pub fn site_css(brand: &BrandStandards) -> String {
    let mut css = brand.to_css();
    css.push_str(include_str!("site.css"));
    css
}

pub fn render_page(page: &Page, brand: &BrandStandards, css_href: &str) -> String {
    crate::site::render_page_with(page, brand, &crate::site::SiteContext { css_href: css_href.into(), ..Default::default() })
}

pub(crate) fn body_html(page: &Page) -> String {
    let mut body = String::new();
    for b in &page.blocks {
        body.push_str(&block(b));
    }
    body
}

#[allow(dead_code)]
fn render_page_legacy(page: &Page, brand: &BrandStandards, css_href: &str) -> String {
    let body = body_html(page);
    format!(
        "<!doctype html><html lang=\"{lang}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><meta name=\"description\" content=\"{desc}\">\
         <meta property=\"og:title\" content=\"{title}\"><meta property=\"og:description\" content=\"{desc}\">\
         <link rel=\"stylesheet\" href=\"{css}\"></head>\
         <body><a class=\"skip\" href=\"#main\">Skip to content</a>\
         <header class=\"site-header\"><a class=\"brand\" href=\"/\">{name}</a></header>\
         <main id=\"main\">{body}</main>\
         <footer class=\"site-footer\"><p>© {name}</p></footer></body></html>",
        lang = esc(page.lang.as_deref().unwrap_or("en")),
        title = esc(&page.title),
        desc = esc(&page.description),
        css = esc(css_href),
        name = esc(&brand.name),
    )
}

fn tone(t: Tone) -> &'static str {
    match t {
        Tone::Light => "tone-light",
        Tone::Dark => "tone-dark",
        Tone::Accent => "tone-accent",
    }
}

fn eyebrow(e: &Option<String>) -> String {
    e.as_ref().map(|e| format!("<p class=\"eyebrow\">{}</p>", esc(e))).unwrap_or_default()
}

fn link(l: &Link, class: &str) -> String {
    format!("<a class=\"{class}\" href=\"{}\">{}</a>", esc(&l.href), esc(&l.label))
}

fn img(i: &Image, eager: bool) -> String {
    format!(
        "<img src=\"{}\" alt=\"{}\" width=\"{}\" height=\"{}\" decoding=\"async\" loading=\"{}\"{}>",
        esc(&i.src),
        esc(&i.alt),
        i.width,
        i.height,
        if eager { "eager" } else { "lazy" },
        if eager { " fetchpriority=\"high\"" } else { "" }
    )
}

fn paragraphs(s: &str) -> String {
    s.split("\n\n").filter(|p| !p.trim().is_empty()).map(|p| format!("<p>{}</p>", esc(p.trim()))).collect()
}

fn card(c: &Card) -> String {
    format!(
        "<article class=\"card\">{}<h3>{}</h3>{}{}</article>",
        c.image.as_ref().map(|i| img(i, false)).unwrap_or_default(),
        esc(&c.heading),
        paragraphs(&c.body),
        c.link.as_ref().map(|l| link(l, "card-link")).unwrap_or_default()
    )
}

fn field(f: &Field) -> String {
    let req = if f.required { " required" } else { "" };
    let id = format!("f-{}", esc(&f.name));
    let input = match f.kind {
        FieldKind::Textarea => format!("<textarea id=\"{id}\" name=\"{}\" rows=\"5\"{req}></textarea>", esc(&f.name)),
        k => {
            let ty = match k {
                FieldKind::Email => "email",
                FieldKind::Tel => "tel",
                _ => "text",
            };
            format!("<input id=\"{id}\" name=\"{}\" type=\"{ty}\"{req}>", esc(&f.name))
        }
    };
    format!("<div class=\"field\"><label for=\"{id}\">{}</label>{input}</div>", esc(&f.label))
}

fn block(b: &Block) -> String {
    match b {
        Block::Hero { eyebrow: e, heading, body, cta, secondary, image, tone: t } => format!(
            "<section class=\"block hero {}\"><div class=\"inner\">{}<h1>{}</h1>{}<div class=\"actions\">{}{}</div></div>{}</section>",
            tone(*t),
            eyebrow(e),
            esc(heading),
            body.as_deref().map(paragraphs).unwrap_or_default(),
            cta.as_ref().map(|l| link(l, "btn btn-primary")).unwrap_or_default(),
            secondary.as_ref().map(|l| link(l, "btn btn-ghost")).unwrap_or_default(),
            image.as_ref().map(|i| format!("<figure class=\"hero-media\">{}</figure>", img(i, true))).unwrap_or_default(),
        ),
        Block::Section { eyebrow: e, heading, body, tone: t } => {
            format!("<section class=\"block section {}\"><div class=\"inner\">{}<h2>{}</h2>{}</div></section>", tone(*t), eyebrow(e), esc(heading), paragraphs(body))
        }
        Block::Cards { eyebrow: e, heading, cards, tone: t } => format!(
            "<section class=\"block cards {}\"><div class=\"inner\">{}<h2>{}</h2><div class=\"grid\">{}</div></div></section>",
            tone(*t),
            eyebrow(e),
            esc(heading),
            cards.iter().map(card).collect::<String>()
        ),
        Block::Gallery { heading, images, tone: t } => format!(
            "<section class=\"block gallery {}\"><div class=\"inner\">{}<div class=\"grid\">{}</div></div></section>",
            tone(*t),
            heading.as_ref().map(|h| format!("<h2>{}</h2>", esc(h))).unwrap_or_default(),
            images.iter().map(|i| format!("<figure>{}</figure>", img(i, false))).collect::<String>()
        ),
        Block::Cta { heading, body, cta, tone: t } => format!(
            "<section class=\"block cta {}\"><div class=\"inner\"><h2>{}</h2>{}{}</div></section>",
            tone(*t),
            esc(heading),
            body.as_deref().map(paragraphs).unwrap_or_default(),
            link(cta, "btn btn-primary")
        ),
        Block::Contact { heading, form, tone: t } => format!(
            "<section class=\"block contact {}\"><div class=\"inner\"><h2>{}</h2>\
             <form method=\"post\" action=\"/forms/{}\">{}<div class=\"hp\" aria-hidden=\"true\"><label>Leave empty<input name=\"_hp\" tabindex=\"-1\" autocomplete=\"off\"></label></div>\
             <button class=\"btn btn-primary\" type=\"submit\">{}</button></form></div></section>",
            tone(*t),
            esc(heading),
            esc(&form.name),
            form.fields.iter().map(field).collect::<String>(),
            esc(&form.submit_label)
        ),
        Block::RawHtml { html, tone: t } => format!("<section class=\"block raw {}\"><div class=\"inner\">{}</div></section>", tone(*t), sanitize(html)),
    }
}

/// Conservative sanitizer for owner-approved raw HTML: strips script/style/
/// iframe/object elements, event-handler attributes and javascript: URLs.
/// (Phase 3 swaps this for an allowlist parser — see docs/WEB_STANDARDS.md.)
fn sanitize(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "iframe", "object", "embed", "link", "meta", "base"] {
        loop {
            let lower = s.to_lowercase();
            let Some(start) = lower.find(&format!("<{tag}")) else { break };
            let end = lower[start..].find(&format!("</{tag}>")).map(|e| start + e + tag.len() + 3).or_else(|| lower[start..].find('>').map(|e| start + e + 1)).unwrap_or(s.len());
            s.replace_range(start..end, "");
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.to_lowercase().find(" on") {
        let after = &rest[i + 3..];
        let is_handler = after.chars().take_while(|c| c.is_ascii_alphabetic()).count() > 0 && after.trim_start_matches(|c: char| c.is_ascii_alphabetic()).trim_start().starts_with('=');
        out.push_str(&rest[..i]);
        if is_handler {
            let attr_end = after.find(|c| c == '>' || c == ' ').unwrap_or(after.len());
            rest = &after[attr_end..];
        } else {
            out.push_str(" on");
            rest = after;
        }
    }
    out.push_str(rest);
    out.replace("javascript:", "").replace("JAVASCRIPT:", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_escaped_and_scriptless() {
        let page = Page {
            slug: "/".into(),
            title: "<Acme>".into(),
            description: "d".into(),
            lang: None,
            nav: None,
            blocks: vec![
                Block::Hero { eyebrow: None, heading: "Hi \"there\"".into(), body: None, cta: None, secondary: None, image: None, tone: Tone::Dark },
                Block::RawHtml { html: "<p onclick=\"x()\">ok</p><script>alert(1)</script><a href=\"javascript:alert(1)\">x</a>".into(), tone: Tone::Light },
            ],
        };
        let html = render_page(&page, &BrandStandards::default(), "/site.css");
        assert!(html.contains("&lt;Acme&gt;"));
        assert!(html.contains("Hi &quot;there&quot;"));
        assert!(!html.to_lowercase().contains("<script"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("javascript:"));
        let css = site_css(&BrandStandards::default());
        assert!(jugg_brand::lint_css(&BrandStandards::default(), &css).is_empty(), "site.css must pass its own brand gate");
    }
}
