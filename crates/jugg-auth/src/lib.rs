//! Passwordless authentication. There is no password column anywhere.
//!
//! Owners pick any combination of:
//! * **Magic link** — emailed single-use link, 15 min, token stored only as SHA-256.
//! * **Passkey** — WebAuthn platform authenticator (Face ID / Touch ID / Windows Hello / security key).
//! * **Authenticator app** — RFC 6238 TOTP with replay protection.
//!
//! Plus 10 single-use **recovery codes** (Argon2id-hashed) shown once at setup.
//! Policy: an owner must keep at least two methods (or one + recovery codes) so a
//! lost phone never locks the business out.

pub mod magic;
#[cfg(feature = "passkeys")]
pub mod passkey;
pub mod ratelimit;
pub mod recovery;
pub mod session;
pub mod totp;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("link or code is invalid or expired")]
    Invalid,
    #[error("too many attempts; try again later")]
    RateLimited,
    #[error(transparent)]
    Db(#[from] jugg_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Crypto(#[from] jugg_crypto::CryptoError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AuthError>;

/// URL-safe random token (256 bits).
pub(crate) fn random_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(jugg_crypto::random_bytes::<32>())
}

/// Normalise an email for blind indexing.
pub fn normalise_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Create a user (first-run wizard creates the owner).
pub async fn create_user(pool: &sqlx::SqlitePool, ring: &jugg_crypto::Keyring, email: &str, role: jugg_core::Role) -> Result<String> {
    let id = jugg_core::new_id();
    let email = normalise_email(email);
    let bidx = jugg_crypto::blind_index(ring, email.as_bytes())?;
    let enc = jugg_crypto::seal(ring, email.as_bytes(), format!("users:email:{id}").as_bytes())?;
    let role = match role {
        jugg_core::Role::Owner => "owner",
        jugg_core::Role::Editor => "editor",
        jugg_core::Role::Viewer => "viewer",
    };
    sqlx::query("INSERT INTO users (id, email_enc, email_bidx, role, created_at) VALUES (?,?,?,?,?)")
        .bind(&id)
        .bind(enc)
        .bind(bidx)
        .bind(role)
        .bind(jugg_core::now_unix())
        .execute(pool)
        .await?;
    Ok(id)
}

/// Change a user's sign-in email (admin command on the box). Fails if another
/// active user already has it.
pub async fn set_email(pool: &sqlx::SqlitePool, ring: &jugg_crypto::Keyring, user_id: &str, email: &str) -> Result<()> {
    let email = normalise_email(email);
    let ok = email.len() <= 254 && email.split_once('@').is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')) && !email.contains(char::is_whitespace);
    if !ok {
        return Err(AuthError::Other("that doesn't look like an email address".into()));
    }
    if let Some(other) = find_user_by_email(pool, ring, &email).await? {
        if other != user_id {
            return Err(AuthError::Other("another user already signs in with that email".into()));
        }
    }
    let bidx = jugg_crypto::blind_index(ring, email.as_bytes())?;
    let enc = jugg_crypto::seal(ring, email.as_bytes(), format!("users:email:{user_id}").as_bytes())?;
    let n = sqlx::query("UPDATE users SET email_enc = ?, email_bidx = ? WHERE id = ?").bind(enc).bind(bidx).bind(user_id).execute(pool).await?.rows_affected();
    if n == 0 {
        return Err(AuthError::Other("no such user".into()));
    }
    Ok(())
}

pub async fn find_user_by_email(pool: &sqlx::SqlitePool, ring: &jugg_crypto::Keyring, email: &str) -> Result<Option<String>> {
    let bidx = jugg_crypto::blind_index(ring, normalise_email(email).as_bytes())?;
    Ok(sqlx::query_scalar("SELECT id FROM users WHERE email_bidx = ? AND disabled_at IS NULL").bind(bidx).fetch_optional(pool).await?)
}
