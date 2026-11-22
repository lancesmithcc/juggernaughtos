//! `juggernaught steward` — the always-on daemon.
//!
//! Runs as its own user with CAP_NET_ADMIN only. Listens on a Unix socket for
//! events from the app (honeytoken hits, failed sign-ins, 404 bursts), tails
//! sshd via journald, runs integrity / health / backup-age checks on timers,
//! scores everything in the correlator and applies timed nftables bans.

use crate::correlator::Correlator;
use crate::{Action, Event, Severity};
use jugg_core::Config;
use jugg_db::sqlx::{self, SqlitePool};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Event { event: Event },
    Status,
    /// Re-take the integrity baseline (after a verified update).
    Rebaseline,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Status {
    pub firewall: String,
    pub checks: Vec<Check>,
    pub open_incidents: Vec<IncidentSummary>,
    pub active_bans: i64,
    pub events_24h: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub status: String,
    pub detail: String,
    pub checked_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentSummary {
    pub subject: String,
    pub score: f64,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Firewall {
    /// `nft` is present and the `inet juggernaught` table exists.
    Nftables,
    /// Development / unsupported host: bans are recorded, not enforced.
    Simulated,
}

impl Firewall {
    pub async fn detect() -> Self {
        if !cfg!(target_os = "linux") {
            return Firewall::Simulated;
        }
        match tokio::process::Command::new("nft").args(["list", "table", "inet", "juggernaught"]).output().await {
            Ok(o) if o.status.success() => Firewall::Nftables,
            _ => Firewall::Simulated,
        }
    }

    async fn ban(self, ip: &str, secs: u64) -> Result<(), String> {
        let Some(argv) = crate::firewall::ban_command(ip, secs) else { return Err(format!("not an IP: {ip}")) };
        match self {
            Firewall::Simulated => {
                tracing::warn!(%ip, secs, "ban (simulated: nftables not active on this host)");
                Ok(())
            }
            Firewall::Nftables => {
                let o = tokio::process::Command::new("nft").args(&argv).output().await.map_err(|e| e.to_string())?;
                if o.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&o.stderr).into())
                }
            }
        }
    }
}

pub struct Steward {
    pub cfg: Config,
    pub pool: SqlitePool,
    pub correlator: Mutex<Correlator>,
    pub firewall: Firewall,
    pub fim_roots: Vec<PathBuf>,
}

impl Steward {
    pub async fn new(cfg: Config, pool: SqlitePool, firewall: Firewall) -> Arc<Self> {
        let allow = vec!["127.0.0.1".into(), "::1".into()];
        let fim_roots = if cfg.dev {
            vec![]
        } else {
            vec!["/opt/juggernaught/current".into(), "/etc/juggernaught".into(), "/etc/systemd/system/juggernaught.service".into(), "/etc/systemd/system/jugg-steward.service".into()]
        };
        Arc::new(Self { correlator: Mutex::new(Correlator::new(cfg.steward.clone(), allow)), cfg, pool, firewall, fim_roots })
    }

