use crate::{random_token, AuthError, Result};
use sqlx::SqlitePool;

pub const TTL_SECS: i64 = 15 * 60;

/// Issue a magic-link token for `user_id`. The caller emails
/// `https://admin.<domain>/auth/magic/<token>`; only the hash is stored.
/// Always respond "check your email" whether or not the address exists.
pub async fn issue(pool: &SqlitePool, user_id: &str) -> Result<String> {
    let token = random_token();
    let now = jugg_core::now_unix();
    // One live link per user: issuing a new one invalidates older ones.
    sqlx::query("UPDATE auth_magic_links SET used_at = ? WHERE user_id = ? AND used_at IS NULL").bind(now).bind(user_id).execute(pool).await?;
    sqlx::query("INSERT INTO auth_magic_links (token_hash, user_id, expires_at) VALUES (?,?,?)")
        .bind(jugg_crypto::sha256_hex(token.as_bytes()))
        .bind(user_id)
        .bind(now + TTL_SECS)
        .execute(pool)
        .await?;
    Ok(token)
}

/// Consume a token. Single use: the UPDATE only succeeds once.
pub async fn redeem(pool: &SqlitePool, token: &str) -> Result<String> {
    let now = jugg_core::now_unix();
    let user: Option<String> = sqlx::query_scalar(
        "UPDATE auth_magic_links SET used_at = ? WHERE token_hash = ? AND used_at IS NULL AND expires_at > ? RETURNING user_id",
    )
    .bind(now)
    .bind(jugg_crypto::sha256_hex(token.as_bytes()))
    .bind(now)
    .fetch_optional(pool)
    .await?;
    user.ok_or(AuthError::Invalid)
}
