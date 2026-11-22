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
            let installed = !t.bin.is_empty() && root(tools_dir, name, t).join(&t.bin).exists();
            let note = if installed {
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
    if !t.bin.is_empty() && dest.join(&t.bin).exists() {
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
                let asset = d.asset.as_ref().ok_or("lock entry has no asset")?.replace("{v}", v).replace("{arch_short}", a_short).replace("{arch}", a);
                let pin = if a == "aarch64" { &d.sha256_aarch64 } else { &d.sha256_x86_64 };
                let pin = pin.as_ref().ok_or("no pinned hash for this CPU")?;
                let url = format!("https://github.com/{}/releases/download/{}/{}", t.repo, t.version, asset);
                let file = staging.join(".download");
                run(Command::new("curl").args(["-fsSL", "--proto", "=https", "--tlsv1.2", "--retry", "3", "-o"]).arg(&file).arg(&url))?;
                let got = sha256_file(&file)?;
                if &got != pin {
                    return Err(format!("checksum mismatch for {asset}: expected {pin}, got {got}. Refusing to install."));
                }
                if t.install == "github-tarball" {
                    run(Command::new("tar").args(["-xzf"]).arg(&file).args(["--strip-components=1", "-C"]).arg(&staging))?;
                } else {
                    let bin = staging.join(&t.bin);
                    std::fs::create_dir_all(bin.parent().unwrap_or(&staging)).map_err(|e| e.to_string())?;
                    std::fs::rename(&file, &bin).map_err(|e| e.to_string())?;
                }
                let _ = std::fs::remove_file(&file);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(staging.join(&t.bin), std::fs::Permissions::from_mode(0o755));
                }
                Ok(())
            }
            "npm" => {
                let node = run(Command::new("node").arg("--version")).map_err(|_| "Node.js 20+ is required (apt install nodejs)".to_string())?;
                let major: u32 = node.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
                if major < 20 {
                    return Err(format!("Node.js 20+ is required, found {}", node.trim()));
                }
                let pkg = t.dist.as_ref().and_then(|d| d.package.clone()).unwrap_or_else(|| name.to_string());
                run(Command::new("npm").args(["install", "--no-audit", "--no-fund", "--omit=dev", "--prefix"]).arg(&staging).arg(format!("{pkg}@{}", t.version)))?;
                Ok(())
            }
            "bundled-node" => install_bundled_node(name, t, &staging, a),
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

/// Where a bundled tool's source lives: `tools/<name>` next to the release (or the
/// repository in development).
pub fn bundled_source(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dirs: Vec<PathBuf> = exe.ancestors().skip(1).take(4).map(Path::to_path_buf).collect();
    dirs.extend(std::env::current_dir().ok());
    dirs.into_iter().map(|d| d.join("tools").join(name)).find(|p| p.join("package-lock.json").exists())
}

