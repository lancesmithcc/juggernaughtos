//! The Steward: Juggernaught's always-on security and maintenance layer.
//!
//! Modelled on a hardened production box (fail2ban + UFW + unattended-upgrades +
//! Falco + Suricata + OpenCanary honeypots + honeytoken files + a confidence-
//! scoring correlator), rebuilt as one small Rust daemon that fits a 4 GB VPS:
//!
//! ```text
//!  sensors ──events──▶ correlator ──score≥incident──▶ incident + AI triage
//!  (http, sshd,        (per-subject,      └─score≥ban──▶ nftables ban (timed)
//!   canary, fim,        decaying score)
//!   health, falco*,
//!   suricata*)                         * optional, enabled by RAM
//! ```
//!
//! It runs as its own systemd service and user (`jugg-steward`) with only
//! CAP_NET_ADMIN, owns its own SQLite file, and talks to the app over a Unix
//! socket, so a compromised web process cannot erase security history.

pub mod client;
pub mod correlator;
pub mod daemon;
pub mod firewall;
pub mod fim;
pub mod honeytokens;
pub mod maintenance;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub source: String,
    pub kind: String,
    pub severity: Severity,
    /// 0–1: how sure the sensor is that this is malicious. Honeytoken hits ≈ 0.95,
    /// one failed login ≈ 0.05.
    pub confidence: f64,
    /// Who/what it is about: an IP, a path, a service.
    pub subject: String,
    pub detail: serde_json::Value,
    pub at: i64,
}

impl Event {
    pub fn new(source: &str, kind: &str, severity: Severity, confidence: f64, subject: impl Into<String>, detail: serde_json::Value) -> Self {
        Self { source: source.into(), kind: kind.into(), severity, confidence, subject: subject.into(), detail, at: jugg_core::now_unix() }
    }
}

/// What the steward decided to do. Every action is recorded on the incident.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Ban { subject: String, secs: u64 },
    OpenIncident { subject: String, score: f64 },
    Snapshot { subject: String },
    Notify { message: String },
    RestartService { unit: String },
}

/// Which optional heavy sensors this box can afford.
pub fn sensor_plan(total_ram_mb: u64) -> Vec<&'static str> {
    let mut v = vec!["http", "sshd", "canary", "fim", "health", "cve", "crowdsec"];
    if total_ram_mb >= 7_500 {
        v.push("falco");
    }
    if total_ram_mb >= 15_000 {
        v.push("suricata");
    }
    v
}
