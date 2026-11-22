//! Juggernaught crypto. One rule: **symmetric for data, post-quantum for keys.**
//!
//! * Data at rest (fields, files): AES-256-GCM with keys derived from the box KEK.
//! * Keys that leave the box (backups, device enrolment): ML-KEM-768 + X25519
//!   hybrid wrap, so a break of either primitive alone does not expose data.
//! * Things that must be provably ours (update manifests, audit checkpoints,
//!   backup manifests): ML-DSA-65 signatures.
//!
//! All primitives come from `aws-lc-rs` (the same library rustls uses for
//! X25519MLKEM768 TLS), so there is a single audited crypto provider in the binary.

mod envelope;
mod hybrid;
mod keyring;
mod sign;
mod stream;

pub use envelope::{blind_index, open, seal, FileCipher, CHUNK_SIZE};
pub use hybrid::{HybridPublicKey, HybridSecretKey, HybridWrapped};
pub use keyring::{Keyring, Purpose};
pub use sign::{verify as verify_signature, Signer, ML_DSA_65_PUBLIC_KEY_LEN};
pub use stream::{StreamOpener, StreamSealer};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("crypto operation failed")]
    Failed,
    #[error("ciphertext is malformed or was tampered with")]
    Tampered,
    #[error("unknown key id {0}")]
    UnknownKey(u8),
    #[error("key store: {0}")]
    KeyStore(String),
}

impl From<aws_lc_rs::error::Unspecified> for CryptoError {
    fn from(_: aws_lc_rs::error::Unspecified) -> Self {
        CryptoError::Failed
    }
}

impl From<aws_lc_rs::error::KeyRejected> for CryptoError {
    fn from(e: aws_lc_rs::error::KeyRejected) -> Self {
        CryptoError::KeyStore(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, CryptoError>;

/// Cryptographically secure random bytes.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    aws_lc_rs::rand::fill(&mut out).expect("system RNG failed");
    out
}

/// SHA-256 hex digest (artifact checksums, token hashes).
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, data))
}

/// Constant-time equality.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    aws_lc_rs::constant_time::verify_slices_are_equal(a, b).is_ok()
}
