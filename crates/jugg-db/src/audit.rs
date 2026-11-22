//! Tamper-evident audit log: every entry hashes the previous one; periodic
//! checkpoints are signed with ML-DSA-65 so history can be verified offline.

use crate::Result;
use jugg_crypto::{sha256_hex, Signer};
use sqlx::SqlitePool;

pub const CHECKPOINT_EVERY: i64 = 64;
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub async fn append(pool: &SqlitePool, signer: Option<&Signer>, actor: &str, action: &str, detail: &serde_json::Value) -> Result<i64> {
    let mut tx = pool.begin().await?;
    let prev: Option<String> = sqlx::query_scalar("SELECT entry_hash FROM audit_log ORDER BY seq DESC LIMIT 1").fetch_optional(&mut *tx).await?;
    let prev = prev.unwrap_or_else(|| GENESIS.to_string());
    let now = jugg_core::now_unix();
    let detail = detail.to_string();
    let entry_hash = sha256_hex(format!("{prev}|{actor}|{action}|{detail}|{now}").as_bytes());
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO audit_log (prev_hash, entry_hash, actor, action, detail_json, created_at) VALUES (?,?,?,?,?,?) RETURNING seq",
    )
    .bind(&prev)
    .bind(&entry_hash)
    .bind(actor)
    .bind(action)
    .bind(&detail)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(signer) = signer {
        if seq % CHECKPOINT_EVERY == 0 {
            let sig = signer.sign(format!("jugg-audit-v1|{seq}|{entry_hash}").as_bytes())?;
            sqlx::query("INSERT INTO audit_checkpoints (seq, entry_hash, signature, created_at) VALUES (?,?,?,?)")
                .bind(seq)
                .bind(&entry_hash)
                .bind(sig)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(seq)
}

/// Recompute the chain. Returns the first broken `seq`, if any.
pub async fn verify_chain(pool: &SqlitePool) -> Result<Option<i64>> {
    let rows: Vec<(i64, String, String, String, String, String, i64)> =
        sqlx::query_as("SELECT seq, prev_hash, entry_hash, actor, action, detail_json, created_at FROM audit_log ORDER BY seq")
            .fetch_all(pool)
            .await?;
    let mut prev = GENESIS.to_string();
    for (seq, p, h, actor, action, detail, at) in rows {
        let expect = sha256_hex(format!("{prev}|{actor}|{action}|{detail}|{at}").as_bytes());
        if p != prev || h != expect {
            return Ok(Some(seq));
        }
        prev = h;
    }
    Ok(None)
}

// ---------------------------------------------------------------- export

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};

pub const EXPORT_FORMAT: &str = "jugg-audit-export-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExportEntry {
    pub seq: i64,
    pub prev_hash: String,
    pub entry_hash: String,
    pub actor: String,
    pub action: String,
    pub detail_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportCheckpoint {
    pub seq: i64,
    pub entry_hash: String,
    /// ML-DSA-65 over "jugg-audit-v1|seq|entry_hash", base64.
    pub signature: String,
}

/// Self-contained, offline-verifiable copy of the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Export {
    pub format: String,
    pub exported_at: i64,
    /// The box's ML-DSA-65 public key, base64. Pin it with its fingerprint.
    pub box_public_key: String,
    pub entries: Vec<ExportEntry>,
    pub checkpoints: Vec<ExportCheckpoint>,
    /// Signature over "jugg-audit-export-v1|<last seq>|<last hash>|<exported_at>", base64.
    /// Covers the entries after the last periodic checkpoint.
    pub head_signature: String,
}

/// First 16 bytes of SHA-256 of the public key, hex, grouped. Print it somewhere
/// the box cannot rewrite (owner's notes, the recovery-key sheet).
pub fn fingerprint(public_key: &[u8]) -> String {
    let h = sha256_hex(public_key);
    h.as_bytes()[..32].chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join(":")
}

fn head_message(entries: &[ExportEntry], exported_at: i64) -> String {
    let (seq, hash) = entries.last().map(|e| (e.seq, e.entry_hash.as_str())).unwrap_or((0, GENESIS));
    format!("{EXPORT_FORMAT}|{seq}|{hash}|{exported_at}")
}

pub async fn export(pool: &SqlitePool, signer: &Signer) -> Result<Export> {
    let entries: Vec<ExportEntry> = sqlx::query_as::<_, (i64, String, String, String, String, String, i64)>(
        "SELECT seq, prev_hash, entry_hash, actor, action, detail_json, created_at FROM audit_log ORDER BY seq",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(seq, prev_hash, entry_hash, actor, action, detail_json, created_at)| ExportEntry { seq, prev_hash, entry_hash, actor, action, detail_json, created_at })
    .collect();
    let checkpoints = sqlx::query_as::<_, (i64, String, Vec<u8>)>("SELECT seq, entry_hash, signature FROM audit_checkpoints ORDER BY seq")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(seq, entry_hash, sig)| ExportCheckpoint { seq, entry_hash, signature: B64.encode(sig) })
        .collect();
    let exported_at = jugg_core::now_unix();
    let head_signature = B64.encode(signer.sign(head_message(&entries, exported_at).as_bytes())?);
    Ok(Export { format: EXPORT_FORMAT.into(), exported_at, box_public_key: B64.encode(signer.public_key()), entries, checkpoints, head_signature })
}

