use crate::{CryptoError, Result};
use aws_lc_rs::hkdf::{Salt, HKDF_SHA256};
use secrecy::{ExposeSecret, SecretBox};
use std::path::{Path, PathBuf};

/// What a derived key is for. Each purpose gets an independent key so a
/// compromise of one use (say a leaked blind index) says nothing about another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Field-level encryption of SQLite columns.
    Field,
    /// HMAC blind indexes so encrypted columns stay searchable by equality.
    BlindIndex,
    /// Wrapping per-file data keys.
    FileWrap,
    /// Session / magic-link token MACs.
    Token,
}

impl Purpose {
    fn label(self) -> &'static [u8] {
        match self {
            Purpose::Field => b"jugg/v1/field",
            Purpose::BlindIndex => b"jugg/v1/blind-index",
            Purpose::FileWrap => b"jugg/v1/file-wrap",
            Purpose::Token => b"jugg/v1/token",
        }
    }
}

struct Kek(SecretBox<[u8; 32]>);

/// The box key-encryption key (KEK) plus everything derived from it.
///
/// The KEK lives **outside SQLite** in `keys_dir/kek-<id>.key` (mode 0600, owned by the
/// `jugg` service user). Rotation adds a new KEK id; old ids stay readable until
/// a re-encryption sweep finishes. Optional hardening (phase 5): seal the KEK
/// with TPM2 or require an unlock passphrase at boot (Argon2id-derived wrap).
pub struct Keyring {
    dir: PathBuf,
    active: u8,
    keks: Vec<(u8, Kek)>,
}

impl Keyring {
    /// Load every KEK, creating KEK 1 on first boot. Sealed installs get their
    /// keys from systemd credentials (`$CREDENTIALS_DIRECTORY/kek-<n>`, decrypted
    /// by systemd with TPM2 when present); plaintext key files are then gone.
    pub fn load_or_create(dir: impl AsRef<Path>) -> Result<Self> {
        let creds = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
        Self::load(dir, creds.as_deref())
    }

    pub fn load(dir: impl AsRef<Path>, creds: Option<&Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let mut from_creds = Vec::new();
        if let Some(c) = creds {
            for entry in std::fs::read_dir(c).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(id) = name.strip_prefix("kek-").and_then(|s| s.parse::<u8>().ok()) {
                    let bytes = std::fs::read(entry.path()).map_err(|e| CryptoError::KeyStore(e.to_string()))?;
                    let arr: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::KeyStore(format!("credential {name} is not 32 bytes")))?;
                    from_creds.push((id, Kek(SecretBox::new(Box::new(arr)))));
                }
            }
        }
        if !from_creds.is_empty() {
            from_creds.sort_by_key(|(id, _)| *id);
            let active = from_creds.last().map(|(id, _)| *id).unwrap_or(1);
            return Ok(Self { dir, active, keks: from_creds });
        }
        if is_sealed(&dir) {
            return Err(CryptoError::KeyStore(
                "the encryption keys are sealed (systemd-creds); run this through its systemd service, e.g. `systemctl start juggernaught-backup`".into(),
            ));
        }
        std::fs::create_dir_all(&dir).map_err(|e| CryptoError::KeyStore(e.to_string()))?;
        restrict_dir(&dir)?;

        let mut keks = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|e| CryptoError::KeyStore(e.to_string()))? {
            let entry = entry.map_err(|e| CryptoError::KeyStore(e.to_string()))?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_prefix("kek-").and_then(|s| s.strip_suffix(".key")) {
                let id: u8 = id.parse().map_err(|_| CryptoError::KeyStore(format!("bad key file {name}")))?;
                let bytes = std::fs::read(entry.path()).map_err(|e| CryptoError::KeyStore(e.to_string()))?;
                let arr: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::KeyStore(format!("{name} is not 32 bytes")))?;
                keks.push((id, Kek(SecretBox::new(Box::new(arr)))));
            }
        }
        keks.sort_by_key(|(id, _)| *id);

        let mut ring = Self { dir, active: 0, keks };
        if ring.keks.is_empty() {
            ring.rotate()?;
        } else {
            ring.active = ring.keks.last().map(|(id, _)| *id).unwrap_or(1);
        }
        Ok(ring)
    }

    /// In-memory keyring for tests and dev.
    pub fn ephemeral() -> Self {
        Self { dir: PathBuf::new(), active: 1, keks: vec![(1, Kek(SecretBox::new(Box::new(crate::random_bytes::<32>()))))] }
    }

    /// Create a new KEK and make it active. Returns the new id.
    pub fn rotate(&mut self) -> Result<u8> {
        let id = self.keks.last().map(|(id, _)| id + 1).unwrap_or(1);
        let key = crate::random_bytes::<32>();
        if !self.dir.as_os_str().is_empty() {
            let path = self.dir.join(format!("kek-{id}.key"));
            write_secret_file(&path, &key)?;
        }
        self.keks.push((id, Kek(SecretBox::new(Box::new(key)))));
        self.active = id;
        Ok(id)
    }

    pub fn active_id(&self) -> u8 {
        self.active
    }

    /// Raw KEK material for escrow into an encrypted backup (wrapped to the
    /// owner's offline recovery key). Never log or persist the result in clear.
    pub fn export_for_escrow(&self) -> Vec<(u8, zeroize::Zeroizing<[u8; 32]>)> {
        self.keks.iter().map(|(id, k)| (*id, zeroize::Zeroizing::new(*k.0.expose_secret()))).collect()
    }

    /// Recreate a key directory from escrowed KEKs (disaster restore onto a new box).
    pub fn restore_from_escrow(dir: impl AsRef<Path>, keks: &[(u8, [u8; 32])]) -> Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| CryptoError::KeyStore(e.to_string()))?;
        restrict_dir(dir)?;
        for (id, k) in keks {
            write_secret_file(&dir.join(format!("kek-{id}.key")), k)?;
        }
        Self::load_or_create(dir)
    }

    /// Wrap a data key under the box's file-wrap key (KEK-derived, AES-256-GCM).
    pub fn wrap_dek(&self, dek: &[u8; 32], context: &[u8]) -> Result<(u8, Vec<u8>)> {
        use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
        let (kid, k) = self.derive_active(Purpose::FileWrap)?;
        let nonce = crate::random_bytes::<12>();
        let mut buf = dek.to_vec();
        LessSafeKey::new(UnboundKey::new(&AES_256_GCM, k.expose_secret())?).seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(context), &mut buf)?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&buf);
        Ok((kid, out))
    }

    pub fn unwrap_dek(&self, kid: u8, wrapped: &[u8], context: &[u8]) -> Result<[u8; 32]> {
        use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
        if wrapped.len() != 12 + 32 + 16 {
            return Err(CryptoError::Tampered);
        }
        let k = self.derive(kid, Purpose::FileWrap)?;
        let nonce = Nonce::try_assume_unique_for_key(&wrapped[..12]).map_err(|_| CryptoError::Tampered)?;
        let mut buf = wrapped[12..].to_vec();
        let plain = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, k.expose_secret())?).open_in_place(nonce, Aad::from(context), &mut buf).map_err(|_| CryptoError::Tampered)?;
        plain.try_into().map_err(|_| CryptoError::Tampered)
    }

    /// Derive a 32-byte purpose key from KEK `id` via HKDF-SHA256.
    pub fn derive(&self, id: u8, purpose: Purpose) -> Result<SecretBox<[u8; 32]>> {
        let kek = self.keks.iter().find(|(k, _)| *k == id).map(|(_, k)| k).ok_or(CryptoError::UnknownKey(id))?;
        let salt = Salt::new(HKDF_SHA256, b"juggernaught-os");
        let prk = salt.extract(kek.0.expose_secret());
        let info = [purpose.label()];
        let okm = prk.expand(&info, HkdfLen(32))?;
        let mut out = [0u8; 32];
        okm.fill(&mut out)?;
        Ok(SecretBox::new(Box::new(out)))
    }

    pub(crate) fn derive_active(&self, purpose: Purpose) -> Result<(u8, SecretBox<[u8; 32]>)> {
        Ok((self.active, self.derive(self.active, purpose)?))
    }
}