fn install_bundled_node(name: &str, t: &LockedTool, staging: &Path, a: &str) -> Result<(), String> {
    let src = bundled_source(name).ok_or_else(|| format!("tools/{name} is missing from this release"))?;
    for entry in std::fs::read_dir(&src).map_err(|e| e.to_string())?.flatten() {
        let f = entry.file_name().to_string_lossy().to_string();
        if entry.path().is_file() && !f.starts_with('.') {
            std::fs::copy(entry.path(), staging.join(&f)).map_err(|e| e.to_string())?;
        }
    }
    // Pinned Node.js (distro packages are often too old for Playwright). Pins are for
    // Linux servers; a macOS development machine uses its own Node 20+.
    if cfg!(target_os = "macos") {
        let v = run(Command::new("node").arg("--version")).map_err(|_| "Node.js 20+ is required".to_string())?;
        let major: u32 = v.trim().trim_start_matches('v').split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
        if major < 20 {
            return Err(format!("Node.js 20+ is required, found {}", v.trim()));
        }
        // The wrapper looks for node next to itself; point that at this Mac's Node.
        let system_node = run(Command::new("sh").args(["-c", "command -v node"]))?.trim().to_string();
        std::fs::create_dir_all(staging.join("node/bin")).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&system_node, staging.join("node/bin/node")).map_err(|e| e.to_string())?;
        run(Command::new("npm").args(["ci", "--omit=dev", "--no-audit", "--no-fund", "--ignore-scripts"]).current_dir(staging))?;
        run(Command::new("node").arg(staging.join("node_modules/playwright-core/cli.js")).args(["install", "--no-shell", "chromium"]).current_dir(staging).env("PLAYWRIGHT_BROWSERS_PATH", staging.join("browsers")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(staging.join(&t.bin), std::fs::Permissions::from_mode(0o755));
        }
        return Ok(());
    }
    let d = t.dist.as_ref().ok_or("lock entry has no dist")?;
    let nv = d.node_version.as_ref().ok_or("lock entry has no node_version")?;
    let (node_arch, pin) = if a == "aarch64" { ("arm64", &d.node_sha256_aarch64) } else { ("x64", &d.node_sha256_x86_64) };
    let pin = pin.as_ref().ok_or("no pinned Node.js hash for this CPU")?;
    let os = if cfg!(target_os = "macos") { "darwin" } else { "linux" };
    let asset = format!("node-{nv}-{os}-{node_arch}.tar.gz");
    let file = staging.join(".node.tar.gz");
    run(Command::new("curl").args(["-fsSL", "--proto", "=https", "--tlsv1.2", "--retry", "3", "-o"]).arg(&file).arg(format!("https://nodejs.org/dist/{nv}/{asset}")))?;
    let got = sha256_file(&file)?;
    if &got != pin {
        return Err(format!("checksum mismatch for {asset}: expected {pin}, got {got}. Refusing to install."));
    }
    std::fs::create_dir_all(staging.join("node")).map_err(|e| e.to_string())?;
    run(Command::new("tar").arg("-xzf").arg(&file).args(["--strip-components=1", "-C"]).arg(staging.join("node")))?;
    let _ = std::fs::remove_file(&file);
    let node = staging.join("node/bin/node");
    let npm = staging.join("node/lib/node_modules/npm/bin/npm-cli.js");
    let path = format!("{}:/usr/local/bin:/usr/bin:/bin", staging.join("node/bin").display());
    // `npm ci` installs exactly the lockfile and checks every package's integrity hash.
    run(Command::new(&node).arg(&npm).args(["ci", "--omit=dev", "--no-audit", "--no-fund", "--ignore-scripts"]).current_dir(staging).env("PATH", &path))?;
    let cli = staging.join("node_modules/playwright-core/cli.js");
    if cli.exists() {
        let browsers = staging.join("browsers");
        run(Command::new(&node).arg(&cli).args(["install", "--no-shell", "chromium"]).current_dir(staging).env("PATH", &path).env("PLAYWRIGHT_BROWSERS_PATH", &browsers))?;
        // Shared libraries Chromium needs (apt; only possible as root on Linux).
        if cfg!(target_os = "linux") && run(Command::new("id").arg("-u")).map(|u| u.trim() == "0").unwrap_or(false) {
            run(Command::new(&node).arg(&cli).args(["install-deps", "chromium"]).current_dir(staging).env("PATH", &path).env("DEBIAN_FRONTEND", "noninteractive"))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(staging.join(&t.bin), std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

fn canary_at(dir: &Path, t: &LockedTool) -> Result<String, String> {
    let bin = dir.join(&t.bin);
    if !bin.exists() {
        return Err(format!("{} missing after install", t.bin));
    }
    let args = t.canary.clone().unwrap_or_else(|| vec!["--help".into()]);
    let out = Command::new(&bin).args(&args).env_clear().env("PATH", "/usr/bin:/bin").env("HOME", dir).output().map_err(|e| e.to_string())?;
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
        .filter(|(n, t)| !t.bin.is_empty() && root(tools_dir, n, t).join(&t.bin).exists())
        .map(|(n, t)| (n.clone(), canary_at(&root(tools_dir, n, t), t)))
        .collect()
}