#[derive(Debug, Default, PartialEq)]
pub struct Verified {
    pub entries: usize,
    pub checkpoints: usize,
    pub fingerprint: String,
}

/// Verify an export with nothing but the file (and ideally the pinned fingerprint).
pub fn verify_export(x: &Export, pinned_fingerprint: Option<&str>) -> std::result::Result<Verified, String> {
    if x.format != EXPORT_FORMAT {
        return Err(format!("unknown format {:?}", x.format));
    }
    let pk = B64.decode(&x.box_public_key).map_err(|_| "box_public_key is not base64".to_string())?;
    let fp = fingerprint(&pk);
    if let Some(want) = pinned_fingerprint {
        let norm = |s: &str| s.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase();
        if norm(want) != norm(&fp) {
            return Err(format!("signing key fingerprint {fp} does not match the pinned {want}"));
        }
    }
    let mut prev = GENESIS.to_string();
    let mut last_seq = 0;
    for e in &x.entries {
        if e.seq <= last_seq {
            return Err(format!("entry {} is out of order", e.seq));
        }
        let expect = sha256_hex(format!("{prev}|{}|{}|{}|{}", e.actor, e.action, e.detail_json, e.created_at).as_bytes());
        if e.prev_hash != prev || e.entry_hash != expect {
            return Err(format!("hash chain broken at entry {}", e.seq));
        }
        prev = e.entry_hash.clone();
        last_seq = e.seq;
    }
    let by_seq: std::collections::HashMap<i64, &ExportEntry> = x.entries.iter().map(|e| (e.seq, e)).collect();
    for c in &x.checkpoints {
        let e = by_seq.get(&c.seq).ok_or_else(|| format!("checkpoint {} points at a missing entry (history deleted?)", c.seq))?;
        if e.entry_hash != c.entry_hash {
            return Err(format!("checkpoint {} does not match entry {}", c.seq, c.seq));
        }
        let sig = B64.decode(&c.signature).map_err(|_| format!("checkpoint {} signature is not base64", c.seq))?;
        if !jugg_crypto::verify_signature(&pk, format!("jugg-audit-v1|{}|{}", c.seq, c.entry_hash).as_bytes(), &sig) {
            return Err(format!("checkpoint {} signature is invalid", c.seq));
        }
    }
    // Every CHECKPOINT_EVERY-th entry must carry a checkpoint (entries signed with a signer).
    let have: std::collections::HashSet<i64> = x.checkpoints.iter().map(|c| c.seq).collect();
    if let Some(missing) = x.entries.iter().map(|e| e.seq).find(|s| s % CHECKPOINT_EVERY == 0 && !have.contains(s)) {
        return Err(format!("checkpoint {missing} is missing"));
    }
    let head = B64.decode(&x.head_signature).map_err(|_| "head_signature is not base64".to_string())?;
    if !jugg_crypto::verify_signature(&pk, head_message(&x.entries, x.exported_at).as_bytes(), &head) {
        return Err("export signature is invalid (entries added, removed or edited after export)".into());
    }
    Ok(Verified { entries: x.entries.len(), checkpoints: x.checkpoints.len(), fingerprint: fp })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_verifies_offline_and_catches_edits() {
        let pool = crate::connect_memory().await.unwrap();
        let dir = std::env::temp_dir().join(format!("jugg-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let signer = Signer::load_or_create(&dir.join("s.pk8")).unwrap();
        for i in 0..(CHECKPOINT_EVERY + 5) {
            append(&pool, Some(&signer), "owner", "site.publish", &serde_json::json!({ "n": i })).await.unwrap();
        }
        let x = export(&pool, &signer).await.unwrap();
        let fp = fingerprint(&signer.public_key());
        let ok = verify_export(&x, Some(&fp)).unwrap();
        assert_eq!((ok.entries, ok.checkpoints), (CHECKPOINT_EVERY as usize + 5, 1));
        assert!(verify_export(&x, Some("00:00")).is_err(), "wrong pinned key");

        let mut edited = x.clone();
        edited.entries[3].detail_json = "{\"n\":999}".into();
        assert!(verify_export(&edited, None).unwrap_err().contains("chain broken at entry 4"));

        let mut truncated = x.clone();
        truncated.entries.pop();
        assert!(verify_export(&truncated, None).unwrap_err().contains("export signature"));

        let mut rewritten = x.clone();
        rewritten.entries.truncate(10);
        rewritten.checkpoints.clear();
        assert!(verify_export(&rewritten, None).unwrap_err().contains("export signature"));

        let mut swapped = x.clone();
        let other = Signer::load_or_create(&dir.join("o.pk8")).unwrap();
        swapped.box_public_key = B64.encode(other.public_key());
        assert!(verify_export(&swapped, None).is_err(), "checkpoints signed by a different key");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn chain_detects_tampering() {
        let pool = crate::connect_memory().await.unwrap();
        for i in 0..5 {
            append(&pool, None, "owner", "brand.update", &serde_json::json!({ "v": i })).await.unwrap();
        }
        assert_eq!(verify_chain(&pool).await.unwrap(), None);
        sqlx::query("UPDATE audit_log SET detail_json = '{\"v\":99}' WHERE seq = 3").execute(&pool).await.unwrap();
        assert_eq!(verify_chain(&pool).await.unwrap(), Some(3));
    }
}
