//! Whole-site build: every page plus the files a real site needs.
//!
//! Output is a flat list of `(path, bytes)` written into an immutable release
//! directory by the caller. CSS is content-hashed so it can be cached forever;
//! HTML is small and revalidated.

use crate::render::body_html;
use crate::validate::{validate_page, violation, Override, Report};
use crate::{esc, Block, Form, Page};
use jugg_brand::BrandStandards;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct SiteContext {
    /// Menu: (label, href).
    pub nav: Vec<(String, String)>,
    pub css_href: String,
    /// Absolute site URL (`https://example.com`) for canonical + sitemap.
    pub base_url: Option<String>,
    /// Font file to preload (largest-contentful text).
    pub preload_font: Option<String>,
    /// Prefix for every site-internal URL (draft previews live under `/_preview`).
    pub prefix: String,
    /// Mark the page as not for search engines (previews, thank-you, 404).
    pub noindex: bool,
}

#[derive(Debug, Clone)]
pub struct FontFace {
    pub family: String,
    /// Public path, e.g. `/fonts/PlusJakartaSans-Variable.ttf`.
    pub url: String,
    /// `"200 800"` for variable fonts, `"400"` for statics.
    pub weight: String,
    pub format: String,
}

#[derive(Debug, Default)]
pub struct Built {
    pub files: Vec<(String, Vec<u8>)>,
    pub reports: Vec<(String, Report)>,
    pub forms: Vec<Form>,
}

impl Built {
    pub fn publishable(&self) -> bool {
        self.reports.iter().all(|(_, r)| r.publishable())
    }
}

#[derive(Debug)]
pub enum BuildError {
    BadSlug(String),
    DuplicateSlug(String),
    NoHomepage,
}

impl std::error::Error for BuildError {}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::BadSlug(s) => write!(f, "page address “{s}” must look like /about or /services/web (lowercase letters, numbers, dashes)"),
            BuildError::DuplicateSlug(s) => write!(f, "two pages use the address {s}"),
            BuildError::NoHomepage => write!(f, "the site needs a homepage (address /)"),
        }
    }
}

