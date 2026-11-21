//! Durable job queue on SQLite. Long renders (video, TTS, image batches,
//! site builds, backups) run here so they survive restarts and report progress.

use crate::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub payload_json: String,
    pub attempts: i64,
}

pub async fn enqueue(pool: &SqlitePool, kind: &str, payload: &serde_json::Value, priority: i64) -> Result<String> {
    let id = jugg_core::new_id();
    let now = jugg_core::now_unix();
    sqlx::query("INSERT INTO jobs (id, kind, status, priority, payload_json, run_after, created_at) VALUES (?,?,'queued',?,?,?,?)")
        .bind(&id)
        .bind(kind)
        .bind(priority)
        .bind(payload.to_string())
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
    Ok(id)
}

/// Atomically claim the highest-priority ready job. SQLite has no
/// `SKIP LOCKED`; a single UPDATE … RETURNING is atomic under the one-writer model.
pub async fn claim(pool: &SqlitePool, worker: &str) -> Result<Option<Job>> {
    let now = jugg_core::now_unix();
    Ok(sqlx::query_as::<_, Job>(
        "UPDATE jobs SET status='running', worker=?, heartbeat_at=?, attempts=attempts+1
         WHERE id = (SELECT id FROM jobs WHERE status='queued' AND run_after <= ?
                     ORDER BY priority DESC, created_at LIMIT 1)
         RETURNING id, kind, payload_json, attempts",
    )
    .bind(worker)
    .bind(now)
    .bind(now)
    .fetch_optional(pool)
    .await?)
}

pub async fn heartbeat(pool: &SqlitePool, id: &str, progress: f64) -> Result<()> {
    sqlx::query("UPDATE jobs SET heartbeat_at=?, progress=? WHERE id=?").bind(jugg_core::now_unix()).bind(progress).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn finish(pool: &SqlitePool, id: &str, result: std::result::Result<serde_json::Value, String>) -> Result<()> {
    let now = jugg_core::now_unix();
    match result {
        Ok(v) => {
            sqlx::query("UPDATE jobs SET status='succeeded', result_json=?, progress=1, finished_at=? WHERE id=?")
                .bind(v.to_string())
                .bind(now)
                .bind(id)
                .execute(pool)
                .await?;
        }
        Err(e) => {
            // Retry with exponential backoff until max_attempts.
            sqlx::query(
                "UPDATE jobs SET error=?,
                   status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'queued' END,
                   run_after = ? + (30 * (1 << attempts)),
                   finished_at = CASE WHEN attempts >= max_attempts THEN ? ELSE NULL END
                 WHERE id=?",
            )
            .bind(e)
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Requeue jobs whose worker died (no heartbeat for `stale_secs`).
pub async fn reap_stale(pool: &SqlitePool, stale_secs: i64) -> Result<u64> {
    let cutoff = jugg_core::now_unix() - stale_secs;
    Ok(sqlx::query("UPDATE jobs SET status='queued', worker=NULL WHERE status='running' AND heartbeat_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn claim_is_priority_ordered_and_exclusive() {
        let pool = crate::connect_memory().await.unwrap();
        enqueue(&pool, "render_video", &serde_json::json!({}), 10).await.unwrap();
        let hi = enqueue(&pool, "chat_reply", &serde_json::json!({}), 90).await.unwrap();
        let first = claim(&pool, "w1").await.unwrap().unwrap();
        assert_eq!(first.id, hi);
        let second = claim(&pool, "w2").await.unwrap().unwrap();
        assert_ne!(second.id, hi);
        assert!(claim(&pool, "w3").await.unwrap().is_none());
        finish(&pool, &first.id, Ok(serde_json::json!({"ok":true}))).await.unwrap();
    }
}
