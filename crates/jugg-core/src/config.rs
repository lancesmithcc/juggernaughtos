use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// `/etc/juggernaught/juggernaught.toml`. Secrets never live here: provider
/// keys are encrypted in SQLite and the KEK lives in `keys_dir`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Public site domain, e.g. `example.com`. Admin is served at `admin.<domain>`.
    pub domain: Option<String>,
    /// Contact for ACME / Let's Encrypt.
    pub acme_email: Option<String>,
    pub data_dir: PathBuf,
    pub keys_dir: PathBuf,
    pub tools_dir: PathBuf,
    pub listen_http: String,
    pub listen_https: String,
    /// Dev mode: plain HTTP on loopback, no ACME, verbose logs.
    pub dev: bool,
    pub mail: MailConfig,
    pub steward: StewardConfig,
    pub updates: UpdatePolicy,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            domain: None,
            acme_email: None,
            data_dir: "/var/lib/juggernaught".into(),
            keys_dir: "/var/lib/juggernaught/keys".into(),
            tools_dir: "/opt/juggernaught/tools".into(),
            listen_http: "0.0.0.0:80".into(),
            listen_https: "0.0.0.0:443".into(),
            dev: false,
            mail: MailConfig::default(),
            steward: StewardConfig::default(),
            updates: UpdatePolicy::default(),
        }
    }
}

impl Config {
    pub fn dev() -> Self {
        let base = PathBuf::from("./.jugg-dev");
        Self {
            data_dir: base.join("data"),
            keys_dir: base.join("keys"),
            tools_dir: base.join("tools"),
            listen_http: "127.0.0.1:3700".into(),
            dev: true,
            ..Self::default()
        }
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("juggernaught.db")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MailConfig {
    /// SMTP relay for magic links (`smtp://user@host:587`). Password is stored encrypted.
    pub smtp_url: Option<String>,
    pub from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StewardConfig {
    /// Score at which an IP is banned at the firewall.
    pub ban_threshold: f64,
    /// Score at which an incident is opened and AI triage runs.
    pub incident_threshold: f64,
    /// Seconds for an IP's score to halve.
    pub score_half_life_secs: u64,
    pub ban_secs: u64,
    /// Heavy sensors are enabled only when the box has enough RAM.
    pub enable_falco: Option<bool>,
    pub enable_suricata: Option<bool>,
}

impl Default for StewardConfig {
    fn default() -> Self {
        Self {
            ban_threshold: 1.0,
            incident_threshold: 0.6,
            score_half_life_secs: 1800,
            ban_secs: 86_400,
            enable_falco: None,
            enable_suricata: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePolicy {
    pub channel: String,
    /// Apply security patches without asking.
    pub auto_security: bool,
    /// Apply minor versions after the staged canary passes.
    pub auto_minor: bool,
    /// Daily window (UTC hour) for updates and restarts.
    pub window_utc_hour: u8,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self { channel: "stable".into(), auto_security: true, auto_minor: true, window_utc_hour: 9 }
    }
}
