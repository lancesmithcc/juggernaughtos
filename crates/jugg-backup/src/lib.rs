//! Whole-box backups.
//!
//! ```text
//! file  = "JBK1" | u32 header_len | header JSON | body | trailer
//! body  = STREAM AES-256-GCM( archive )            (64 KiB chunks)
//! archive entries = u16 path_len | path | u64 len | bytes      (repeated)
//!                   then the index entry "__index.json" listing sha256 per file
//! trailer = sha256(body) [32] | ML-DSA-65 signature | u32 sig_len
//! ```
//!
//! The body's data key (DEK) is wrapped **twice** in the header:
//! * to the box (KEK-derived file-wrap key) — routine restore tests and rollbacks;
//! * to the owner's offline **recovery key** (ML-KEM-768 + X25519) — disaster recovery
//!   on a brand-new machine. The box KEKs are escrowed the same way, so field-encrypted
//!   data (emails, messages, secrets) is readable after a restore.
//!
//! The signature covers `header ‖ sha256(body)`, so a backup cannot be swapped,
//! truncated or edited without detection.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use jugg_crypto::{HybridPublicKey, HybridSecretKey, HybridWrapped, Keyring, Signer, StreamOpener, StreamSealer};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"JBK1";
const INDEX: &str = "__index.json";

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("crypto: {0}")]
    Crypto(#[from] jugg_crypto::CryptoError),
    #[error("database: {0}")]
    Db(String),
    #[error("backup is invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, BackupError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    pub id: String,
    pub created_at: i64,
    pub box_signing_key: String,
    pub stream_prefix: String,
    pub dek_box: (u8, String),
    pub dek_recovery: Option<HybridWrapped>,
    pub keks_recovery: Option<HybridWrapped>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub file: PathBuf,
    pub id: String,
    pub created_at: i64,
    pub entries: usize,
    pub bytes: u64,
    pub recovery_escrow: bool,
}

/// What goes into a backup: (path inside archive, file on disk).
pub fn collect(roots: &[(&str, PathBuf)]) -> Vec<(String, PathBuf)> {
    fn walk(prefix: &str, p: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(meta) = std::fs::symlink_metadata(p) else { return };
        if meta.is_dir() {
            let mut kids: Vec<_> = std::fs::read_dir(p).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            kids.sort();
            for k in kids {
                let name = k.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                walk(&format!("{prefix}/{name}"), &k, out);
            }
        } else if meta.is_file() {
            out.push((prefix.trim_start_matches('/').to_string(), p.to_path_buf()));
        }
    }
    let mut out = Vec::new();
    for (prefix, root) in roots {
        walk(prefix, root, &mut out);
    }
    out
}

fn sha_hex(ctx: aws_lc_rs::digest::Context) -> String {
    hex::encode(ctx.finish().as_ref())
}

/// Consistent SQLite snapshot (online, no downtime).
pub async fn snapshot_db(pool: &jugg_db::sqlx::SqlitePool, to: &Path) -> Result<()> {
    let _ = std::fs::remove_file(to);
    jugg_db::sqlx::query("VACUUM INTO ?").bind(to.display().to_string()).execute(pool).await.map_err(|e| BackupError::Db(e.to_string()))?;
    Ok(())
}

