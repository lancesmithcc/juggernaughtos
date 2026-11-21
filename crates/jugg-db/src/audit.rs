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

#[cfg(test)]
mod tests {
    use super::*;

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
