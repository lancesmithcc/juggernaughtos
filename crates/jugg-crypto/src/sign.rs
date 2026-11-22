//! ML-DSA-65 signatures for update manifests, audit-log checkpoints and backups.

use crate::{CryptoError, Result};
use aws_lc_rs::signature::{KeyPair, PqdsaKeyPair, UnparsedPublicKey, ML_DSA_65, ML_DSA_65_SIGNING};
use std::path::Path;

pub const ML_DSA_65_PUBLIC_KEY_LEN: usize = 1952;

pub struct Signer {
    kp: PqdsaKeyPair,
}

impl Signer {
    pub fn generate() -> Result<Self> {
        Ok(Self { kp: PqdsaKeyPair::generate(&ML_DSA_65_SIGNING)? })
    }

    /// Load `path` (PKCS#8 DER) or create it (mode 0600) on first use. Sealed
    /// installs read it from the systemd credential `signing` instead.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if let Some(c) = std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from) {
            if let Ok(der) = std::fs::read(c.join("signing")) {
                return Ok(Self { kp: PqdsaKeyPair::from_pkcs8(&ML_DSA_65_SIGNING, &der)? });
            }
        }
        if path.parent().is_some_and(crate::keyring::is_sealed) && !path.exists() {
            return Err(CryptoError::KeyStore("the signing key is sealed (systemd-creds); run through the service".into()));
        }
        if path.exists() {
            let der = std::fs::read(path).map_err(|e| CryptoError::KeyStore(e.to_string()))?;
            return Ok(Self { kp: PqdsaKeyPair::from_pkcs8(&ML_DSA_65_SIGNING, &der)? });
        }
        let s = Self::generate()?;
        let der = s.kp.to_pkcs8v1()?;
        crate::keyring::write_secret_file(path, der.as_ref())?;
        Ok(s)
    }

    pub fn public_key(&self) -> Vec<u8> {
        self.kp.public_key().as_ref().to_vec()
    }

    pub fn sign(&self, msg: &[u8]) -> Result<Vec<u8>> {
        let mut sig = vec![0u8; ML_DSA_65_SIGNING.signature_len()];
        let n = self.kp.sign(msg, &mut sig)?;
        sig.truncate(n);
        Ok(sig)
    }
}

pub fn verify(public_key: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    UnparsedPublicKey::new(&ML_DSA_65, public_key).verify(msg, sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_persist() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sign.pk8");
        let s = Signer::load_or_create(&p).unwrap();
        let pk = s.public_key();
        assert_eq!(pk.len(), ML_DSA_65_PUBLIC_KEY_LEN);
        let sig = s.sign(b"manifest").unwrap();
        assert!(verify(&pk, b"manifest", &sig));
        assert!(!verify(&pk, b"manifest!", &sig));
        let again = Signer::load_or_create(&p).unwrap();
        assert_eq!(again.public_key(), pk);
    }
}
