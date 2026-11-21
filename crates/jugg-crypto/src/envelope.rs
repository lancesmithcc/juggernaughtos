//! AES-256-GCM envelopes for fields and files.
//!
//! Field format: `"J1" | kek_id:u8 | nonce:12 | ciphertext+tag:16`.
//! The AAD binds a ciphertext to where it lives (`table:column:row_id`), so a
//! ciphertext copied into another row or column fails to decrypt.

use crate::keyring::{Keyring, Purpose};
use crate::{CryptoError, Result};
use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use aws_lc_rs::hmac;
use secrecy::ExposeSecret;

const MAGIC: &[u8; 2] = b"J1";
const FILE_MAGIC: &[u8; 4] = b"JF01";
/// Plaintext bytes per file chunk.
pub const CHUNK_SIZE: usize = 64 * 1024;

fn key(bytes: &[u8; 32]) -> Result<LessSafeKey> {
    Ok(LessSafeKey::new(UnboundKey::new(&AES_256_GCM, bytes)?))
}

/// Encrypt a field value. `aad` should identify the location, e.g. `b"users:email:0193…"`.
pub fn seal(ring: &Keyring, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let (kid, k) = ring.derive_active(Purpose::Field)?;
    let nonce_bytes = crate::random_bytes::<NONCE_LEN>();
    let mut buf = plaintext.to_vec();
    key(k.expose_secret())?.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce_bytes), Aad::from(aad), &mut buf)?;
    let mut out = Vec::with_capacity(3 + NONCE_LEN + buf.len());
    out.extend_from_slice(MAGIC);
    out.push(kid);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&buf);
    Ok(out)
}

/// Decrypt a value produced by [`seal`] with the same `aad`.
pub fn open(ring: &Keyring, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 3 + NONCE_LEN + 16 || &sealed[..2] != MAGIC {
        return Err(CryptoError::Tampered);
    }
    let kid = sealed[2];
    let k = ring.derive(kid, Purpose::Field)?;
    let nonce = Nonce::try_assume_unique_for_key(&sealed[3..3 + NONCE_LEN]).map_err(|_| CryptoError::Tampered)?;
    let mut buf = sealed[3 + NONCE_LEN..].to_vec();
    let plain = key(k.expose_secret())?.open_in_place(nonce, Aad::from(aad), &mut buf).map_err(|_| CryptoError::Tampered)?;
    Ok(plain.to_vec())
}

/// Deterministic HMAC-SHA256 "blind index" for equality lookups on encrypted
/// columns (e.g. find a user by email without storing the email in clear).
/// Normalise input (trim + lowercase emails) before calling.
pub fn blind_index(ring: &Keyring, value: &[u8]) -> Result<String> {
    let k = ring.derive(ring.active_id(), Purpose::BlindIndex)?;
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, k.expose_secret()), value);
    Ok(hex::encode(tag.as_ref()))
}

/// Chunked file encryption (images, documents, videos).
///
/// Each file gets a random 256-bit data key (DEK). The DEK is wrapped by the
/// KEK-derived `FileWrap` key and stored in the header, so rotating the KEK
/// only rewrites headers, never the media. Chunks use the STREAM construction:
/// nonce = 8-byte random prefix ‖ 32-bit counter, and the AAD marks the final
/// chunk, so truncation, reordering and chunk swapping are all detected.
pub struct FileCipher;

impl FileCipher {
    pub fn encrypt(ring: &Keyring, plaintext: &[u8], file_id: &str) -> Result<Vec<u8>> {
        let dek = crate::random_bytes::<32>();
        let (kid, wrap) = ring.derive_active(Purpose::FileWrap)?;
        let mut wrapped = dek.to_vec();
        let wrap_nonce = crate::random_bytes::<NONCE_LEN>();
        key(wrap.expose_secret())?.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(wrap_nonce),
            Aad::from(file_id.as_bytes()),
            &mut wrapped,
        )?;
        let prefix = crate::random_bytes::<8>();

        let mut out = Vec::with_capacity(plaintext.len() + plaintext.len() / CHUNK_SIZE * 16 + 128);
        out.extend_from_slice(FILE_MAGIC);
        out.push(kid);
        out.extend_from_slice(&wrap_nonce);
        out.extend_from_slice(&wrapped); // 32 + 16
        out.extend_from_slice(&prefix);

