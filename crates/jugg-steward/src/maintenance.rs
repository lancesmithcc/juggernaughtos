//! Scheduled maintenance, all self-running. Each job writes a row to `checks`
//! and the admin "Security" panel turns them into one plain-language status.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub name: &'static str,
    /// systemd OnCalendar expression (the steward also runs them on its own timer).
    pub schedule: &'static str,
    pub what: &'static str,
}

pub const TASKS: &[Task] = &[
    Task { name: "os_security_updates", schedule: "*-*-* 03:30", what: "unattended-upgrades security pocket; reboot in window if kernel changed" },
    Task { name: "platform_update_check", schedule: "*-*-* 04:00", what: "juggernaught + tools.lock: verify ML-DSA manifest, stage, canary, promote or roll back" },
    Task { name: "cve_scan", schedule: "*-*-* 04:30", what: "cargo-audit DB + OSV for tools.lock, npm audit for node tools" },
    Task { name: "backup", schedule: "*-*-* 02:00", what: "SQLite online backup + assets → AES-256-GCM chunks, DEK hybrid-wrapped (ML-KEM-768+X25519) to owner recovery key, manifest ML-DSA signed; push off-box" },
    Task { name: "backup_restore_test", schedule: "Sun *-*-* 05:00", what: "restore latest backup into a scratch dir and verify hashes" },
    Task { name: "tls_expiry", schedule: "*-*-* 06:00", what: "warn at 21 days, fail at 7; rustls-acme renews at 30" },
    Task { name: "site_watchdog", schedule: "*:0/5", what: "GET public site + admin health over the public internet path; restart unit after 3 failures" },
    Task { name: "disk_and_memory", schedule: "*:0/10", what: "warn at 80% disk / swap pressure; prune render caches first" },
    Task { name: "fim_scan", schedule: "*:0/15", what: "hash release, /etc/juggernaught, unit files against baseline" },
    Task { name: "audit_chain_verify", schedule: "*-*-* 05:30", what: "recompute audit hash chain, verify signed checkpoints" },
    Task { name: "font_mirror_sync", schedule: "Sun *-*-* 03:00", what: "pull google/fonts changes, rebuild subset index" },
    Task { name: "decoy_rotation", schedule: "*-*-01 03:00", what: "rotate honeytoken values and canary paths" },
];
