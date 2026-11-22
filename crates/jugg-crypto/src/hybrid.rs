//! Hybrid post-quantum key wrapping: ML-KEM-768 + X25519.
//!
//! Used when a key must leave the box: encrypted off-box backups are wrapped to
//! the owner's recovery key, and new devices / a second box receive keys this way.
//! The wrapping key is HKDF(ss_mlkem ‖ ss_x25519, transcript), so an attacker
//! must break **both** ML-KEM-768 and X25519 to unwrap.

use crate::{CryptoError, Result};
use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use aws_lc_rs::agreement::{self, PrivateKey, UnparsedPublicKey, X25519};
use aws_lc_rs::encoding::AsBigEndian;
use aws_lc_rs::hkdf::{Salt, HKDF_SHA256};
use aws_lc_rs::kem::{Ciphertext, DecapsulationKey, EncapsulationKey, ML_KEM_768};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HybridPublicKey {
    #[serde(with = "b64")]
    pub mlkem768: Vec<u8>,
    #[serde(with = "b64")]
    pub x25519: Vec<u8>,
}

pub struct HybridSecretKey {
    mlkem: DecapsulationKey,
    x25519: PrivateKey,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HybridWrapped {
    #[serde(with = "b64")]
    pub mlkem_ct: Vec<u8>,
    #[serde(with = "b64")]
    pub x25519_eph: Vec<u8>,
    #[serde(with = "b64")]
    pub nonce: Vec<u8>,
    #[serde(with = "b64")]
    pub wrapped: Vec<u8>,
}

impl HybridSecretKey {
    pub fn generate() -> Result<(Self, HybridPublicKey)> {
        let mlkem = DecapsulationKey::generate(&ML_KEM_768)?;
        let x25519 = PrivateKey::generate(&X25519)?;
        let public = HybridPublicKey {
            mlkem768: mlkem.encapsulation_key()?.key_bytes()?.as_ref().to_vec(),
            x25519: x25519.compute_public_key()?.as_ref().to_vec(),
        };
        Ok((Self { mlkem, x25519 }, public))
    }

    /// Raw secret material for export to an offline recovery file (owner stores it).
    pub fn to_bytes(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let seed: aws_lc_rs::encoding::Curve25519SeedBin<'static> = self.x25519.as_be_bytes()?;
        Ok((self.mlkem.key_bytes()?.as_ref().to_vec(), seed.as_ref().to_vec()))
    }

    pub fn from_bytes(mlkem: &[u8], x25519_seed: &[u8]) -> Result<Self> {
        Ok(Self { mlkem: DecapsulationKey::new(&ML_KEM_768, mlkem)?, x25519: PrivateKey::from_private_key(&X25519, x25519_seed)? })
    }

    pub fn unwrap_key(&self, w: &HybridWrapped, context: &[u8]) -> Result<Vec<u8>> {
        let ss_kem = self.mlkem.decapsulate(Ciphertext::from(w.mlkem_ct.as_slice())).map_err(|_| CryptoError::Tampered)?;
        let ss_x = agreement::agree(&self.x25519, &UnparsedPublicKey::new(&X25519, &w.x25519_eph), CryptoError::Tampered, |s| Ok(s.to_vec()))?;
        let kek = combine(ss_kem.as_ref(), &ss_x, &w.mlkem_ct, &w.x25519_eph, context)?;
        let nonce = Nonce::try_assume_unique_for_key(&w.nonce).map_err(|_| CryptoError::Tampered)?;
        let mut buf = w.wrapped.clone();
        let plain = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &kek)?)
            .open_in_place(nonce, Aad::from(context), &mut buf)
            .map_err(|_| CryptoError::Tampered)?;
        Ok(plain.to_vec())
    }
}

impl HybridPublicKey {
    /// Wrap a data key (or any small secret) to this recipient.
    pub fn wrap_key(&self, key: &[u8], context: &[u8]) -> Result<HybridWrapped> {
        let ek = EncapsulationKey::new(&ML_KEM_768, &self.mlkem768)?;
        let (ct, ss_kem) = ek.encapsulate()?;
        let eph = PrivateKey::generate(&X25519)?;
        let eph_pub = eph.compute_public_key()?.as_ref().to_vec();
        let ss_x = agreement::agree(&eph, &UnparsedPublicKey::new(&X25519, &self.x25519), CryptoError::Failed, |s| Ok(s.to_vec()))?;
        let kek = combine(ss_kem.as_ref(), &ss_x, ct.as_ref(), &eph_pub, context)?;
        let nonce = crate::random_bytes::<NONCE_LEN>();
        let mut buf = key.to_vec();
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &kek)?).seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(context),
            &mut buf,
        )?;
        Ok(HybridWrapped { mlkem_ct: ct.as_ref().to_vec(), x25519_eph: eph_pub, nonce: nonce.to_vec(), wrapped: buf })
    }
}

fn combine(ss_kem: &[u8], ss_x: &[u8], ct: &[u8], eph: &[u8], context: &[u8]) -> Result<[u8; 32]> {
    let mut ikm = Vec::with_capacity(64);
    ikm.extend_from_slice(ss_kem);
    ikm.extend_from_slice(ss_x);
    let prk = Salt::new(HKDF_SHA256, b"jugg/v1/hybrid-kem").extract(&ikm);
    let info: [&[u8]; 3] = [ct, eph, context];
    let okm = prk.expand(&info, Len32)?;
    let mut out = [0u8; 32];
    okm.fill(&mut out)?;
    Ok(out)
}

struct Len32;
impl aws_lc_rs::hkdf::KeyType for Len32 {
    fn len(&self) -> usize {
        32
    }
}

mod b64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        STANDARD.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_unwrap_and_context_binding() {
        let (sk, pk) = HybridSecretKey::generate().unwrap();
        let dek = crate::random_bytes::<32>();
        let w = pk.wrap_key(&dek, b"backup:2026-11-21").unwrap();
        assert_eq!(sk.unwrap_key(&w, b"backup:2026-11-21").unwrap(), dek);
        assert!(sk.unwrap_key(&w, b"backup:other").is_err());

        let (other, _) = HybridSecretKey::generate().unwrap();
        assert!(other.unwrap_key(&w, b"backup:2026-11-21").is_err(), "wrong recipient must fail");

        let (m, x) = sk.to_bytes().unwrap();
        let restored = HybridSecretKey::from_bytes(&m, &x).unwrap();
        assert_eq!(restored.unwrap_key(&w, b"backup:2026-11-21").unwrap(), dek);
    }
}