pub fn valid_slug(s: &str) -> bool {
    s == "/"
        || (s.starts_with('/')
            && !s.ends_with('/')
            && s.len() <= 80
            && s[1..].split('/').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')))
}

/// File path inside the release for a page slug.
pub fn page_path(slug: &str) -> String {
    if slug == "/" {
        "index.html".into()
    } else {
        format!("{}/index.html", slug.trim_start_matches('/'))
    }
}

fn fnv(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")[..10].to_string()
}

pub fn render_page_with(page: &Page, brand: &BrandStandards, ctx: &SiteContext) -> String {
    let nav: String = ctx
        .nav
        .iter()
        .map(|(label, href)| {
            let current = if *href == page.slug { " aria-current=\"page\"" } else { "" };
            format!("<a href=\"{}\"{current}>{}</a>", esc(href), esc(label))
        })
        .collect();
    let canonical = ctx.base_url.as_ref().map(|b| format!("<link rel=\"canonical\" href=\"{}{}\"><meta property=\"og:url\" content=\"{}{}\">", esc(b), esc(if page.slug == "/" { "/" } else { &page.slug }), esc(b), esc(&page.slug))).unwrap_or_default();
    let preload = ctx.preload_font.as_ref().map(|f| format!("<link rel=\"preload\" href=\"{}\" as=\"font\" type=\"font/ttf\" crossorigin>", esc(f))).unwrap_or_default();
    let robots = if ctx.noindex { "<meta name=\"robots\" content=\"noindex\">" } else { "" };
    let html = format!(
        "<!doctype html><html lang=\"{lang}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><meta name=\"description\" content=\"{desc}\">{robots}\
         <meta property=\"og:type\" content=\"website\"><meta property=\"og:title\" content=\"{title}\"><meta property=\"og:description\" content=\"{desc}\">\
         <meta name=\"theme-color\" content=\"{theme}\"><link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\">{canonical}{preload}\
         <link rel=\"stylesheet\" href=\"{css}\"></head>\
         <body><a class=\"skip\" href=\"#main\">Skip to content</a>\
         <header class=\"site-header\"><a class=\"brand\" href=\"/\">{name}</a>{nav_html}</header>\
         <main id=\"main\">{body}</main>\
         <footer class=\"site-footer\"><p>© {name}</p></footer></body></html>",
        lang = esc(page.lang.as_deref().unwrap_or("en")),
        title = esc(&page.title),
        desc = esc(&page.description),
        theme = esc(brand.color("ink").unwrap_or("#1b1a17")),
        css = esc(&ctx.css_href),
        name = esc(&brand.name),
        nav_html = if nav.is_empty() { String::new() } else { format!("<nav class=\"site-nav\" aria-label=\"Main\">{nav}</nav>") },
        body = body_html(page),
    );
    if ctx.prefix.is_empty() {
        html
    } else {
        // Previews: every site-internal URL moves under the prefix.
        let p = &ctx.prefix;
        html.replace("href=\"/", &format!("href=\"{p}/")).replace("src=\"/", &format!("src=\"{p}/")).replace("action=\"/", &format!("action=\"{p}/"))
    }
}

/// Brand tokens + @font-face for self-hosted fonts + the component layer.
pub fn site_css_with_fonts(brand: &BrandStandards, faces: &[FontFace]) -> String {
    let mut css = String::new();
    for f in faces {
        css.push_str(&format!(
            "@font-face{{font-family:'{}';src:url('{}') format('{}');font-weight:{};font-display:swap}}",
            f.family.replace('\'', ""),
            f.url,
            f.format,
            f.weight
        ));
    }
    css.push_str(&crate::site_css(brand));
    css
}

/// Build the whole site. `overrides` maps slug → accepted overrides ("*" applies to all pages).
pub fn build_site(pages: &[Page], brand: &BrandStandards, faces: &[FontFace], overrides: &HashMap<String, Vec<Override>>, base_url: Option<&str>) -> Result<Built, BuildError> {
    let mut seen = std::collections::HashSet::new();
    for p in pages {
        if !valid_slug(&p.slug) {
            return Err(BuildError::BadSlug(p.slug.clone()));
        }
        if !seen.insert(p.slug.clone()) {
            return Err(BuildError::DuplicateSlug(p.slug.clone()));
        }
    }
    if !seen.contains("/") {
        return Err(BuildError::NoHomepage);
    }

    let css = site_css_with_fonts(brand, faces);
    let css_name = format!("site.{}.css", fnv(css.as_bytes()));
    let mut nav: Vec<(String, String)> = pages.iter().filter_map(|p| p.nav.clone().map(|l| (l, p.slug.clone()))).collect();
    if !nav.is_empty() && !nav.iter().any(|(_, h)| h == "/") {
        nav.insert(0, ("Home".into(), "/".into()));
    }
    let ctx = SiteContext {
        nav,
        css_href: format!("/{css_name}"),
        base_url: base_url.map(|b| b.trim_end_matches('/').to_string()),
        preload_font: faces.first().map(|f| f.url.clone()),
        prefix: String::new(),
        noindex: false,
    };

    let mut built = Built::default();
    for p in pages {
        let mut ov = overrides.get(&p.slug).cloned().unwrap_or_default();
        ov.extend(overrides.get("*").cloned().unwrap_or_default());
        let mut report = validate_page(p, &ov);
        for href in internal_links(p) {
            let target = href.split(['#', '?']).next().unwrap_or("").trim_end_matches('/');
            let target = if target.is_empty() { "/" } else { target };
            if !seen.contains(target) && !target.starts_with("/forms/") {
                report.violations.push(violation("site.internal_link", format!("link to {href}, but there is no such page"), &ov));
            }
        }
        for b in &p.blocks {
            if let Block::Contact { form, .. } = b {
                built.forms.push(form.clone());
            }
        }
        built.files.push((page_path(&p.slug), render_page_with(p, brand, &ctx).into_bytes()));
        built.reports.push((p.slug.clone(), report));
    }

    let utility = |slug: &str, title: &str, heading: &str, body: &str| Page {
        slug: slug.into(),
        title: format!("{title} — {}", brand.name),
        description: body.into(),
        lang: None,
        nav: None,
        blocks: vec![Block::Hero { eyebrow: None, heading: heading.into(), body: Some(body.into()), cta: Some(crate::Link { label: "Back to the homepage".into(), href: "/".into() }), secondary: None, image: None, tone: crate::Tone::Light }],
    };
    let quiet = SiteContext { noindex: true, ..ctx.clone() };
    built.files.push(("404.html".into(), render_page_with(&utility("/404", "Page not found", "We couldn't find that page", "It may have moved. Everything else is still here."), brand, &quiet).into_bytes()));
    built.files.push(("thanks/index.html".into(), render_page_with(&utility("/thanks", "Thank you", "Thanks, we got your message", "We'll get back to you soon."), brand, &quiet).into_bytes()));
    built.files.push((css_name, css.into_bytes()));

    let mut robots = format!("User-agent: *\nAllow: /\n{}", crate_robots_bait());
    if let Some(b) = &ctx.base_url {
        robots.push_str(&format!("Sitemap: {b}/sitemap.xml\n"));
        let urls: String = pages.iter().map(|p| format!("<url><loc>{}{}</loc></url>", esc(b), esc(&p.slug))).collect();
        built.files.push(("sitemap.xml".into(), format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{urls}</urlset>").into_bytes()));
    }
    built.files.push(("robots.txt".into(), robots.into_bytes()));
    built.files.push(("favicon.svg".into(), favicon(brand).into_bytes()));
    built.files.push(("forms.json".into(), serde_json::to_vec(&built.forms).unwrap_or_default()));
    Ok(built)
}

/// Brand monogram favicon (replaced by the vectorized logo mark when one exists).
fn favicon(brand: &BrandStandards) -> String {
    let initial: String = brand.name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "•".into());
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\"><rect width=\"64\" height=\"64\" rx=\"16\" fill=\"{bg}\"/><text x=\"32\" y=\"44\" text-anchor=\"middle\" font-family=\"system-ui,sans-serif\" font-weight=\"700\" font-size=\"36\" fill=\"{fg}\">{i}</text></svg>",
        bg = brand.color("primary").unwrap_or("#c2502a"),
        fg = brand.color("primary-ink").unwrap_or("#ffffff"),
        i = esc(&initial)
    )
}

/// Kept here so jugg-web stays independent of the steward crate; must match
/// `jugg_steward::honeytokens::ROBOTS_BAIT`.
fn crate_robots_bait() -> &'static str {
    "Disallow: /internal-admin-7f3a/\n"
}

