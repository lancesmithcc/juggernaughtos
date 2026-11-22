//! Database layer. One SQLite file in WAL mode, one writer (this process),
//! many readers. Migrations are embedded in the binary.

pub mod audit;
pub mod jobs;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

pub use sqlx;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
pub static STEWARD_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations/steward");

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Crypto(#[from] jugg_crypto::CryptoError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

fn options(url: &str) -> Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .pragma("temp_store", "memory")
        .pragma("secure_delete", "on"))
}

/// Open (creating if needed) and migrate the main database.
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| DbError::Other(e.to_string()))?;
    }
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options(&format!("sqlite://{}", path.display()))?)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// The steward's own database (separate file, separate OS user).
pub async fn connect_steward(path: &Path) -> Result<SqlitePool> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| DbError::Other(e.to_string()))?;
    }
    let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options(&format!("sqlite://{}", path.display()))?).await?;
    STEWARD_MIGRATOR.run(&pool).await?;
    Ok(pool)
}

pub async fn connect_steward_memory() -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options("sqlite::memory:")?).await?;
    STEWARD_MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// In-memory database for tests.
pub async fn connect_memory() -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options("sqlite::memory:")?).await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn migrations_apply() {
        let pool = super::connect_memory().await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE '\\_%' ESCAPE '\\'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(n >= 20, "expected full schema, got {n} tables");
        // Brand history: several versions of the same brand must coexist.
        for v in 1..=3 {
            sqlx::query("INSERT INTO brands (id, version, standards_json, is_active, created_at) VALUES ('brand', ?, '{}', 0, 0)").bind(v).execute(&pool).await.unwrap();
        }
    }
}
