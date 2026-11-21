//! Passkeys (WebAuthn). Biometric sign-in through the platform authenticator.
//! User verification is required, so a passkey alone satisfies "something you
//! have + something you are".

use crate::{AuthError, Result};
use jugg_crypto::Keyring;
use sqlx::SqlitePool;
use webauthn_rs::prelude::*;

pub use webauthn_rs::prelude::{Webauthn as WebauthnCtx, CreationChallengeResponse, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse};

pub fn build(rp_id: &str, origin: &str) -> Result<Webauthn> {
    let origin = Url::parse(origin).map_err(|e| AuthError::Other(e.to_string()))?;
    WebauthnBuilder::new(rp_id, &origin)
        .map(|b| b.rp_name("Juggernaught"))
        .and_then(|b| b.build())
        .map_err(|e| AuthError::Other(e.to_string()))
}

async fn load(pool: &SqlitePool, ring: &Keyring, user_id: &str) -> Result<Vec<(String, Passkey)>> {
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as("SELECT id, credential_enc FROM auth_passkeys WHERE user_id = ?").bind(user_id).fetch_all(pool).await?;
    rows.into_iter()
        .map(|(id, enc)| {
            let raw = jugg_crypto::open(ring, &enc, format!("auth_passkeys:credential:{id}").as_bytes())?;
            let pk: Passkey = serde_json::from_slice(&raw).map_err(|e| AuthError::Other(e.to_string()))?;
            Ok((id, pk))
        })
        .collect()
}

fn user_uuid(user_id: &str) -> Result<Uuid> {
    Uuid::parse_str(user_id).map_err(|e| AuthError::Other(e.to_string()))
}

/// Step 1 of registration. Store the returned state server-side (in-memory, 5 min).
pub async fn start_registration(wa: &Webauthn, pool: &SqlitePool, ring: &Keyring, user_id: &str, name: &str) -> Result<(CreationChallengeResponse, PasskeyRegistration)> {
    let existing: Vec<CredentialID> = load(pool, ring, user_id).await?.into_iter().map(|(_, p)| p.cred_id().clone()).collect();
    wa.start_passkey_registration(user_uuid(user_id)?, name, name, Some(existing)).map_err(|e| AuthError::Other(e.to_string()))
}

pub async fn finish_registration(wa: &Webauthn, pool: &SqlitePool, ring: &Keyring, user_id: &str, label: &str, resp: &RegisterPublicKeyCredential, state: &PasskeyRegistration) -> Result<()> {
    let pk = wa.finish_passkey_registration(resp, state).map_err(|_| AuthError::Invalid)?;
    let id = jugg_core::new_id();
    let raw = serde_json::to_vec(&pk).map_err(|e| AuthError::Other(e.to_string()))?;
    let enc = jugg_crypto::seal(ring, &raw, format!("auth_passkeys:credential:{id}").as_bytes())?;
    sqlx::query("INSERT INTO auth_passkeys (id, user_id, credential_enc, label, created_at) VALUES (?,?,?,?,?)")
        .bind(&id)
        .bind(user_id)
        .bind(enc)
        .bind(label)
        .bind(jugg_core::now_unix())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn start_login(wa: &Webauthn, pool: &SqlitePool, ring: &Keyring, user_id: &str) -> Result<(RequestChallengeResponse, PasskeyAuthentication)> {
    let keys: Vec<Passkey> = load(pool, ring, user_id).await?.into_iter().map(|(_, p)| p).collect();
    if keys.is_empty() {
        return Err(AuthError::Invalid);
    }
    wa.start_passkey_authentication(&keys).map_err(|e| AuthError::Other(e.to_string()))
}

pub async fn finish_login(wa: &Webauthn, pool: &SqlitePool, ring: &Keyring, user_id: &str, resp: &PublicKeyCredential, state: &PasskeyAuthentication) -> Result<()> {
    let result = wa.finish_passkey_authentication(resp, state).map_err(|_| AuthError::Invalid)?;
    // Update the signature counter / backup state on the stored credential.
    for (id, mut pk) in load(pool, ring, user_id).await? {
        if pk.update_credential(&result).is_some() {
            let raw = serde_json::to_vec(&pk).map_err(|e| AuthError::Other(e.to_string()))?;
            let enc = jugg_crypto::seal(ring, &raw, format!("auth_passkeys:credential:{id}").as_bytes())?;
            sqlx::query("UPDATE auth_passkeys SET credential_enc = ?, last_used_at = ? WHERE id = ?").bind(enc).bind(jugg_core::now_unix()).bind(&id).execute(pool).await?;
        }
    }
    Ok(())
}

/// Usernameless sign-in for a single-business box: offer every registered
/// passkey; the authenticator picks the one it holds.
pub async fn start_login_any(wa: &Webauthn, pool: &SqlitePool, ring: &Keyring) -> Result<(RequestChallengeResponse, PasskeyAuthentication)> {
    let users: Vec<String> = sqlx::query_scalar("SELECT DISTINCT user_id FROM auth_passkeys").fetch_all(pool).await?;
    let mut keys = Vec::new();
    for u in users {
        keys.extend(load(pool, ring, &u).await?.into_iter().map(|(_, p)| p));
    }
    if keys.is_empty() {
        return Err(AuthError::Invalid);
    }
    wa.start_passkey_authentication(&keys).map_err(|e| AuthError::Other(e.to_string()))
}

/// Finish usernameless sign-in; returns the user the credential belongs to.
pub async fn finish_login_any(wa: &Webauthn, pool: &SqlitePool, ring: &Keyring, resp: &PublicKeyCredential, state: &PasskeyAuthentication) -> Result<String> {
    let result = wa.finish_passkey_authentication(resp, state).map_err(|_| AuthError::Invalid)?;
    let users: Vec<String> = sqlx::query_scalar("SELECT DISTINCT user_id FROM auth_passkeys").fetch_all(pool).await?;
    for u in users {
        for (id, mut pk) in load(pool, ring, &u).await? {
            if pk.cred_id() == result.cred_id() {
                if pk.update_credential(&result).is_some() {
                    let raw = serde_json::to_vec(&pk).map_err(|e| AuthError::Other(e.to_string()))?;
                    let enc = jugg_crypto::seal(ring, &raw, format!("auth_passkeys:credential:{id}").as_bytes())?;
                    sqlx::query("UPDATE auth_passkeys SET credential_enc = ? WHERE id = ?").bind(enc).bind(&id).execute(pool).await?;
                }
                sqlx::query("UPDATE auth_passkeys SET last_used_at = ? WHERE id = ?").bind(jugg_core::now_unix()).bind(&id).execute(pool).await?;
                return Ok(u);
            }
        }
    }
    Err(AuthError::Invalid)
}

/// Labels of a user's passkeys for the settings list.
pub async fn list(pool: &SqlitePool, user_id: &str) -> Result<Vec<(String, String, i64)>> {
    Ok(sqlx::query_as("SELECT id, label, created_at FROM auth_passkeys WHERE user_id = ? ORDER BY created_at").bind(user_id).fetch_all(pool).await?)
}