fn internal_links(p: &Page) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |l: &crate::Link| {
        if l.href.starts_with('/') && !l.href.starts_with("//") {
            out.push(l.href.clone());
        }
    };
    for b in &p.blocks {
        match b {
            Block::Hero { cta, secondary, .. } => {
                cta.iter().for_each(&mut push);
                secondary.iter().for_each(&mut push);
            }
            Block::Cards { cards, .. } => cards.iter().filter_map(|c| c.link.as_ref()).for_each(&mut push),
            Block::Cta { cta, .. } => push(cta),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    proptest::proptest! {
        #[test]
        fn valid_slugs_stay_inside_the_release(s in "/?[a-z0-9./\\-]{0,20}") {
            if super::valid_slug(&s) {
                let p = super::page_path(&s);
                proptest::prop_assert!(!p.starts_with('/') && !p.contains("..") && !p.contains('\\'), "{s} -> {p}");
                proptest::prop_assert!(p.ends_with("index.html"));
            }
        }
    }
    use super::*;
    use crate::{Link, Tone};

    fn page(slug: &str, nav: Option<&str>, cta: &str) -> Page {
        Page {
            slug: slug.into(),
            title: "Harbour Bakery — fresh bread".into(),
            description: "Small-batch sourdough and pastries baked every morning on the waterfront.".into(),
            lang: None,
            nav: nav.map(str::to_string),
            blocks: vec![
                Block::Hero { eyebrow: None, heading: "Bread worth waking up for".into(), body: None, cta: Some(Link { label: "See the menu".into(), href: cta.into() }), secondary: None, image: None, tone: Tone::Dark },
                Block::Section { eyebrow: None, heading: "Our story".into(), body: "Since 1998.".into(), tone: Tone::Light },
            ],
        }
    }

    #[test]
    fn builds_site_with_nav_sitemap_and_link_checks() {
        let b = BrandStandards::default();
        let pages = vec![page("/", Some("Home"), "/menu"), page("/about", Some("About"), "/")];
        let built = build_site(&pages, &b, &[], &HashMap::new(), Some("https://harbour.test")).unwrap();
        let files: HashMap<_, _> = built.files.iter().map(|(k, v)| (k.as_str(), String::from_utf8_lossy(v).to_string())).collect();
        assert!(files.contains_key("index.html") && files.contains_key("about/index.html") && files.contains_key("404.html"));
        assert!(files["index.html"].contains("<nav class=\"site-nav\"") && files["about/index.html"].contains("aria-current=\"page\""));
        assert!(files["sitemap.xml"].contains("https://harbour.test/about"));
        assert!(files["robots.txt"].contains("Sitemap: https://harbour.test/sitemap.xml"));
        assert!(files.keys().any(|k| k.starts_with("site.") && k.ends_with(".css")));
        // "/menu" doesn't exist → warning, still publishable
        let home = &built.reports.iter().find(|(s, _)| s == "/").unwrap().1;
        assert!(home.violations.iter().any(|v| v.rule == "site.internal_link"));
        assert!(built.publishable());
        assert!(!files["index.html"].to_lowercase().contains("<script"));
    }

    #[test]
    fn rejects_bad_sites() {
        let b = BrandStandards::default();
        assert!(matches!(build_site(&[page("/about", None, "/")], &b, &[], &HashMap::new(), None), Err(BuildError::NoHomepage)));
        assert!(matches!(build_site(&[page("/", None, "/"), page("/About Us", None, "/")], &b, &[], &HashMap::new(), None), Err(BuildError::BadSlug(_))));
        assert!(valid_slug("/services/web-design") && !valid_slug("/x/") && !valid_slug("/../etc") && !valid_slug("//evil"));
    }

    #[test]
    fn preview_prefix_rewrites_internal_urls() {
        let b = BrandStandards::default();
        let html = render_page_with(&page("/", None, "/about"), &b, &SiteContext { prefix: "/_preview".into(), css_href: "/site.css".into(), ..Default::default() });
        assert!(html.contains("href=\"/_preview/about\"") && html.contains("href=\"/_preview/site.css\""));
    }
}
