//! `tools.lock.toml` — every external tool, pinned. The updater is the only
//! thing that edits it, after a staged canary run passes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsLock {
    #[serde(default)]
    pub tool: BTreeMap<String, LockedTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedTool {
    /// `owner/repo` on GitHub.
    pub repo: String,
    pub version: String,
    /// How it is built/installed: `cargo`, `npm`, `release-binary`, `font-mirror`.
    pub install: String,
    /// Binary to run, relative to `<tools_dir>/<name>/<version>/`.
    pub bin: String,
    /// Subcommand that starts its MCP server, if it has one.
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
    /// Commit hash or artifact sha256 the version resolved to.
    #[serde(default)]
    pub pin: Option<String>,
    pub license: String,
    /// Does the tool need network inside the sandbox?
    #[serde(default)]
    pub network: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_mem")]
    pub memory_mb: u64,
    /// Where the artifact comes from and its pinned hashes per CPU.
    #[serde(default)]
    pub dist: Option<Dist>,
    /// Shown when a tool can't be installed on this platform.
    #[serde(default)]
    pub note: Option<String>,
    /// Arguments for the post-install smoke test (default `--help`).
    #[serde(default)]
    pub canary: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dist {
    /// Release asset name; `{v}` = version without a leading `v`, `{arch}` =
    /// aarch64|x86_64, `{arch_short}` = arm64|x64.
    #[serde(default)]
    pub asset: Option<String>,
    #[serde(default)]
    pub sha256_aarch64: Option<String>,
    #[serde(default)]
    pub sha256_x86_64: Option<String>,
    /// npm package name (install = "npm").
    #[serde(default)]
    pub package: Option<String>,
    /// install = "bundled-node": source shipped in the release under `tools/<name>`,
    /// run with this pinned Node.js from nodejs.org (SHA-256 per CPU).
    #[serde(default)]
    pub node_version: Option<String>,
    #[serde(default)]
    pub node_sha256_aarch64: Option<String>,
    #[serde(default)]
    pub node_sha256_x86_64: Option<String>,
}

fn default_timeout() -> u64 {
    300
}
fn default_mem() -> u64 {
    2048
}

impl ToolsLock {
    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// The lock file compiled into the binary (fallback when the on-disk one is missing).
    pub fn embedded() -> Self {
        Self::parse(include_str!("../../../tools.lock.toml")).expect("embedded tools.lock.toml is valid")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_lock_parses_and_covers_suite() {
        let l = super::ToolsLock::embedded();
        for t in ["photocraft", "vectorcraft", "pdfcraft", "kokoros", "hyperframes", "open-design", "impeccable"] {
            assert!(l.tool.contains_key(t), "missing {t}");
        }
        for (name, t) in &l.tool {
            if t.install.starts_with("github-") {
                let d = t.dist.as_ref().unwrap_or_else(|| panic!("{name}: dist missing"));
                for h in [&d.sha256_aarch64, &d.sha256_x86_64] {
                    assert_eq!(h.as_deref().map(str::len), Some(64), "{name}: every downloadable tool is hash-pinned per CPU");
                }
            }
        }
    }
}