pub fn create(out_dir: &Path, files: &[(String, PathBuf)], ring: &Keyring, signer: &Signer, recovery: Option<&HybridPublicKey>) -> Result<Report> {
    std::fs::create_dir_all(out_dir)?;
    let created_at = jugg_core::now_unix();
    let id = format!("{}-{}", created_at, hex::encode(jugg_crypto::random_bytes::<4>()));
    let dek = zeroize::Zeroizing::new(jugg_crypto::random_bytes::<32>());
    let prefix = jugg_crypto::random_bytes::<8>();
    let ctx = format!("jugg-backup:{id}");
    let (kid, wrapped) = ring.wrap_dek(&dek, ctx.as_bytes())?;
    let (dek_recovery, keks_recovery) = match recovery {
        Some(pk) => {
            let keks: Vec<(u8, String)> = ring.export_for_escrow().iter().map(|(i, k)| (*i, B64.encode(k.as_ref()))).collect();
            let keks_json = zeroize::Zeroizing::new(serde_json::to_vec(&keks).map_err(|e| BackupError::Invalid(e.to_string()))?);
            (Some(pk.wrap_key(dek.as_ref(), ctx.as_bytes())?), Some(pk.wrap_key(&keks_json, format!("{ctx}:keks").as_bytes())?))
        }
        None => (None, None),
    };
    let header = Header {
        version: 1,
        id: id.clone(),
        created_at,
        box_signing_key: B64.encode(signer.public_key()),
        stream_prefix: B64.encode(prefix),
        dek_box: (kid, B64.encode(wrapped)),
        dek_recovery,
        keks_recovery,
    };
    let header_bytes = serde_json::to_vec(&header).map_err(|e| BackupError::Invalid(e.to_string()))?;

    let final_path = out_dir.join(format!("juggernaught-{id}.jbk"));
    let tmp_path = out_dir.join(format!(".juggernaught-{id}.jbk.tmp"));
    let mut f = BufWriter::new(open_private(&tmp_path)?);
    f.write_all(MAGIC)?;
    f.write_all(&(header_bytes.len() as u32).to_be_bytes())?;
    f.write_all(&header_bytes)?;

    // Body: hash the ciphertext as it is written.
    struct HashingWriter<W: Write> {
        inner: W,
        hash: aws_lc_rs::digest::Context,
        bytes: u64,
    }
    impl<W: Write> Write for HashingWriter<W> {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let n = self.inner.write(b)?;
            self.hash.update(&b[..n]);
            self.bytes += n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }
    let hw = HashingWriter { inner: f, hash: aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256), bytes: 0 };
    let mut sealer = StreamSealer::new(hw, &dek, prefix, ctx.as_bytes())?;

    let mut index = Vec::with_capacity(files.len());
    let mut buf = vec![0u8; 256 * 1024];
    for (name, path) in files {
        let mut src = File::open(path)?;
        let size = src.metadata()?.len();
        write_entry_head(&mut sealer, name, size)?;
        let mut h = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
        let mut copied = 0u64;
        while copied < size {
            let want = ((size - copied) as usize).min(buf.len());
            let n = src.read(&mut buf[..want])?;
            if n == 0 {
                return Err(BackupError::Invalid(format!("{name} shrank while backing up")));
            }
            h.update(&buf[..n]);
            sealer.write_all(&buf[..n])?;
            copied += n as u64;
        }
        index.push(Entry { path: name.clone(), size, sha256: sha_hex(h) });
    }
    let index_bytes = serde_json::to_vec(&index).map_err(|e| BackupError::Invalid(e.to_string()))?;
    write_entry_head(&mut sealer, INDEX, index_bytes.len() as u64)?;
    sealer.write_all(&index_bytes)?;
    let hw = sealer.finish()?;
    let body_hash = hw.hash.finish();
    let mut f = hw.inner;

    let mut signed = header_bytes.clone();
    signed.extend_from_slice(body_hash.as_ref());
    let sig = signer.sign(&signed)?;
    f.write_all(body_hash.as_ref())?;
    f.write_all(&sig)?;
    f.write_all(&(sig.len() as u32).to_be_bytes())?;
    f.flush()?;
    f.into_inner().map_err(|e| BackupError::Io(e.into_error()))?.sync_all()?;
    std::fs::rename(&tmp_path, &final_path)?;

    Ok(Report { file: final_path, id, created_at, entries: index.len(), bytes: index.iter().map(|e| e.size).sum(), recovery_escrow: header.dek_recovery.is_some() })
}

