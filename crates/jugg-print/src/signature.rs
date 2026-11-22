//! Email signature: table layout + inline styles, because email clients
//! (Outlook, Gmail) ignore stylesheets. Colours and fonts still come only from
//! brand tokens; the logo must be an absolute https URL on the business's site.

use crate::esc;
use jugg_brand::BrandStandards;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignatureInput {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub website: Option<String>,
    /// Absolute https URL of the logo (hosted on the business's own site).
    #[serde(default)]
    pub logo_url: Option<String>,
}

fn link(href: &str, label: &str, color: &str) -> String {
    format!(r#"<a href="{}" style="color:{color};text-decoration:none;">{}</a>"#, esc(href), esc(label))
}

pub fn email_signature(b: &BrandStandards, s: &SignatureInput) -> String {
    let ink = b.color("ink").unwrap_or("#1b1a17");
    let soft = b.color("ink-soft").unwrap_or("#5e5a52");
    let primary = b.color("primary").unwrap_or("#c2502a");
    let font = format!("'{}',{}", b.typography.body.family, "Arial,Helvetica,sans-serif");
    let mut contact = Vec::new();
    if let Some(p) = s.phone.as_deref().filter(|p| !p.trim().is_empty()) {
        let digits: String = p.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();
        contact.push(link(&format!("tel:{digits}"), p, ink));
    }
    if let Some(e) = s.email.as_deref().filter(|e| e.contains('@')) {
        contact.push(link(&format!("mailto:{e}"), e, ink));
    }
    if let Some(w) = s.website.as_deref().filter(|w| !w.trim().is_empty()) {
        let href = if w.starts_with("https://") { w.to_string() } else { format!("https://{}", w.trim_start_matches("http://")) };
        contact.push(link(&href, w.trim_start_matches("https://").trim_start_matches("http://"), primary));
    }
    let logo = s
        .logo_url
        .as_deref()
        .filter(|u| u.starts_with("https://"))
        .map(|u| format!(r#"<td style="padding:0 14px 0 0;vertical-align:top;"><img src="{}" alt="{}" width="56" height="56" style="display:block;border:0;border-radius:8px;"></td>"#, esc(u), esc(&b.name)))
        .unwrap_or_default();
    format!(
        r#"<table cellpadding="0" cellspacing="0" border="0" role="presentation" style="font-family:{font};font-size:13px;line-height:1.45;color:{ink};"><tr>{logo}<td style="vertical-align:top;border-left:2px solid {primary};padding-left:14px;"><div style="font-size:15px;font-weight:700;color:{ink};">{name}</div>{title}<div style="font-size:12px;color:{soft};margin-top:2px;">{business}</div><div style="margin-top:8px;font-size:12px;">{contact}</div></td></tr></table>"#,
        font = esc(&font),
        name = esc(s.name.trim()),
        title = s.title.as_deref().filter(|t| !t.trim().is_empty()).map(|t| format!(r#"<div style="font-size:12px;color:{soft};">{}</div>"#, esc(t))).unwrap_or_default(),
        business = esc(&b.name),
        contact = contact.join(&format!(r#" <span style="color:{soft};">·</span> "#)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_inline_and_safe() {
        let b = BrandStandards::default();
        let html = email_signature(&b, &SignatureInput { name: "Ada \"<b>\"".into(), email: Some("ada@harbour.test".into()), website: Some("harbour.test".into()), logo_url: Some("http://insecure/logo.png".into()), ..Default::default() });
        assert!(html.contains("mailto:ada@harbour.test") && html.contains("https://harbour.test"));
        assert!(!html.contains("<b>"), "escaped");
        assert!(!html.contains("insecure"), "non-https logo dropped");
        assert!(!html.contains("<style"), "inline styles only");
    }
}