    /// Score an event and carry out whatever the correlator decides.
    pub async fn handle(&self, ev: Event) -> Vec<Action> {
        let id = jugg_core::new_id();
        let sev = serde_json::to_value(ev.severity).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "info".into());
        let _ = sqlx::query("INSERT INTO events (id, source, kind, severity, confidence, subject, detail_json, created_at) VALUES (?,?,?,?,?,?,?,?)")
            .bind(&id)
            .bind(&ev.source)
            .bind(&ev.kind)
            .bind(&sev)
            .bind(ev.confidence)
            .bind(&ev.subject)
            .bind(ev.detail.to_string())
            .bind(ev.at)
            .execute(&self.pool)
            .await;
        let actions = self.correlator.lock().await.ingest(&ev);
        for a in &actions {
            match a {
                Action::Ban { subject, secs } => {
                    let applied = self.firewall.ban(subject, *secs).await;
                    if let Err(e) = &applied {
                        tracing::error!(%subject, error = %e, "ban failed");
                    }
                    let _ = sqlx::query("INSERT INTO bans (subject, reason, expires_at, created_at) VALUES (?,?,?,?) ON CONFLICT(subject) DO UPDATE SET expires_at = excluded.expires_at, reason = excluded.reason")
                        .bind(subject)
                        .bind(format!("{} via {}", ev.kind, ev.source))
                        .bind(ev.at + *secs as i64)
                        .bind(ev.at)
                        .execute(&self.pool)
                        .await;
                }
                Action::OpenIncident { subject, score } => {
                    // Forensic snapshot: the subject's recent events, frozen into the incident.
                    let recent: Vec<(String, String, String, f64, i64)> = sqlx::query_as(
                        "SELECT id, source, kind, confidence, created_at FROM events WHERE subject = ? ORDER BY created_at DESC LIMIT 50",
                    )
                    .bind(subject)
                    .fetch_all(&self.pool)
                    .await
                    .unwrap_or_default();
                    let ids: Vec<&String> = recent.iter().map(|r| &r.0).collect();
                    let snapshot: Vec<_> = recent.iter().map(|r| json!({ "source": r.1, "kind": r.2, "confidence": r.3, "at": r.4 })).collect();
                    let _ = sqlx::query("INSERT INTO incidents (id, subject, score, status, event_ids, actions_json, created_at) VALUES (?,?,?,'open',?,?,?)")
                        .bind(jugg_core::new_id())
                        .bind(subject)
                        .bind(score)
                        .bind(serde_json::to_string(&ids).unwrap_or_default())
                        .bind(json!({ "actions": actions, "snapshot": snapshot }).to_string())
                        .bind(ev.at)
                        .execute(&self.pool)
                        .await;
                    tracing::warn!(%subject, score, "incident opened");
                }
                _ => {}
            }
        }
        actions
    }

    pub async fn status(&self) -> Status {
        let now = jugg_core::now_unix();
        let checks: Vec<(String, String, String, i64)> = sqlx::query_as("SELECT name, status, detail, checked_at FROM checks ORDER BY name").fetch_all(&self.pool).await.unwrap_or_default();
        let incidents: Vec<(String, f64, i64)> = sqlx::query_as("SELECT subject, score, created_at FROM incidents WHERE status = 'open' ORDER BY created_at DESC LIMIT 20").fetch_all(&self.pool).await.unwrap_or_default();
        Status {
            firewall: if self.firewall == Firewall::Nftables { "nftables".into() } else { "simulated".into() },
            checks: checks.into_iter().map(|(name, status, detail, checked_at)| Check { name, status, detail, checked_at }).collect(),
            open_incidents: incidents.into_iter().map(|(subject, score, created_at)| IncidentSummary { subject, score, created_at }).collect(),
            active_bans: sqlx::query_scalar("SELECT count(*) FROM bans WHERE expires_at > ?").bind(now).fetch_one(&self.pool).await.unwrap_or(0),
            events_24h: sqlx::query_scalar("SELECT count(*) FROM events WHERE created_at > ?").bind(now - 86_400).fetch_one(&self.pool).await.unwrap_or(0),
        }
    }

    async fn set_check(&self, name: &str, status: &str, detail: &str) {
        let _ = sqlx::query("INSERT INTO checks (name, status, detail, checked_at) VALUES (?,?,?,?) ON CONFLICT(name) DO UPDATE SET status=excluded.status, detail=excluded.detail, checked_at=excluded.checked_at")
            .bind(name)
            .bind(status)
            .bind(detail)
            .bind(jugg_core::now_unix())
            .execute(&self.pool)
            .await;
    }

    /// File integrity: compare against the stored baseline; first run records it.
    pub async fn fim_scan(&self, rebaseline: bool) {
        if self.fim_roots.is_empty() {
            return self.set_check("fim", "ok", "not monitored in development").await;
        }
        let now = crate::fim::snapshot(&self.fim_roots);
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT path, sha256 FROM fim_baseline").fetch_all(&self.pool).await.unwrap_or_default();
        if rows.is_empty() || rebaseline {
            let _ = sqlx::query("DELETE FROM fim_baseline").execute(&self.pool).await;
            for (p, h) in &now {
                let _ = sqlx::query("INSERT INTO fim_baseline (path, sha256, mode, updated_at) VALUES (?,?,0,?)").bind(p.display().to_string()).bind(h).bind(jugg_core::now_unix()).execute(&self.pool).await;
            }
            return self.set_check("fim", "ok", &format!("baseline recorded ({} files)", now.len())).await;
        }
        let baseline = rows.into_iter().map(|(p, h)| (PathBuf::from(p), h)).collect();
        let drift = crate::fim::diff(&baseline, &now);
        if drift.is_empty() {
            self.set_check("fim", "ok", &format!("{} files unchanged", now.len())).await;
        } else {
            self.set_check("fim", "fail", &format!("{} file(s) changed outside an update", drift.len())).await;
            for ev in drift {
                self.handle(ev).await;
            }
        }
    }

    pub async fn disk_check(&self) {
        let out = tokio::process::Command::new("df").args(["-Pk"]).arg(&self.cfg.data_dir).output().await;
        let pct = out.ok().and_then(|o| {
            String::from_utf8_lossy(&o.stdout).lines().nth(1).and_then(|l| l.split_whitespace().nth(4).map(|p| p.trim_end_matches('%').to_string())).and_then(|p| p.parse::<u32>().ok())
        });
        match pct {
            Some(p) if p >= 90 => self.set_check("disk", "fail", &format!("{p}% used")).await,
            Some(p) if p >= 80 => self.set_check("disk", "warn", &format!("{p}% used")).await,
            Some(p) => self.set_check("disk", "ok", &format!("{p}% used")).await,
            None => self.set_check("disk", "warn", "could not read disk usage").await,
        }
    }

    pub async fn backup_check(&self) {
        let newest = std::fs::read_dir(&self.cfg.backup.dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "jbk"))
            .filter_map(|e| e.metadata().ok()?.modified().ok())
            .max();
        match newest.and_then(|t| t.elapsed().ok()) {
            Some(age) if age.as_secs() < 26 * 3600 => self.set_check("backup", "ok", &format!("last backup {}h ago", age.as_secs() / 3600)).await,
            Some(age) => self.set_check("backup", "warn", &format!("last backup {}h ago", age.as_secs() / 3600)).await,
            None => self.set_check("backup", "warn", "no backups yet").await,
        }
    }

    pub async fn site_check(&self, http: &reqwest::Client, failures: &mut u32) {
        let url = match (&self.cfg.domain, self.cfg.tls.mode) {
            (Some(d), jugg_core::config::TlsMode::Acme) if !self.cfg.dev => format!("https://admin.{d}/favicon.svg"),
            _ => format!("http://{}/favicon.svg", self.cfg.listen_http.replace("0.0.0.0", "127.0.0.1")),
        };
        match http.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                *failures = 0;
                self.set_check("site", "ok", "responding").await;
            }
            other => {
                *failures += 1;
                let why = other.map(|r| r.status().to_string()).unwrap_or_else(|e| e.to_string());
                self.set_check("site", if *failures >= 3 { "fail" } else { "warn" }, &format!("{url}: {why}")).await;
                if *failures == 3 && !self.cfg.dev {
                    // Self-heal: the unit grants this exact restart via polkit rule (install.sh).
                    let _ = tokio::process::Command::new("systemctl").args(["restart", "juggernaught.service"]).status().await;
                }
            }
        }
    }
}

