use crate::{random_token, AuthError, Result};
use sqlx::SqlitePool;

/// Absolute session lifetime; sliding idle timeout is enforced on each request.
pub const MAX_AGE_SECS: i64 = 30 * 24 * 3600;
pub const IDLE_SECS: i64 = 7 * 24 * 3600;
pub const COOKIE: &str = "__Host-jugg_session";
/// Browsers only accept `__Host-` cookies over HTTPS; dev runs on plain http://localhost.
pub const DEV_COOKIE: &str = "jugg_session";

pub fn cookie_name(dev: bool) -> &'static str {
    if dev {
        DEV_COOKIE
    } else {
        COOKIE
    }
}

/// `Set-Cookie` value for a new session.
pub fn set_cookie(token: &str, dev: bool) -> String {
    let secure = if dev { "" } else { "; Secure" };
    format!("{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={MAX_AGE_SECS}{secure}", cookie_name(dev))
}

pub fn clear_cookie(dev: bool) -> String {
    let secure = if dev { "" } else { "; Secure" };
    format!("{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}", cookie_name(dev))
}

/// Find the session token in a `Cookie` header.
pub fn token_from_cookie_header(header: &str, dev: bool) -> Option<String> {
    let name = cookie_name(dev);
    header.split(';').find_map(|kv| kv.trim().strip_prefix(name).and_then(|r| r.strip_prefix('=')).map(str::to_string)).filter(|t| !t.is_empty())
}

pub struct Session {
    pub user_id: String,
    pub method: String,
}

/// Create a session; returns the cookie value. Cookie must be
/// `__Host-` prefixed, `Secure; HttpOnly; SameSite=Strict; Path=/`.
pub async fn create(pool: &SqlitePool, user_id: &str, method: &str, device_label: Option<&str>) -> Result<String> {
    let token = random_token();
    let now = jugg_core::now_unix();
    sqlx::query("INSERT INTO sessions (token_hash, user_id, method, created_at, last_seen_at, expires_at, device_label) VALUES (?,?,?,?,?,?,?)")
        .bind(jugg_crypto::sha256_hex(token.as_bytes()))
        .bind(user_id)
        .bind(method)
        .bind(now)
        .bind(now)
        .bind(now + MAX_AGE_SECS)
        .bind(device_label)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn validate(pool: &SqlitePool, token: &str) -> Result<Session> {
    let now = jugg_core::now_unix();
    let row: Option<(String, String)> = sqlx::query_as(
        "UPDATE sessions SET last_seen_at = ?
         WHERE token_hash = ? AND revoked_at IS NULL AND expires_at > ? AND last_seen_at > ?
         RETURNING user_id, method",
    )
    .bind(now)
    .bind(jugg_crypto::sha256_hex(token.as_bytes()))
    .bind(now)
    .bind(now - IDLE_SECS)
    .fetch_optional(pool)
    .await?;
    let (user_id, method) = row.ok_or(AuthError::Invalid)?;
    Ok(Session { user_id, method })
}

pub async fn revoke(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE token_hash = ?").bind(jugg_core::now_unix()).bind(jugg_crypto::sha256_hex(token.as_bytes())).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jugg_crypto::Keyring;

    #[tokio::test]
    async fn full_passwordless_flow() {
        let pool = jugg_db::connect_memory().await.unwrap();
        let ring = Keyring::ephemeral();
        let uid = crate::create_user(&pool, &ring, " Owner@Example.com ", jugg_core::Role::Owner).await.unwrap();
        assert_eq!(crate::find_user_by_email(&pool, &ring, "owner@example.com").await.unwrap().as_deref(), Some(uid.as_str()));

        // magic link: single use
        let token = crate::magic::issue(&pool, &uid).await.unwrap();
        assert_eq!(crate::magic::redeem(&pool, &token).await.unwrap(), uid);
        assert!(crate::magic::redeem(&pool, &token).await.is_err(), "magic link must be single use");

        // recovery codes: single use
        let codes = crate::recovery::regenerate(&pool, &uid).await.unwrap();
        assert_eq!(codes.len(), 10);
        crate::recovery::redeem(&pool, &uid, &codes[3].to_lowercase()).await.unwrap();
        assert!(crate::recovery::redeem(&pool, &uid, &codes[3]).await.is_err());

        // sessions
        let s = create(&pool, &uid, "magic_link", Some("MacBook")).await.unwrap();
        assert_eq!(validate(&pool, &s).await.unwrap().user_id, uid);
        revoke(&pool, &s).await.unwrap();
        assert!(validate(&pool, &s).await.is_err());

        // totp enrolment requires confirmation before login works
        crate::totp::begin(&pool, &ring, &uid, "Juggernaught", "owner@example.com").await.unwrap();
        assert!(crate::totp::verify(&pool, &ring, &uid, "000000").await.is_err());
    }
}
