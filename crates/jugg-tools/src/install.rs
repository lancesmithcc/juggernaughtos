//! `juggernaught tools install` — fetch pinned tools, verify, smoke-test, activate.
//!
//! Layout: `<tools_dir>/<name>/<version>/…` (what the registry runs). Install is
//! staged in `<name>/.staging-<version>` and only renamed into place after the
//! SHA-256 matches the pin in tools.lock.toml **and** the binary starts.

use crate::lock::{LockedTool, ToolsLock};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ToolState {
    pub name: String,
    pub version: String,
    pub installed: bool,
    pub note: String,
}

pub fn arch() -> (&'static str, &'static str) {
    match std::env::consts::ARCH {
        "aarch64" => ("aarch64", "arm64"),
        _ => ("x86_64", "x64"),
    }
}

pub fn root(tools_dir: &Path, name: &str, t: &LockedTool) -> PathBuf {
    tools_dir.join(name).join(&t.version)
}

pub fn status(lock: &ToolsLock, tools_dir: &Path) -> Vec<ToolState> {
    lock.tool
        .iter()
        .map(|(name, t)| {
            let installed = t.is_builtin() || (!t.program().is_empty() && root(tools_dir, name, t).join(t.program()).exists());
            let note = if t.is_builtin() {
                t.note.clone().unwrap_or_else(|| "ready".into())
            } else if installed {
                "ready".into()
            } else if t.install == "unavailable" {
                t.note.clone().unwrap_or_else(|| "not available on this platform yet".into())
            } else {
                "not installed".into()
            };
            ToolState { name: name.clone(), version: t.version.clone(), installed, note }
        })
        .collect()
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let out = if cfg!(target_os = "macos") { run(Command::new("shasum").args(["-a", "256"]).arg(p))? } else { run(Command::new("sha256sum").arg(p))? };
    out.split_whitespace().next().map(str::to_string).ok_or_else(|| "could not hash download".into())
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).chars().take(600).collect())
    }
}

/// Install one tool. Returns a human summary.
pub fn install(tools_dir: &Path, name: &str, t: &LockedTool) -> Result<String, String> {
    let dest = root(tools_dir, name, t);
    if t.is_builtin() {
        return Ok(format!("{name} needs no install"));
    }
    if !t.program().is_empty() && dest.join(t.program()).exists() {
        return Ok(format!("{name} {} already installed", t.version));
    }
    let (a, a_short) = arch();
    let staging = tools_dir.join(name).join(format!(".staging-{}", t.version));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        match t.install.as_str() {
            "github-tarball" | "github-binary" => {
                let d = t.dist.as_ref().ok_or("lock entry has no dist")?;
                let v = t.version.trim_start_matches('v');
                let mac = cfg!(target_os = "macos");
                let template = if mac { d.asset_macos.as_ref().ok_or("there's no macOS download for this tool")? } else { d.asset.as_ref().ok_or("lock entry has no asset")? };
                let asset = template.replace("{v}", v).replace("{arch_short}", a_short).replace("{arch}", a);
                let pin = match (mac, a) {
                    (true, "aarch64") => &d.sha256_macos_aarch64,
                    (true, _) => &d.sha256_macos_x86_64,
                    (false, "aarch64") => &d.sha256_aarch64,
                    (false, _) => &d.sha256_x86_64,
                };
                let pin = pin.as_ref().ok_or("no pinned hash for this CPU")?;
                let url = format!("https://github.com/{}/releases/download/{}/{}", t.repo, t.version, asset);
                let file = staging.join(".download");
                run(Command::new("curl").args(["-fsSL", "--proto", "=https", "--tlsv1.2", "--retry", "3", "-o"]).arg(&file).arg(&url))?;
                let got = sha256_file(&file)?;
                if &got != pin {
                    return Err(format!("checksum mismatch for {asset}: expected {pin}, got {got}. Refusing to install."));
                }
                if asset.ends_with(".zip") {
                    unzip_flat(&file, &staging)?;
                } else if t.install == "github-tarball" {
                    run(Command::new("tar").args(["-xzf"]).arg(&file).args(["--strip-components=1", "-C"]).arg(&staging))?;
                } else {
                    let bin = staging.join(t.program());
                    std::fs::create_dir_all(bin.parent().unwrap_or(&staging)).map_err(|e| e.to_string())?;
                    std::fs::rename(&file, &bin).map_err(|e| e.to_string())?;
                }
                let _ = std::fs::remove_file(&file);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(staging.join(t.program()), std::fs::Permissions::from_mode(0o755));
                }
                Ok(())
            }
            "npm" => {
                let dir = node_dir().ok_or_else(|| format!("{name} needs Node.js {NODE_MIN} or newer. Install it (for example `nvm install {NODE_MIN}`, or nodejs from your package manager), then run this again."))?;
                let pkg = t.dist.as_ref().and_then(|d| d.package.clone()).unwrap_or_else(|| name.to_string());
                // Use that Node's own npm; if its link is broken, any npm on PATH runs
                // fine under it (npm is a Node script).
                let npm = Some(dir.join("npm")).filter(|p| p.exists()).or_else(|| run(Command::new("sh").args(["-c", "command -v npm"])).ok().map(|p| PathBuf::from(p.trim()))).ok_or("npm wasn't found")?;
                run(Command::new(npm)
                    .env("PATH", tool_path())
                    .args(["install", "--no-audit", "--no-fund", "--omit=dev", "--prefix"])
                    .arg(&staging)
                    .arg(format!("{pkg}@{}", t.version)))?;
                Ok(())
            }
            other => Err(t.note.clone().unwrap_or_else(|| format!("install method `{other}` is not supported"))),
        }
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    // Canary: the binary must start on this machine before it goes live.
    if let Err(e) = canary_at(&staging, t) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("{name} downloaded but failed its smoke test: {e}"));
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| e.to_string())?;
    Ok(format!("{name} {} installed and verified", t.version))
}

