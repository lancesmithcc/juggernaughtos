use crate::{AuthError, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use sqlx::SqlitePool;

pub const CODE_COUNT: usize = 10;

/// Generate fresh recovery codes (replacing old ones). Shown to the owner once.
/// Format `XXXXX-XXXXX` from an unambiguous alphabet (~50 bits each).
pub async fn regenerate(pool: &SqlitePool, user_id: &str) -> Result<Vec<String>> {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut codes = Vec::with_capacity(CODE_COUNT);
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM auth_recovery_codes WHERE user_id = ?").bind(user_id).execute(&mut *tx).await?;
    for _ in 0..CODE_COUNT {
        let raw = jugg_crypto::random_bytes::<10>();
        let chars: String = raw.iter().map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char).collect();
        let code = format!("{}-{}", &chars[..5], &chars[5..]);
        let hash = Argon2::default()
            .hash_password(code.as_bytes(), &SaltString::encode_b64(&jugg_crypto::random_bytes::<16>()).map_err(|e| AuthError::Other(e.to_string()))?)
            .map_err(|e| AuthError::Other(e.to_string()))?
            .to_string();
        sqlx::query("INSERT INTO auth_recovery_codes (id, user_id, code_hash) VALUES (?,?,?)")
            .bind(jugg_core::new_id())
            .bind(user_id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        codes.push(code);
    }
    tx.commit().await?;
    Ok(codes)
}

pub async fn redeem(pool: &SqlitePool, user_id: &str, code: &str) -> Result<()> {
    let code = code.trim().to_uppercase();
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, code_hash FROM auth_recovery_codes WHERE user_id = ? AND used_at IS NULL").bind(user_id).fetch_all(pool).await?;
    for (id, hash) in rows {
        let parsed = PasswordHash::new(&hash).map_err(|e| AuthError::Other(e.to_string()))?;
        if Argon2::default().verify_password(code.as_bytes(), &parsed).is_ok() {
            let n = sqlx::query("UPDATE auth_recovery_codes SET used_at = ? WHERE id = ? AND used_at IS NULL")
                .bind(jugg_core::now_unix())
                .bind(&id)
                .execute(pool)
                .await?
                .rows_affected();
            return if n == 1 { Ok(()) } else { Err(AuthError::Invalid) };
        }
    }
    Err(AuthError::Invalid)
}