        let dek_key = key(&dek)?;
        let chunks: Vec<&[u8]> = if plaintext.is_empty() { vec![&[][..]] } else { plaintext.chunks(CHUNK_SIZE).collect() };
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut buf = chunk.to_vec();
            dek_key.seal_in_place_append_tag(chunk_nonce(&prefix, i as u32), chunk_aad(file_id, i == last), &mut buf)?;
            out.extend_from_slice(&buf);
        }
        Ok(out)
    }

    pub fn decrypt(ring: &Keyring, data: &[u8], file_id: &str) -> Result<Vec<u8>> {
        const HDR: usize = 4 + 1 + NONCE_LEN + 48 + 8;
        if data.len() < HDR + 16 || &data[..4] != FILE_MAGIC {
            return Err(CryptoError::Tampered);
        }
        let kid = data[4];
        let wrap = ring.derive(kid, Purpose::FileWrap)?;
        let wrap_nonce = Nonce::try_assume_unique_for_key(&data[5..5 + NONCE_LEN]).map_err(|_| CryptoError::Tampered)?;
        let mut wrapped = data[5 + NONCE_LEN..5 + NONCE_LEN + 48].to_vec();
        let dek = key(wrap.expose_secret())?
            .open_in_place(wrap_nonce, Aad::from(file_id.as_bytes()), &mut wrapped)
            .map_err(|_| CryptoError::Tampered)?;
        let dek: [u8; 32] = dek.try_into().map_err(|_| CryptoError::Tampered)?;
        let mut prefix = [0u8; 8];
        prefix.copy_from_slice(&data[HDR - 8..HDR]);

        let dek_key = key(&dek)?;
        let body = &data[HDR..];
        let chunks: Vec<&[u8]> = body.chunks(CHUNK_SIZE + 16).collect();
        let last = chunks.len() - 1;
        let mut out = Vec::with_capacity(body.len());
        for (i, chunk) in chunks.into_iter().enumerate() {
            let mut buf = chunk.to_vec();
            let plain = dek_key
                .open_in_place(chunk_nonce(&prefix, i as u32), chunk_aad(file_id, i == last), &mut buf)
                .map_err(|_| CryptoError::Tampered)?;
            out.extend_from_slice(plain);
        }
        Ok(out)
    }
}

fn chunk_nonce(prefix: &[u8; 8], counter: u32) -> Nonce {
    let mut n = [0u8; NONCE_LEN];
    n[..8].copy_from_slice(prefix);
    n[8..].copy_from_slice(&counter.to_be_bytes());
    Nonce::assume_unique_for_key(n)
}

fn chunk_aad(file_id: &str, last: bool) -> Aad<Vec<u8>> {
    let mut a = file_id.as_bytes().to_vec();
    a.push(if last { 1 } else { 0 });
    Aad::from(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_roundtrip_and_aad_binding() {
        let ring = Keyring::ephemeral();
        let ct = seal(&ring, b"owner@example.com", b"users:email:1").unwrap();
        assert_eq!(open(&ring, &ct, b"users:email:1").unwrap(), b"owner@example.com");
        assert!(open(&ring, &ct, b"users:email:2").is_err(), "moved ciphertext must fail");
        let mut bad = ct.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&ring, &bad, b"users:email:1").is_err(), "tamper must fail");
    }

    #[test]
    fn blind_index_is_deterministic() {
        let ring = Keyring::ephemeral();
        assert_eq!(blind_index(&ring, b"a@b.c").unwrap(), blind_index(&ring, b"a@b.c").unwrap());
        assert_ne!(blind_index(&ring, b"a@b.c").unwrap(), blind_index(&ring, b"x@b.c").unwrap());
    }

    #[test]
    fn file_roundtrip_multi_chunk_and_truncation() {
        let ring = Keyring::ephemeral();
        let data: Vec<u8> = (0..(CHUNK_SIZE * 2 + 123)).map(|i| (i % 251) as u8).collect();
        let ct = FileCipher::encrypt(&ring, &data, "asset-1").unwrap();
        assert_eq!(FileCipher::decrypt(&ring, &ct, "asset-1").unwrap(), data);
        assert!(FileCipher::decrypt(&ring, &ct, "asset-2").is_err());
        // drop the final chunk: the new last chunk lacks the "final" AAD flag
        let truncated = &ct[..ct.len() - (123 + 16)];
        assert!(FileCipher::decrypt(&ring, truncated, "asset-1").is_err(), "truncation must fail");
        assert_eq!(FileCipher::decrypt(&ring, &FileCipher::encrypt(&ring, b"", "e").unwrap(), "e").unwrap(), b"");
    }
}
