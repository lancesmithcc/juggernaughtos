//! `juggernaught keys seal|status|unseal`: move the KEKs and the box signing key
//! out of plaintext files into systemd encrypted credentials. systemd-creds binds
//! them to the TPM2 chip when the machine has one (plus the host key), otherwise
//! to the host key in /var/lib/systemd/credential.secret. systemd decrypts them
//! into a private ramfs (`$CREDENTIALS_DIRECTORY`) only for our own units.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Units that need the keys. Each gets the same drop-in.
pub const UNITS: [&str; 3] = ["juggernaught.service", "juggernaught-backup.service", "juggernaught-restore-test.service"];
pub const SIGNING_FILE: &str = "box-signing.mldsa65.pk8";
const DROPIN: &str = "50-sealed-keys.conf";

pub struct Paths {
    pub keys_dir: PathBuf,
    /// Where the `.cred` files live (root-only, e.g. /etc/juggernaught/creds).
    pub cred_dir: PathBuf,
    /// /etc/systemd/system
    pub unit_dir: PathBuf,
}

/// (credential name, plaintext file) for every key this box holds.
pub fn key_files(keys_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(keys_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let id = name.strip_prefix("kek-")?.strip_suffix(".key")?.parse::<u8>().ok()?;
            Some((format!("kek-{id}"), e.path()))
        })
        .collect();
    out.sort();
    let signing = keys_dir.join(SIGNING_FILE);
    if signing.exists() {
        out.push(("signing".into(), signing));
    }
    out
}

