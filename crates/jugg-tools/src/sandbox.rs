//! Running untrusted-ish external binaries.
//!
//! On Linux every external tool runs under bubblewrap:
//! read-only `/usr`, `/lib`, `/etc/ssl`, the tool's own install dir read-only,
//! the job dir as the **only** writable path, private `/tmp`, new PID/IPC/UTS
//! namespaces, no network unless the lock file says `network = true`, and
//! `--die-with-parent`. The service unit adds cgroup MemoryMax / CPUQuota.
//! On other platforms (developer laptops) it falls back to a plain child
//! process with a cleared environment.

use crate::ToolError;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub tool_root: PathBuf,
    pub job_dir: PathBuf,
    pub network: bool,
    pub timeout: Duration,
    pub env: Vec<(String, String)>,
}

pub struct RunResult {
    pub status: i32,
    pub stdout: String,
    pub stderr_tail: String,
}

const MAX_CAPTURE: usize = 2 * 1024 * 1024;

pub fn build_command(spec: &SandboxSpec) -> Command {
    let use_bwrap = cfg!(target_os = "linux") && which("bwrap").is_some();
    let mut cmd = if use_bwrap {
        let mut c = Command::new("bwrap");
        for ro in ["/usr", "/lib", "/lib64", "/bin", "/etc/ssl", "/etc/fonts", "/etc/resolv.conf"] {
            if Path::new(ro).exists() {
                c.args(["--ro-bind", ro, ro]);
            }
        }
        let root = spec.tool_root.to_string_lossy().to_string();
        let job = spec.job_dir.to_string_lossy().to_string();
        c.args(["--ro-bind", &root, &root, "--bind", &job, "/work", "--chdir", "/work"]);
        c.args(["--tmpfs", "/tmp", "--proc", "/proc", "--dev", "/dev"]);
        c.args(["--unshare-pid", "--unshare-ipc", "--unshare-uts", "--unshare-cgroup-try", "--die-with-parent", "--new-session"]);
        if !spec.network {
            c.arg("--unshare-net");
        }
        c.arg("--clearenv");
        c.arg(&spec.program);
        c
    } else {
        let mut c = Command::new(&spec.program);
        c.current_dir(&spec.job_dir);
        c.env_clear();
        c
    };
    cmd.args(&spec.args);
    // Inside bubblewrap the system dirs are all there is; outside it (development
    // on a Mac), add wherever Node lives so npm tools like HyperFrames can start.
    cmd.env("PATH", if use_bwrap { "/usr/local/bin:/usr/bin:/bin".to_string() } else { crate::install::tool_path() });
    cmd.env("HOME", if use_bwrap { "/work".into() } else { spec.job_dir.to_string_lossy().to_string() });
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.kill_on_drop(true);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd
}

pub async fn run(spec: &SandboxSpec) -> Result<RunResult, ToolError> {
    if !spec.program.exists() {
        return Err(ToolError::NotInstalled(spec.program.display().to_string()));
    }
    let child = build_command(spec).spawn().map_err(|e| ToolError::Failed(e.to_string()))?;
    let out = tokio::time::timeout(spec.timeout, child.wait_with_output())
        .await
        .map_err(|_| ToolError::Timeout(spec.timeout.as_secs()))?
        .map_err(|e| ToolError::Failed(e.to_string()))?;
    let mut stdout = String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(MAX_CAPTURE)]).to_string();
    if out.stdout.len() > MAX_CAPTURE {
        stdout.push_str("\n…[truncated]");
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: String = err.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
    Ok(RunResult { status: out.status.code().unwrap_or(-1), stdout, stderr_tail: tail })
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(bin)).find(|p| p.exists()))
}