/// Parse an sshd journal line into an event.
pub fn sshd_event(message: &str) -> Option<Event> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"from ([0-9a-fA-F:.]+) port").unwrap());
    let ip = re.captures(message)?.get(1)?.as_str().to_string();
    let (kind, sev, conf) = if message.contains("Invalid user") {
        ("ssh_invalid_user", Severity::Medium, 0.5)
    } else if message.contains("Failed password") || message.contains("Failed publickey") {
        ("ssh_failed_auth", Severity::Low, 0.35)
    } else if message.contains("maximum authentication attempts") {
        ("ssh_max_attempts", Severity::Medium, 0.6)
    } else {
        return None;
    };
    Some(Event::new("sshd", kind, sev, conf, ip, json!({ "line": message.chars().take(300).collect::<String>() })))
}

async fn tail_sshd(st: Arc<Steward>) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let Ok(mut child) = tokio::process::Command::new("journalctl")
        .args(["-f", "-n", "0", "-o", "cat", "-u", "ssh.service", "-u", "sshd.service"])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    else {
        return tracing::warn!("journalctl unavailable; sshd sensor off");
    };
    let Some(out) = child.stdout.take() else { return };
    let mut lines = BufReader::new(out).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(ev) = sshd_event(&line) {
            st.handle(ev).await;
        }
    }
}

