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
    pub tls: TlsConfig,
    pub backup: BackupConfig,
    /// Unix socket the steward daemon listens on.
    pub steward_socket: PathBuf,
    /// Steward's own state (separate user, separate database).
    pub steward_dir: PathBuf,
}

/// How HTTPS is provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    /// Built-in Let's Encrypt (TLS-ALPN-01 on :443), PQ-hybrid rustls.
    Acme,
    /// Plain HTTP on `listen_http` behind a trusted tunnel/proxy (e.g. Cloudflare Tunnel).
    Proxy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    pub mode: TlsMode,
    /// Use Let's Encrypt staging (for testing installs without rate limits).
    pub staging: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self { mode: TlsMode::Acme, staging: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupConfig {
    pub dir: PathBuf,
    /// Daily backups kept locally.
    pub keep: usize,
    /// Optional rclone remote (`remote:bucket/path`) for off-box copies.
    pub rclone_remote: Option<String>,
    /// Owner's offline recovery public key (ML-KEM-768 + X25519).
    pub recovery_public_key: PathBuf,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            dir: "/var/lib/juggernaught/backups".into(),
            keep: 7,
            rclone_remote: None,
            recovery_public_key: "/etc/juggernaught/recovery.pub.json".into(),
        }
    }
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
            tls: TlsConfig::default(),
            backup: BackupConfig::default(),
            steward_socket: "/run/juggernaught/steward.sock".into(),
            steward_dir: "/var/lib/juggernaught-steward".into(),
        }
    }
}

impl Config {
    /// Development config. Data lives in `$JUGG_DEV_ROOT` or `./.jugg-dev`,
    /// always as an absolute path so child processes (the MCP bridge launched
    /// by Claude Code / Codex from a job directory) find the same database.
    pub fn dev() -> Self {
        let base = std::env::var_os("JUGG_DEV_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join(".jugg-dev"));
        Self {
            data_dir: base.join("data"),
            keys_dir: base.join("keys"),
            tools_dir: base.join("tools"),
            listen_http: "127.0.0.1:3700".into(),
            dev: true,
            tls: TlsConfig { mode: TlsMode::Proxy, staging: true },
            backup: BackupConfig { dir: base.join("backups"), recovery_public_key: base.join("keys").join("recovery.pub.json"), ..BackupConfig::default() },
            steward_socket: base.join("steward.sock"),
            steward_dir: base.join("steward"),
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

    /// Published public site (atomic symlink to the live deploy).
    pub fn site_dir(&self) -> PathBuf {
        self.data_dir.join("site").join("current")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.data_dir.join("assets")
    }

    pub fn acme_cache(&self) -> PathBuf {
        self.data_dir.join("acme")
    }

    /// Host names this box answers for: apex, www, admin.
    pub fn hostnames(&self) -> Vec<String> {
        match &self.domain {
            Some(d) => vec![d.clone(), format!("www.{d}"), format!("admin.{d}")],
            None => vec![],
        }
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
