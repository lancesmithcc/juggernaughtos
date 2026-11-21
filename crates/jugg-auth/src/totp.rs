use crate::{AuthError, Result};
use jugg_crypto::Keyring;
use sqlx::SqlitePool;
use totp_rs::{Algorithm, Secret, TOTP};

fn build(secret: Vec<u8>, issuer: &str, account: &str) -> Result<TOTP> {
    TOTP::new(Algorithm::SHA1, 6, 1, 30, secret, Some(issuer.to_string()), account.to_string()).map_err(|e| AuthError::Other(e.to_string()))
}

pub struct Enrolment {
    /// `otpauth://` URL for the QR code.
    pub url: String,
    /// Base64 PNG QR code, shown once.
    pub qr_png_base64: String,
}

/// Start TOTP enrolment; it is not active until [`confirm`] succeeds.
pub async fn begin(pool: &SqlitePool, ring: &Keyring, user_id: &str, issuer: &str, account: &str) -> Result<Enrolment> {
    let secret = Secret::generate_secret().to_bytes().map_err(|e| AuthError::Other(e.to_string()))?;
    let totp = build(secret.clone(), issuer, account)?;
    let enc = jugg_crypto::seal(ring, &secret, format!("auth_totp:secret:{user_id}").as_bytes())?;
    sqlx::query("INSERT INTO auth_totp (user_id, secret_enc) VALUES (?, ?) ON CONFLICT(user_id) DO UPDATE SET secret_enc = excluded.secret_enc, confirmed_at = NULL, last_step = 0")
        .bind(user_id)
        .bind(enc)
        .execute(pool)
        .await?;
    Ok(Enrolment { url: totp.get_url(), qr_png_base64: totp.get_qr_base64().map_err(AuthError::Other)? })
}

pub async fn confirm(pool: &SqlitePool, ring: &Keyring, user_id: &str, code: &str) -> Result<()> {
    verify_inner(pool, ring, user_id, code, true).await
}

/// Verify a login code. Rejects reuse of the same or an earlier time step.
pub async fn verify(pool: &SqlitePool, ring: &Keyring, user_id: &str, code: &str) -> Result<()> {
    verify_inner(pool, ring, user_id, code, false).await
}

async fn verify_inner(pool: &SqlitePool, ring: &Keyring, user_id: &str, code: &str, confirming: bool) -> Result<()> {
    let row: Option<(Vec<u8>, Option<i64>, i64)> =
        sqlx::query_as("SELECT secret_enc, confirmed_at, last_step FROM auth_totp WHERE user_id = ?").bind(user_id).fetch_optional(pool).await?;
    let (enc, confirmed, last_step) = row.ok_or(AuthError::Invalid)?;
    if confirmed.is_none() && !confirming {
        return Err(AuthError::Invalid);
    }
    let secret = jugg_crypto::open(ring, &enc, format!("auth_totp:secret:{user_id}").as_bytes())?;
    let totp = build(secret, "Juggernaught", user_id)?;
    let now = jugg_core::now_unix() as u64;
    // Accept current step ±1 (clock skew), each step at most once.
    for skew in [0i64, -1, 1] {
        let t = (now as i64 + skew * 30) as u64;
        let step = (t / 30) as i64;
        if step > last_step && jugg_crypto::ct_eq(totp.generate(t).as_bytes(), code.trim().as_bytes()) {
            sqlx::query("UPDATE auth_totp SET last_step = ?, confirmed_at = COALESCE(confirmed_at, ?) WHERE user_id = ?")
                .bind(step)
                .bind(jugg_core::now_unix())
                .bind(user_id)
                .execute(pool)
                .await?;
            return Ok(());
        }
    }
    Err(AuthError::Invalid)
}