struct HkdfLen(usize);
impl aws_lc_rs::hkdf::KeyType for HkdfLen {
    fn len(&self) -> usize {
        self.0
    }
}

/// Marker written by `juggernaught keys seal` once plaintext keys are destroyed.
pub const SEALED_MARKER: &str = ".sealed";

pub fn is_sealed(dir: &Path) -> bool {
    dir.join(SEALED_MARKER).exists()
}

pub(crate) fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| CryptoError::KeyStore(format!("{}: {e}", path.display())))?;
    f.write_all(bytes).map_err(|e| CryptoError::KeyStore(e.to_string()))?;
    f.sync_all().map_err(|e| CryptoError::KeyStore(e.to_string()))
}

fn restrict_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| CryptoError::KeyStore(e.to_string()))?;
    }
    let _ = dir;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_keys_come_from_credentials_only() {
        let dir = tempfile::tempdir().unwrap();
        let creds = tempfile::tempdir().unwrap();
        let k = *Keyring::load(dir.path(), None).unwrap().derive(1, Purpose::Field).unwrap().expose_secret();
        // Seal: move the key into the credentials dir, destroy plaintext, mark sealed.
        std::fs::rename(dir.path().join("kek-1.key"), creds.path().join("kek-1")).unwrap();
        std::fs::write(dir.path().join(SEALED_MARKER), b"").unwrap();
        let ring = Keyring::load(dir.path(), Some(creds.path())).unwrap();
        assert_eq!(*ring.derive(1, Purpose::Field).unwrap().expose_secret(), k, "same key via credentials");
        // Without credentials a sealed box must refuse, never mint a fresh key.
        assert!(Keyring::load(dir.path(), None).is_err());
        assert!(!dir.path().join("kek-1.key").exists());
    }

    #[test]
    fn persists_and_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let k1 = {
            let ring = Keyring::load_or_create(dir.path()).unwrap();
            assert_eq!(ring.active_id(), 1);
            *ring.derive(1, Purpose::Field).unwrap().expose_secret()
        };
        let mut ring = Keyring::load_or_create(dir.path()).unwrap();
        assert_eq!(*ring.derive(1, Purpose::Field).unwrap().expose_secret(), k1, "KEK must persist");
        assert_ne!(
            *ring.derive(1, Purpose::Field).unwrap().expose_secret(),
            *ring.derive(1, Purpose::BlindIndex).unwrap().expose_secret(),
            "purposes must be independent"
        );
        assert_eq!(ring.rotate().unwrap(), 2);
        assert_eq!(Keyring::load_or_create(dir.path()).unwrap().active_id(), 2);
    }
}