/// The `[Service]` drop-in that hands the sealed keys to a unit.
pub fn dropin(names: &[String], cred_dir: &Path) -> String {
    let mut s = String::from("# Written by `juggernaught keys seal`. Remove with `juggernaught keys unseal`.\n[Service]\n");
    for n in names {
        s.push_str(&format!("LoadCredentialEncrypted={n}:{}\n", cred_dir.join(format!("{n}.cred")).display()));
    }
    s
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

pub fn has_tpm2() -> bool {
    Command::new("systemd-creds").arg("has-tpm2").output().map(|o| o.status.success()).unwrap_or(false)
}

fn systemd_running() -> bool {
    Path::new("/run/systemd/system").exists()
}

fn overwrite_and_remove(path: &Path) -> std::io::Result<()> {
    let len = std::fs::metadata(path)?.len() as usize;
    std::fs::write(path, vec![0u8; len])?;
    std::fs::remove_file(path)
}

/// Seal every key. Refuses unless the owner's offline recovery key exists, because a
/// TPM-bound key dies with the motherboard and backups would then be unrecoverable.
pub fn seal(p: &Paths, recovery_public_key: &Path) -> Result<String, String> {
    if jugg_crypto::is_sealed(&p.keys_dir) {
        return Err("keys are already sealed (see `juggernaught keys status`)".into());
    }
    if !recovery_public_key.exists() {
        return Err(format!(
            "no owner recovery key yet ({} missing). Run `juggernaught backup init-recovery` and move the key off this machine first: sealed keys cannot leave this hardware, so the recovery key is the only way to restore elsewhere.",
            recovery_public_key.display()
        ));
    }
    let keys = key_files(&p.keys_dir);
    if !keys.iter().any(|(n, _)| n.starts_with("kek-")) {
        return Err(format!("no keys in {} yet; start the service once so it creates them", p.keys_dir.display()));
    }
    std::fs::create_dir_all(&p.cred_dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p.cred_dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let tpm = has_tpm2();
    let with_key = if tpm { "tpm2+host" } else { "host" };
    for (name, file) in &keys {
        let cred = p.cred_dir.join(format!("{name}.cred"));
        run(Command::new("systemd-creds").args(["encrypt", &format!("--name={name}"), &format!("--with-key={with_key}")]).arg(file).arg(&cred))?;
        // Prove the round trip before anything is destroyed.
        let back = run(Command::new("systemd-creds").args(["decrypt", &format!("--name={name}")]).arg(&cred).arg("-"))?;
        let orig = std::fs::read(file).map_err(|e| e.to_string())?;
        if back != orig {
            return Err(format!("{name}: decrypted credential does not match the key; nothing was changed"));
        }
    }
    let names: Vec<String> = keys.iter().map(|(n, _)| n.clone()).collect();
    for unit in UNITS {
        let d = p.unit_dir.join(format!("{unit}.d"));
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        std::fs::write(d.join(DROPIN), dropin(&names, &p.cred_dir)).map_err(|e| e.to_string())?;
    }
    let live = systemd_running();
    if live {
        run(Command::new("systemctl").arg("daemon-reload"))?;
        run(Command::new("systemctl").args(["restart", "juggernaught.service"]))?;
        std::thread::sleep(std::time::Duration::from_secs(3));
        if run(Command::new("systemctl").args(["is-active", "--quiet", "juggernaught.service"])).is_err() {
            for unit in UNITS {
                let _ = std::fs::remove_file(p.unit_dir.join(format!("{unit}.d")).join(DROPIN));
            }
            let _ = run(Command::new("systemctl").arg("daemon-reload"));
            let _ = run(Command::new("systemctl").args(["restart", "juggernaught.service"]));
            return Err("juggernaught.service did not start with the sealed keys; drop-ins removed, plaintext keys kept".into());
        }
    }
    for (_, file) in &keys {
        overwrite_and_remove(file).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    std::fs::write(p.keys_dir.join(jugg_crypto::SEALED_MARKER), format!("sealed with {with_key}\n")).map_err(|e| e.to_string())?;
    Ok(format!(
        "Sealed {} key(s) with {with_key}{}.{}",
        names.len(),
        if tpm { " (TPM2 chip found)" } else { " (no TPM2 chip; host key only)" },
        if live { " Service restarted on the sealed keys; plaintext key files destroyed." } else { " systemd is not running here: units pick the keys up on their next start." }
    ))
}

/// Turn sealed credentials back into key files (to migrate hardware or rotate).
pub fn unseal(p: &Paths, owner: &str) -> Result<String, String> {
    if !jugg_crypto::is_sealed(&p.keys_dir) {
        return Err("keys are not sealed".into());
    }
    let mut n = 0;
    for entry in std::fs::read_dir(&p.cred_dir).map_err(|e| e.to_string())?.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        let Some(name) = fname.strip_suffix(".cred") else { continue };
        let target = if name == "signing" { p.keys_dir.join(SIGNING_FILE) } else { p.keys_dir.join(format!("{name}.key")) };
        let bytes = run(Command::new("systemd-creds").args(["decrypt", &format!("--name={name}")]).arg(entry.path()).arg("-"))?;
        {
            use std::io::Write;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            o.open(&target).and_then(|mut f| f.write_all(&bytes)).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        let _ = Command::new("chown").arg(format!("{owner}:{owner}")).arg(&target).status();
        n += 1;
    }
    for unit in UNITS {
        let _ = std::fs::remove_file(p.unit_dir.join(format!("{unit}.d")).join(DROPIN));
    }
    std::fs::remove_file(p.keys_dir.join(jugg_crypto::SEALED_MARKER)).map_err(|e| e.to_string())?;
    if systemd_running() {
        run(Command::new("systemctl").arg("daemon-reload"))?;
        run(Command::new("systemctl").args(["restart", "juggernaught.service"]))?;
    }
    Ok(format!("Unsealed {n} key(s) back to {}. Seal again with `juggernaught keys seal`.", p.keys_dir.display()))
}

/// On a sealed box an admin command run from a shell has no keys. Re-run it as a
/// transient systemd unit that gets the credentials (needs root), piping stdio back.
#[cfg(unix)]
pub fn reexec_with_credentials(config: &Path, cred_dir: &Path, user: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("systemd-run");
    cmd.args(["--quiet", "--pipe", "--wait", "--collect", "--same-dir", &format!("--uid={user}"), &format!("--gid={user}")]);
    cmd.arg(format!("--setenv=JUGG_CONFIG={}", config.display()));
    for entry in std::fs::read_dir(cred_dir).into_iter().flatten().flatten() {
        let f = entry.file_name().to_string_lossy().to_string();
        if let Some(name) = f.strip_suffix(".cred") {
            cmd.arg(format!("--property=LoadCredentialEncrypted={name}:{}", entry.path().display()));
        }
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| "/usr/local/bin/juggernaught".into());
    cmd.arg("--").arg(exe).args(std::env::args_os().skip(1));
    cmd.exec()
}

pub fn should_reexec(keys_dir: &Path) -> bool {
    jugg_crypto::is_sealed(keys_dir) && std::env::var_os("CREDENTIALS_DIRECTORY").is_none() && systemd_running()
}

pub fn status(p: &Paths) -> String {
    let sealed = jugg_crypto::is_sealed(&p.keys_dir);
    let mut s = String::new();
    s.push_str(&format!("keys         {}\n", if sealed { "sealed (systemd-creds)" } else { "plaintext files (0600)" }));
    if sealed {
        if let Ok(m) = std::fs::read_to_string(p.keys_dir.join(jugg_crypto::SEALED_MARKER)) {
            s.push_str(&format!("binding      {}\n", m.trim().trim_start_matches("sealed with ")));
        }
        let creds: Vec<String> = std::fs::read_dir(&p.cred_dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        s.push_str(&format!("credentials  {}\n", if creds.is_empty() { "NONE (unreadable or missing)".into() } else { creds.join(", ") }));
        for unit in UNITS {
            let ok = p.unit_dir.join(format!("{unit}.d")).join(DROPIN).exists();
            s.push_str(&format!("drop-in      {unit} {}\n", if ok { "ok" } else { "MISSING" }));
        }
    } else {
        s.push_str(&format!("files        {}\n", key_files(&p.keys_dir).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")));
    }
    s.push_str(&format!("tpm2         {}\n", if has_tpm2() { "available" } else { "not available (host key only)" }));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_keys_and_writes_dropins() {
        let d = tempfile::tempdir().unwrap();
        for f in ["kek-2.key", "kek-1.key", SIGNING_FILE, "setup-token", "kek-x.key"] {
            std::fs::write(d.path().join(f), b"k").unwrap();
        }
        let names: Vec<String> = key_files(d.path()).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["kek-1", "kek-2", "signing"]);
        let dropin = dropin(&names, Path::new("/etc/juggernaught/creds"));
        assert!(dropin.contains("[Service]\nLoadCredentialEncrypted=kek-1:/etc/juggernaught/creds/kek-1.cred\n"));
        assert!(dropin.contains("LoadCredentialEncrypted=signing:/etc/juggernaught/creds/signing.cred\n"));
    }

    #[test]
    fn seal_requires_recovery_key() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("kek-1.key"), [0u8; 32]).unwrap();
        let p = Paths { keys_dir: d.path().into(), cred_dir: d.path().join("creds"), unit_dir: d.path().join("units") };
        let e = seal(&p, &d.path().join("missing.pub.json")).unwrap_err();
        assert!(e.contains("recovery key"), "{e}");
        assert!(d.path().join("kek-1.key").exists());
    }
}