async fn serve_socket(st: Arc<Steward>, path: PathBuf) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Group `jugg` (the app) may connect; nobody else.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))?;
    }
    tracing::info!(socket = %path.display(), "steward listening");
    loop {
        let (stream, _) = listener.accept().await?;
        let st = st.clone();
        tokio::spawn(async move {
            let (r, mut w) = stream.into_split();
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.len() > 64 * 1024 {
                    break;
                }
                let resp = match serde_json::from_str::<Request>(&line) {
                    Ok(Request::Event { event }) => json!({ "ok": true, "actions": st.handle(event).await }),
                    Ok(Request::Status) => json!({ "ok": true, "status": st.status().await }),
                    Ok(Request::Rebaseline) => {
                        st.fim_scan(true).await;
                        json!({ "ok": true })
                    }
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                };
                if w.write_all(format!("{resp}\n").as_bytes()).await.is_err() {
                    break;
                }
            }
        });
    }
}

/// Run forever.
pub async fn run(cfg: Config) -> Result<(), String> {
    let pool = jugg_db::connect_steward(&cfg.steward_dir.join("steward.db")).await.map_err(|e| e.to_string())?;
    let firewall = Firewall::detect().await;
    tracing::info!(?firewall, "steward starting");
    let socket = cfg.steward_socket.clone();
    let st = Steward::new(cfg, pool, firewall).await;

    tokio::spawn(tail_sshd(st.clone()));
    {
        let st = st.clone();
        tokio::spawn(async move {
            let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_default();
            let mut failures = 0;
            let mut tick = 0u64;
            let mut iv = tokio::time::interval(Duration::from_secs(60));
            loop {
                iv.tick().await;
                if tick % 5 == 0 {
                    st.site_check(&http, &mut failures).await;
                }
                if tick % 10 == 0 {
                    st.disk_check().await;
                }
                if tick % 15 == 0 {
                    st.fim_scan(false).await;
                }
                if tick % 60 == 0 {
                    st.backup_check().await;
                    let _ = sqlx::query("DELETE FROM bans WHERE expires_at < ?").bind(jugg_core::now_unix()).execute(&st.pool).await;
                }
                tick += 1;
            }
        });
    }
    serve_socket(st, socket).await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sshd_lines() {
        let e = sshd_event("Invalid user admin from 203.0.113.5 port 51234").unwrap();
        assert_eq!(e.subject, "203.0.113.5");
        assert_eq!(e.kind, "ssh_invalid_user");
        assert!(sshd_event("Accepted publickey for lance from 10.0.0.2 port 22 ssh2").is_none());
        assert_eq!(sshd_event("Failed password for root from 2001:db8::7 port 4444 ssh2").unwrap().subject, "2001:db8::7");
    }

    #[tokio::test]
    async fn honeytoken_event_bans_and_opens_incident() {
        let pool = jugg_db::connect_steward_memory().await.unwrap();
        let st = Steward::new(jugg_core::Config::dev(), pool, Firewall::Simulated).await;
        let ev = crate::honeytokens::classify("/.env", "198.51.100.9").unwrap();
        let actions = st.handle(ev).await;
        assert!(actions.iter().any(|a| matches!(a, Action::Ban { .. })));
        let s = st.status().await;
        assert_eq!(s.active_bans, 1);
        assert_eq!(s.open_incidents.len(), 1);
        assert_eq!(s.firewall, "simulated");
    }
}
