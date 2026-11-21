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
