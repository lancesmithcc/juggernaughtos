//! Deception layer. Paths no legitimate visitor ever requests; scanners and
//! bots hit them within minutes of a site going live. Any hit is a
//! high-confidence signal and is fed to the correlator.
//!
//! The web server routes these to [`classify`] **before** normal routing and
//! answers with a bland 404 (never reveal that it is a trap).

use crate::{Event, Severity};
use serde_json::json;

/// (path prefix, confidence, kind)
const TRAPS: &[(&str, f64, &str)] = &[
    ("/.env", 0.95, "secret_file_probe"),
    ("/.git/", 0.95, "repo_probe"),
    ("/.aws/", 0.95, "secret_file_probe"),
    ("/wp-login.php", 0.7, "cms_probe"),
    ("/wp-admin", 0.6, "cms_probe"),
    ("/xmlrpc.php", 0.7, "cms_probe"),
    ("/phpmyadmin", 0.8, "admin_probe"),
    ("/server-status", 0.6, "recon"),
    ("/backup.zip", 0.9, "backup_probe"),
    ("/db.sql", 0.9, "backup_probe"),
    ("/config.json", 0.6, "secret_file_probe"),
    ("/.ssh/", 0.95, "secret_file_probe"),
    ("/cgi-bin/", 0.6, "exploit_probe"),
    // Unique canary planted only in robots.txt "Disallow:"; only crawlers that
    // read robots.txt looking for secrets will ever request it.
    ("/internal-admin-7f3a/", 0.98, "robots_canary"),
];

/// Lines added to the public robots.txt to bait reconnaissance.
pub const ROBOTS_BAIT: &str = "Disallow: /internal-admin-7f3a/\n";

pub fn classify(path: &str, ip: &str) -> Option<Event> {
    let p = path.to_ascii_lowercase();
    let hit = TRAPS.iter().find(|(prefix, _, _)| p == *prefix || p.starts_with(prefix))?;
    let attack_shape = p.contains("../") || p.contains("%2e%2e") || p.contains("<script") || p.contains("union%20select");
    let confidence = if attack_shape { (hit.1 + 0.05f64).min(1.0) } else { hit.1 };
    let severity = if confidence >= 0.9 { Severity::Critical } else { Severity::High };
    Some(Event::new("canary", hit.2, severity, confidence, ip, json!({ "path": path })))
}

/// Generic probing signal for ordinary 404s (weak; only matters in bulk).
pub fn not_found(path: &str, ip: &str) -> Event {
    Event::new("http", "not_found", Severity::Low, 0.15, ip, json!({ "path": path }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn traps() {
        let e = super::classify("/.env", "198.51.100.1").unwrap();
        assert!(e.confidence >= 0.95 && e.subject == "198.51.100.1");
        assert!(super::classify("/about", "1.2.3.4").is_none());
        assert!(super::classify("/WP-LOGIN.PHP", "1.2.3.4").is_some());
    }
}