fn write_entry_head<W: Write>(w: &mut W, name: &str, size: u64) -> std::io::Result<()> {
    let n = name.as_bytes();
    w.write_all(&(n.len() as u16).to_be_bytes())?;
    w.write_all(n)?;
    w.write_all(&size.to_be_bytes())
}

fn open_private(p: &Path) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

/// Read the header and check the signature over `header ‖ sha256(body)`.
/// `trusted_signers` empty = trust the key embedded in the header (only
/// sensible for the box's own backups during a restore test).
pub fn verify(file: &Path, trusted_signers: &[Vec<u8>]) -> Result<(Header, u64, u64)> {
    let mut f = File::open(file)?;
    let total = f.metadata()?.len();
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(BackupError::Invalid("not a Juggernaught backup".into()));
    }
    let mut len = [0u8; 4];
    f.read_exact(&mut len)?;
    let hlen = u32::from_be_bytes(len) as u64;
    let mut header_bytes = vec![0u8; hlen as usize];
    f.read_exact(&mut header_bytes)?;
    let header: Header = serde_json::from_slice(&header_bytes).map_err(|e| BackupError::Invalid(e.to_string()))?;

    f.seek(SeekFrom::End(-4))?;
    f.read_exact(&mut len)?;
    let sig_len = u32::from_be_bytes(len) as u64;
    let body_start = 8 + hlen;
    let trailer = 32 + sig_len + 4;
    if total < body_start + trailer {
        return Err(BackupError::Invalid("truncated".into()));
    }
    let body_len = total - body_start - trailer;
    f.seek(SeekFrom::Start(body_start + body_len))?;
    let mut body_hash = [0u8; 32];
    f.read_exact(&mut body_hash)?;
    let mut sig = vec![0u8; sig_len as usize];
    f.read_exact(&mut sig)?;

    f.seek(SeekFrom::Start(body_start))?;
    let mut h = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
    let mut remaining = body_len;
    let mut buf = vec![0u8; 256 * 1024];
    while remaining > 0 {
        let want = (remaining as usize).min(buf.len());
        let n = f.read(&mut buf[..want])?;
        if n == 0 {
            return Err(BackupError::Invalid("truncated body".into()));
        }
        h.update(&buf[..n]);
        remaining -= n as u64;
    }
    if h.finish().as_ref() != body_hash {
        return Err(BackupError::Invalid("body hash mismatch".into()));
    }
    let embedded = B64.decode(&header.box_signing_key).map_err(|e| BackupError::Invalid(e.to_string()))?;
    let keys: Vec<Vec<u8>> = if trusted_signers.is_empty() { vec![embedded] } else { trusted_signers.to_vec() };
    let mut signed = header_bytes;
    signed.extend_from_slice(&body_hash);
    if !keys.iter().any(|k| jugg_crypto::verify_signature(k, &signed, &sig)) {
        return Err(BackupError::Invalid("signature does not verify".into()));
    }
    Ok((header, body_start, body_len))
}

