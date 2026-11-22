//! Markdown for assistant replies, rendered in the browser while streaming.
//! Safe by construction: raw HTML is shown as text, images are dropped (their
//! alt text stays), and links keep only https:, mailto:, tel: or site-relative
//! targets. The output is then set as innerHTML.

use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};

fn safe_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("https://") || u.starts_with("mailto:") || u.starts_with("tel:") || (u.starts_with('/') && !u.starts_with("//")) || u.starts_with('#')
}

pub fn render(md: &str) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut in_image = 0usize;
    let events = Parser::new_ext(md, opts).filter_map(|ev| match ev {
        Event::Html(h) | Event::InlineHtml(h) => Some(Event::Text(h)),
        Event::Start(Tag::Image { .. }) => {
            in_image += 1;
            None
        }
        Event::End(TagEnd::Image) => {
            in_image = in_image.saturating_sub(1);
            None
        }
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let dest_url = if safe_url(&dest_url) { dest_url } else { CowStr::Borrowed("#") };
            Some(Event::Start(Tag::Link { link_type: if link_type == LinkType::Email { LinkType::Email } else { link_type }, dest_url, title, id }))
        }
        other => Some(other),
    });
    let mut out = String::with_capacity(md.len() * 3 / 2);
    html::push_html(&mut out, events);
    out
}

#[cfg(test)]
mod tests {
    use super::render;

    /// Every `<` in sanitized output starts a real tag (text `<` is escaped), so
    /// checking tag bodies is enough to prove no script, handler or bad URL survived.
    fn tags_are_safe(out: &str) -> Result<(), String> {
        for tag in out.split('<').skip(1).map(|t| t.split('>').next().unwrap_or("").to_ascii_lowercase()) {
            let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace()).next().unwrap_or("");
            if ["script", "iframe", "object", "embed", "style", "svg", "math", "form", "input", "base", "meta", "link"].contains(&name) {
                return Err(format!("tag <{tag}>"));
            }
            for attr in tag.split_whitespace().skip(1) {
                let key = attr.split('=').next().unwrap_or("");
                if key.starts_with("on") || key == "style" || key == "srcdoc" || key == "formaction" {
                    return Err(format!("attribute {attr} in <{tag}>"));
                }
            }
            let squashed: String = tag.chars().filter(|c| !c.is_whitespace()).collect();
            for bad in ["=\"javascript:", "=\"vbscript:", "=\"data:", "=javascript:", "=data:"] {
                if squashed.contains(bad) {
                    return Err(format!("url {bad} in <{tag}>"));
                }
            }
        }
        Ok(())
    }

    const HOSTILE: &[&str] = &[
        "<script>alert(1)</script>", "<SCRIPT SRC=//x.js>", "<img src=x onerror=alert(1)>", "<a href=\"javascript:alert(1)\">x</a>",
        "<a href=\" jaVa&#115;cript:alert(1)\">x</a>", "<a href=\"data:text/html,<script>\">x</a>", "<svg onload=alert(1)>", "<iframe srcdoc=\"<b>\">",
        "<p style=\"background:url(javascript:x)\">", "<math><mi xlink:href=\"javascript:x\">", "<form action=x><input>", "<base href=//evil>",
        "<!--", "-->", "<![CDATA[", "]]>", "\"", "'", "<", ">", "&", "<p>", "</p>", "<a href=\"https://ok.example\">ok</a>", "<details open ontoggle=x>",
        "<img src=\"https://ok/x.png\" alt=\"a\">", "<table><tr><td colspan=2 onclick=x>", "<noscript><p title=\"</noscript><img src=x onerror=x>\">",
        "<a href=\"vbscript:x\">", "<object data=x>", "<meta http-equiv=refresh content=0;url=x>", "<style>@import x</style>", "<b>", "</b>",
    ];

    fn hostile_html() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        proptest::collection::vec(prop_oneof![proptest::sample::select(HOSTILE).prop_map(String::from), "[a-z ]{0,8}"], 0..24).prop_map(|v| v.concat())
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]
        #[test]
        fn chat_markdown_is_always_safe(md in hostile_html(), link in "[a-zA-Z: /.()]{0,30}") {
            let src = format!("{md}\n\n[click]({link}) ![i]({link}) <{link}>");
            let out = render(&src);
            if let Err(e) = tags_are_safe(&out) {
                proptest::prop_assert!(false, "{e}\ninput: {src}\noutput: {out}");
            }
        }
    }

    #[test]
    fn renders_and_neutralises() {
        let h = render("**Done.** Here is [your logo](/assets/1) and <script>alert(1)</script> ![x](https://evil/t.png) [bad](javascript:alert(1))");
        assert!(h.contains("<strong>Done.</strong>"));
        assert!(h.contains("href=\"/assets/1\""));
        assert!(!h.contains("<script"), "{h}");
        assert!(h.contains("&lt;script&gt;"));
        assert!(!h.contains("evil"), "images dropped: {h}");
        assert!(!h.contains("javascript:"), "{h}");
    }
}