/// Unpack a release `.zip` into `dest`, dropping its single top-level folder.
fn unzip_flat(zip: &Path, dest: &Path) -> Result<(), String> {
    let tmp = dest.join(".unzip");
    let _ = std::fs::remove_dir_all(&tmp);
    run(Command::new("unzip").arg("-q").arg(zip).arg("-d").arg(&tmp))?;
    let entries: Vec<PathBuf> = std::fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    let top = match entries.as_slice() {
        [only] if only.is_dir() => only.clone(),
        _ => tmp.clone(),
    };
    for e in std::fs::read_dir(&top).map_err(|e| e.to_string())?.flatten() {
        std::fs::rename(e.path(), dest.join(e.file_name())).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// Node.js the npm tools need (HyperFrames requires 22).
pub const NODE_MIN: u32 = 22;

fn node_major(bin: &Path) -> Option<u32> {
    let v = run(Command::new(bin).arg("--version")).ok()?;
    v.trim().trim_start_matches('v').split('.').next()?.parse().ok()
}

/// Every Node.js on this machine we can find: the PATH, Homebrew, /usr, and nvm.
fn node_candidates() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    if let Ok(all) = run(Command::new("sh").args(["-c", "which -a node 2>/dev/null"])) {
        found.extend(all.lines().map(|l| PathBuf::from(l.trim())));
    }
    found.extend(["/opt/homebrew/bin/node", "/usr/local/bin/node", "/usr/bin/node"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME") {
        if let Ok(rd) = std::fs::read_dir(Path::new(&home).join(".nvm/versions/node")) {
            found.extend(rd.flatten().map(|e| e.path().join("bin/node")));
        }
    }
    found.retain(|p| p.exists());
    found.dedup();
    found
}

/// The folder of the oldest Node.js that still meets `NODE_MIN` (least likely to
/// surprise a tool), or `None` when this machine has no suitable Node.
pub fn node_dir() -> Option<PathBuf> {
    node_candidates()
        .into_iter()
        .filter_map(|p| node_major(&p).filter(|m| *m >= NODE_MIN).map(|m| (m, p)))
        .min_by_key(|(m, _)| *m)
        .and_then(|(_, p)| p.parent().map(Path::to_path_buf))
}

/// A minimal PATH for running tools: a suitable Node.js first (npm tools such as
/// HyperFrames are Node scripts), then the system dirs.
pub fn tool_path() -> String {
    let mut dirs = vec!["/usr/local/bin".to_string(), "/usr/bin".to_string(), "/bin".to_string()];
    // Homebrew on Apple Silicon (ffmpeg, Chrome helpers for video renders).
    if Path::new("/opt/homebrew/bin").is_dir() {
        dirs.insert(0, "/opt/homebrew/bin".to_string());
    }
    if let Some(d) = node_dir() {
        dirs.insert(0, d.to_string_lossy().to_string());
    }
    dirs.join(":")
}

fn canary_at(dir: &Path, t: &LockedTool) -> Result<String, String> {
    let bin = dir.join(t.program());
    if !bin.exists() {
        return Err(format!("{} missing after install", t.program()));
    }
    let out = Command::new(&bin).arg("--help").env_clear().env("PATH", tool_path()).env("HOME", dir).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() || text.to_ascii_lowercase().contains("usage") {
        Ok(text.lines().next().unwrap_or_default().to_string())
    } else {
        Err(text.chars().take(400).collect())
    }
}

/// Re-run every installed tool's smoke test (used before promoting updates).
pub fn verify(lock: &ToolsLock, tools_dir: &Path) -> Vec<(String, Result<String, String>)> {
    lock.tool
        .iter()
        .filter(|(n, t)| !t.program().is_empty() && root(tools_dir, n, t).join(t.program()).exists())
        .map(|(n, t)| (n.clone(), canary_at(&root(tools_dir, n, t), t)))
        .collect()
}