/// How to obtain the body key.
pub enum Unlock<'a> {
    Box(&'a Keyring),
    Recovery(&'a HybridSecretKey),
}

/// Decrypt and walk every entry. With `extract_to`, files are written there
/// (paths validated); without, it is a pure integrity check. Returns the index.
pub fn open(file: &Path, unlock: Unlock<'_>, trusted_signers: &[Vec<u8>], extract_to: Option<&Path>) -> Result<(Header, Vec<Entry>)> {
    let (header, body_start, body_len) = verify(file, trusted_signers)?;
    let ctx = format!("jugg-backup:{}", header.id);
    let dek = zeroize::Zeroizing::new(match unlock {
        Unlock::Box(ring) => {
            let wrapped = B64.decode(&header.dek_box.1).map_err(|e| BackupError::Invalid(e.to_string()))?;
            ring.unwrap_dek(header.dek_box.0, &wrapped, ctx.as_bytes())?
        }
        Unlock::Recovery(sk) => {
            let w = header.dek_recovery.as_ref().ok_or_else(|| BackupError::Invalid("backup has no recovery escrow".into()))?;
            sk.unwrap_key(w, ctx.as_bytes())?.try_into().map_err(|_| BackupError::Invalid("bad key length".into()))?
        }
    });
    let prefix: [u8; 8] = B64.decode(&header.stream_prefix).ok().and_then(|v| v.try_into().ok()).ok_or_else(|| BackupError::Invalid("bad prefix".into()))?;
    let mut f = File::open(file)?;
    f.seek(SeekFrom::Start(body_start))?;
    let mut r = StreamOpener::new(BufReader::new(f.take(body_len)), &dek, prefix, ctx.as_bytes())?;

    let mut seen: Vec<Entry> = Vec::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let mut l2 = [0u8; 2];
        r.read_exact(&mut l2)?;
        let mut name = vec![0u8; u16::from_be_bytes(l2) as usize];
        r.read_exact(&mut name)?;
        let name = String::from_utf8(name).map_err(|_| BackupError::Invalid("bad entry name".into()))?;
        let mut l8 = [0u8; 8];
        r.read_exact(&mut l8)?;
        let size = u64::from_be_bytes(l8);
        if name == INDEX {
            let mut idx = vec![0u8; size as usize];
            r.read_exact(&mut idx)?;
            let index: Vec<Entry> = serde_json::from_slice(&idx).map_err(|e| BackupError::Invalid(e.to_string()))?;
            if index != seen {
                return Err(BackupError::Invalid("index does not match contents".into()));
            }
            if r.read(&mut [0u8; 1])? != 0 {
                return Err(BackupError::Invalid("data after index".into()));
            }
            return Ok((header, index));
        }
        let mut out = match extract_to {
            Some(root) => Some(BufWriter::new(open_private(&safe_join(root, &name)?)?)),
            None => None,
        };
        let mut h = aws_lc_rs::digest::Context::new(&aws_lc_rs::digest::SHA256);
        let mut left = size;
        while left > 0 {
            let want = (left as usize).min(buf.len());
            let n = r.read(&mut buf[..want])?;
            if n == 0 {
                return Err(BackupError::Invalid("truncated entry".into()));
            }
            h.update(&buf[..n]);
            if let Some(o) = out.as_mut() {
                o.write_all(&buf[..n])?;
            }
            left -= n as u64;
        }
        if let Some(mut o) = out {
            o.flush()?;
        }
        seen.push(Entry { path: name, size, sha256: sha_hex(h) });
    }
}

fn safe_join(root: &Path, name: &str) -> Result<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(BackupError::Invalid(format!("unsafe path in backup: {name}")));
    }
    let full = root.join(p);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(full)
}

/// Recover the escrowed KEKs with the owner's recovery key.
pub fn escrowed_keks(header: &Header, sk: &HybridSecretKey) -> Result<Vec<(u8, [u8; 32])>> {
    let w = header.keks_recovery.as_ref().ok_or_else(|| BackupError::Invalid("backup has no key escrow".into()))?;
    let raw = zeroize::Zeroizing::new(sk.unwrap_key(w, format!("jugg-backup:{}:keks", header.id).as_bytes())?);
    let list: Vec<(u8, String)> = serde_json::from_slice(&raw).map_err(|e| BackupError::Invalid(e.to_string()))?;
    list.into_iter()
        .map(|(id, k)| {
            let bytes = B64.decode(k).map_err(|e| BackupError::Invalid(e.to_string()))?;
            Ok((id, bytes.try_into().map_err(|_| BackupError::Invalid("bad KEK".into()))?))
        })
        .collect()
}

/// Keep the newest `keep` backups in `dir`.
pub fn prune(dir: &Path, keep: usize) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jbk")).collect();
    files.sort();
    let remove = files.len().saturating_sub(keep);
    let gone: Vec<PathBuf> = files.into_iter().take(remove).collect();
    for p in &gone {
        std::fs::remove_file(p)?;
    }
    Ok(gone)
}

