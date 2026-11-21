//! Updates without fear.
//!
//! 1. **Fetch** `manifest.json` + `manifest.sig` for the channel.
//! 2. **Verify** the ML-DSA-65 signature against the release public key baked
//!    into the binary (and any rotated keys it lists), then each artifact's
//!    SHA-256 and size. Downgrades are refused unless explicitly allowed.
//! 3. **Stage** into `releases/<component>/<version>/` (never over the live copy).
//! 4. **Canary**: run the component's smoke test against golden fixtures
//!    (render a known page, trace a known logo, speak a known sentence…).
//! 5. **Promote** by atomically swapping the `current` symlink; restart.
//! 6. **Watch**: health checks for 10 minutes; any failure → **roll back** to
//!    the previous symlink target and open an incident.
//!
//! Third-party tools are not signed by their authors, so the Juggernaught
//! release pipeline builds them reproducibly from the pinned upstream tag,
//! records commit + artifact hash in the manifest and signs that. Boxes only
//! ever install what our manifest vouches for.

use jugg_crypto::{sha256_hex, verify_signature};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("manifest signature is invalid")]
    BadSignature,
    #[error("artifact {0} failed checksum")]
    BadChecksum(String),
    #[error("refusing downgrade of {component}: {from} → {to}")]
    Downgrade { component: String, from: String, to: String },
    #[error("canary failed: {0}")]
    CanaryFailed(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub channel: String,
    pub issued_at: i64,
    /// Manifests expire so a captured old manifest cannot be replayed forever.
    pub expires_at: i64,
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    /// `juggernaught`, `photocraft`, `google-fonts`, `kokoro-model`, …
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Upstream provenance for third-party tools.
    #[serde(default)]
    pub upstream_commit: Option<String>,
    /// `security` updates apply immediately under the default policy.
    #[serde(default)]
    pub security: bool,
}

/// Verify a manifest: signature, expiry. Returns the parsed manifest.
pub fn verify_manifest(bytes: &[u8], sig: &[u8], trusted_keys: &[Vec<u8>], now: i64) -> Result<Manifest, UpdateError> {
    if !trusted_keys.iter().any(|k| verify_signature(k, bytes, sig)) {
        return Err(UpdateError::BadSignature);
    }
    let m: Manifest = serde_json::from_slice(bytes).map_err(|e| UpdateError::Parse(e.to_string()))?;
    if m.expires_at < now {
        return Err(UpdateError::Parse("manifest expired".into()));
    }
    Ok(m)
}

pub fn verify_artifact(c: &Component, bytes: &[u8]) -> Result<(), UpdateError> {
    if bytes.len() as u64 != c.size || sha256_hex(bytes) != c.sha256 {
        return Err(UpdateError::BadChecksum(c.name.clone()));
    }
    Ok(())
}

/// Compare dotted versions (leading `v` ignored; non-numeric parts compare as strings).
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    fn parts(v: &str) -> Vec<String> {
        v.trim_start_matches('v').split(['.', '-']).map(str::to_string).collect()
    }
    let (a, b) = (parts(candidate), parts(installed));
    for (x, y) in a.iter().zip(b.iter()) {
        match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) if x != y => return x > y,
            (Ok(_), Ok(_)) => continue,
            _ if x != y => return x > y,
            _ => continue,
        }
    }
    a.len() > b.len()
}

/// Release directory layout: `<root>/<component>/<version>/` + `<root>/<component>/current` symlink.
pub struct Releases {
    pub root: PathBuf,
}

impl Releases {
    pub fn stage_dir(&self, component: &str, version: &str) -> PathBuf {
        self.root.join(component).join(version)
    }

    pub fn current(&self, component: &str) -> Option<String> {
        std::fs::read_link(self.root.join(component).join("current")).ok().and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
    }

    /// Atomically point `current` at `version`; returns the previous version.
    pub fn promote(&self, component: &str, version: &str) -> Result<Option<String>, UpdateError> {
        let base = self.root.join(component);
        let prev = self.current(component);
        let tmp = base.join(".current.tmp");
        let _ = std::fs::remove_file(&tmp);
        symlink(Path::new(version), &tmp)?;
        std::fs::rename(&tmp, base.join("current"))?;
        Ok(prev)
    }

    pub fn rollback(&self, component: &str, to: &str) -> Result<(), UpdateError> {
        self.promote(component, to).map(|_| ())
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}
#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("symlink swap requires unix"))
}

/// Full apply: verify → stage → canary → promote. `canary` gets the staged dir.
pub fn apply(rel: &Releases, c: &Component, artifact: &[u8], allow_downgrade: bool, canary: impl Fn(&Path) -> Result<(), String>) -> Result<Option<String>, UpdateError> {
    verify_artifact(c, artifact)?;
    if let Some(cur) = rel.current(&c.name) {
        if !allow_downgrade && !is_newer(&c.version, &cur) {
            return Err(UpdateError::Downgrade { component: c.name.clone(), from: cur, to: c.version.clone() });
        }
    }
    let dir = rel.stage_dir(&c.name, &c.version);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("artifact"), artifact)?;
    canary(&dir).map_err(UpdateError::CanaryFailed)?;
    rel.promote(&c.name, &c.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jugg_crypto::Signer;

    #[test]
    fn signed_manifest_stage_canary_promote_rollback() {
        let signer = Signer::generate().unwrap();
        let art = b"new binary".to_vec();
        let m = Manifest {
            channel: "stable".into(),
            issued_at: 0,
            expires_at: i64::MAX,
            components: vec![Component { name: "photocraft".into(), version: "v0.24.0".into(), url: "x".into(), sha256: sha256_hex(&art), size: art.len() as u64, upstream_commit: None, security: false }],
        };
        let bytes = serde_json::to_vec(&m).unwrap();
        let sig = signer.sign(&bytes).unwrap();
        let keys = vec![signer.public_key()];
        let m = verify_manifest(&bytes, &sig, &keys, 1).unwrap();
        let mut tampered = bytes.clone();
        tampered[10] ^= 1;
        assert!(verify_manifest(&tampered, &sig, &keys, 1).is_err());

        let dir = tempfile::tempdir().unwrap();
        let rel = Releases { root: dir.path().into() };
        std::fs::create_dir_all(rel.stage_dir("photocraft", "v0.23.1")).unwrap();
        rel.promote("photocraft", "v0.23.1").unwrap();

        // failing canary leaves current untouched
        assert!(apply(&rel, &m.components[0], &art, false, |_| Err("render mismatch".into())).is_err());
        assert_eq!(rel.current("photocraft").as_deref(), Some("v0.23.1"));
        // passing canary promotes; previous version returned for rollback
        let prev = apply(&rel, &m.components[0], &art, false, |_| Ok(())).unwrap();
        assert_eq!(rel.current("photocraft").as_deref(), Some("v0.24.0"));
        rel.rollback("photocraft", &prev.unwrap()).unwrap();
        assert_eq!(rel.current("photocraft").as_deref(), Some("v0.23.1"));
        // corrupted artifact rejected
        assert!(apply(&rel, &m.components[0], b"evil", false, |_| Ok(())).is_err());
    }

    #[test]
    fn version_ordering() {
        assert!(is_newer("v0.24.1", "v0.23.9"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(!is_newer("v0.23.1", "v0.23.1"));
        assert!(!is_newer("0.9", "0.10"));
    }
}