pub fn latest(dir: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jbk")).collect();
    files.sort();
    files.pop()
}

/// Owner recovery key file (kept offline by the owner; never stored on the box).
#[derive(Serialize, Deserialize)]
pub struct RecoveryKeyFile {
    pub kind: String,
    pub created_at: i64,
    pub mlkem768_secret: String,
    pub x25519_seed: String,
    pub public: HybridPublicKey,
}

pub fn new_recovery_key() -> Result<(RecoveryKeyFile, HybridPublicKey)> {
    let (sk, pk) = HybridSecretKey::generate()?;
    let (m, x) = sk.to_bytes()?;
    Ok((RecoveryKeyFile { kind: "juggernaught-recovery-key-v1".into(), created_at: jugg_core::now_unix(), mlkem768_secret: B64.encode(m), x25519_seed: B64.encode(x), public: pk.clone() }, pk))
}

impl RecoveryKeyFile {
    pub fn secret(&self) -> Result<HybridSecretKey> {
        let m = B64.decode(&self.mlkem768_secret).map_err(|e| BackupError::Invalid(e.to_string()))?;
        let x = B64.decode(&self.x25519_seed).map_err(|e| BackupError::Invalid(e.to_string()))?;
        Ok(HybridSecretKey::from_bytes(&m, &x)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_verify_restore_and_tamper() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("assets/logos")).unwrap();
        std::fs::write(src.join("db.sqlite"), b"pretend database").unwrap();
        let big: Vec<u8> = (0..300_000).map(|i| (i % 253) as u8).collect();
        std::fs::write(src.join("assets/logos/logo.png"), &big).unwrap();
        std::fs::write(src.join("assets/empty.txt"), b"").unwrap();

        let ring = Keyring::load_or_create(dir.path().join("keys")).unwrap();
        let signer = Signer::generate().unwrap();
        let (rk, pk) = new_recovery_key().unwrap();
        let files = collect(&[("db", src.join("db.sqlite")), ("assets", src.join("assets"))]);
        assert_eq!(files.len(), 3);
        let out = dir.path().join("backups");
        let rep = create(&out, &files, &ring, &signer, Some(&pk)).unwrap();
        assert!(rep.recovery_escrow);

        // routine restore test with the box key
        let (_, idx) = open(&rep.file, Unlock::Box(&ring), &[signer.public_key()], None).unwrap();
        assert_eq!(idx.len(), 3);

        // disaster restore on a "new machine": only the recovery key file
        let rk: RecoveryKeyFile = serde_json::from_str(&serde_json::to_string(&rk).unwrap()).unwrap();
        let sk = rk.secret().unwrap();
        let restored = dir.path().join("restored");
        let (header, _) = open(&rep.file, Unlock::Recovery(&sk), &[], Some(&restored)).unwrap();
        assert_eq!(std::fs::read(restored.join("assets/logos/logo.png")).unwrap(), big);
        let keks = escrowed_keks(&header, &sk).unwrap();
        let new_ring = Keyring::restore_from_escrow(dir.path().join("newkeys"), &keks).unwrap();
        let ct = jugg_crypto::seal(&ring, b"owner@example.test", b"users:email:1").unwrap();
        assert_eq!(jugg_crypto::open(&new_ring, &ct, b"users:email:1").unwrap(), b"owner@example.test", "escrowed KEK decrypts old data");

        // any flipped byte in the body breaks verification
        let mut bytes = std::fs::read(&rep.file).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 1;
        let bad = dir.path().join("bad.jbk");
        std::fs::write(&bad, &bytes).unwrap();
        assert!(verify(&bad, &[signer.public_key()]).is_err());
        // wrong signer rejected
        assert!(verify(&rep.file, &[Signer::generate().unwrap().public_key()]).is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_join(Path::new("/tmp/x"), "../etc/passwd").is_err());
        assert!(safe_join(Path::new("/tmp/x"), "/etc/passwd").is_err());
    }
}
