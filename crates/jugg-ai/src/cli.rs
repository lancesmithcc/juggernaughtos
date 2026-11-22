//! Agentic CLI providers: Claude Code and OpenAI Codex, run headless.
//!
//! Each runs with a private working directory, our MCP server as its only
//! tool source, and its own shell / file-edit tools disabled. The owner logs
//! the CLI in once on the box (`juggernaught connect claude|codex` runs the
//! vendor's own login flow; we never see or store their credentials).

use crate::{AgentEvent, AgentRequest, AiError, Provider, ProviderKind, Role};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Where an agent CLI may look. Everything else on the machine (keys, the
/// database, other customers' files) simply doesn't exist inside the sandbox.
#[derive(Debug, Clone)]
pub struct CliSandbox {
    /// The CLI's own HOME (its login lives here), read-write.
    pub home: std::path::PathBuf,
    pub read_only: Vec<std::path::PathBuf>,
    pub read_write: Vec<std::path::PathBuf>,
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    if bin.contains('/') {
        return Some(bin.into());
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(bin)).find(|p| p.exists()))
}

/// Build the command for an agent CLI, sandboxed when a profile is given.
pub fn cli_command(bin: &str, sandbox: Option<&CliSandbox>, cwd: &std::path::Path) -> Command {
    let Some(sb) = sandbox else {
        let mut c = Command::new(bin);
        c.current_dir(cwd);
        return c;
    };
    let real = which(bin).or_else(|| Some(sb.home.join(".local/bin").join(bin))).unwrap_or_else(|| bin.into());
    let mut c = Command::new("bwrap");
    for ro in ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"] {
        c.args(["--ro-bind-try", ro, ro]);
    }
    // Our own config and sealed credentials are not the agent's business.
    if std::path::Path::new("/etc/juggernaught").exists() {
        c.args(["--tmpfs", "/etc/juggernaught"]);
    }
    for p in &sb.read_only {
        let p = p.to_string_lossy().to_string();
        c.args(["--ro-bind-try", &p, &p]);
    }
    for p in sb.read_write.iter().chain(std::iter::once(&sb.home)) {
        let _ = std::fs::create_dir_all(p);
        let p = p.to_string_lossy().to_string();
        c.args(["--bind", &p, &p]);
    }
    let home = sb.home.to_string_lossy().to_string();
    c.args(["--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--unshare-pid", "--unshare-ipc", "--unshare-uts", "--die-with-parent", "--new-session"]);
    c.args(["--clearenv", "--setenv", "HOME", &home, "--setenv", "PATH", &format!("{home}/.local/bin:/usr/local/bin:/usr/bin:/bin"), "--setenv", "LANG", "C.UTF-8"]);
    c.arg("--chdir").arg(cwd);
    c.arg(real);
    c
}

pub struct ClaudeCodeCli {
    pub bin: String,
    pub model: Option<String>,
}

pub struct CodexCli {
    pub bin: String,
    pub model: Option<String>,
}

/// Every Claude Code built-in that could read, write, run or fetch anything.
pub const DISALLOWED_CLAUDE_TOOLS: &str = "Bash,BashOutput,KillShell,Edit,Write,MultiEdit,NotebookEdit,NotebookRead,Read,Glob,Grep,LS,WebFetch,WebSearch,Task,TodoWrite,SlashCommand,Skill";

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(bytes)
}

fn transcript(req: &AgentRequest) -> String {
    let mut s = String::new();
    for m in &req.history {
        let who = match m.role {
            Role::User => "Owner",
            Role::Assistant => "You",
            Role::System => "Note",
        };
        s.push_str(&format!("{who}: {}\n\n", m.content));
    }
    s.push_str(&format!("Owner: {}\n", req.user));
    s
}

async fn version(bin: &str) -> Result<String, AiError> {
    let out = Command::new(bin).arg("--version").output().await.map_err(|e| AiError::Unavailable(format!("{bin}: {e}")))?;
    if !out.status.success() {
        return Err(AiError::Unavailable(format!("{bin} --version failed")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn pump(mut child: tokio::process::Child, input: String, tx: &mpsc::Sender<AgentEvent>, mut parse: impl FnMut(&Value) -> Vec<AgentEvent>) -> Result<(), AiError> {
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).await?;
        drop(stdin);
    }
    let stdout = child.stdout.take().ok_or_else(|| AiError::Provider("no stdout".into()))?;
    let mut lines = BufReader::new(stdout).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        for ev in parse(&v) {
            if tx.send(ev).await.is_err() {
                let _ = child.kill().await;
                return Ok(());
            }
        }
    }
    let status = child.wait().await?;
    if !status.success() {
        let _ = tx.send(AgentEvent::Error { message: format!("provider exited with {status}") }).await;
    }
    let _ = tx.send(AgentEvent::Done).await;
    Ok(())
}

#[async_trait]
impl Provider for ClaudeCodeCli {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ClaudeCodeCli
    }
    fn label(&self) -> String {
        "Claude Code".into()
    }
    async fn health(&self) -> Result<String, AiError> {
        version(&self.bin).await
    }

    async fn run(&self, req: AgentRequest, tx: mpsc::Sender<AgentEvent>) -> Result<(), AiError> {
        let env: serde_json::Map<String, Value> = req.mcp_env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        let mcp = json!({"mcpServers": {"jugg": {"command": req.mcp_command.0, "args": req.mcp_command.1, "env": env}}});
        // Config in a private file, not argv (argv is visible to other processes).
        let mcp_file = req.ctx.job_dir.join(".mcp.json");
        write_private(&mcp_file, mcp.to_string().as_bytes())?;
        let mut cmd = cli_command(&self.bin, req.sandbox.as_ref(), &req.ctx.job_dir);
        cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--strict-mcp-config"])
            .arg("--mcp-config")
            .arg(&mcp_file)
            .args(["--allowedTools", "mcp__jugg"])
            // Built-in tools off — including the read/search tools — so the CLI can
            // only act through Juggernaught's tools (and the sandbox hides the rest).
            .args(["--disallowedTools", DISALLOWED_CLAUDE_TOOLS])
            .args(["--append-system-prompt", &req.system]);
        if let Some(m) = &self.model {
            cmd.args(["--model", m]);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        // The config holds this turn's MCP token: it must not outlive the run, even on error.
        struct RemoveOnDrop(std::path::PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = RemoveOnDrop(mcp_file.clone());
        let child = cmd.spawn().map_err(|e| AiError::Unavailable(e.to_string()))?;
        let mut parser = ClaudeParser::default();
        pump(child, transcript(&req), &tx, move |v| parser.parse(v)).await
    }
}

/// Claude Code `stream-json`: one JSON object per line. Stateful because
/// `tool_result` blocks only carry the `tool_use_id`; we remember which of
/// those were Juggernaught tools and ignore Claude Code's own internals.
#[derive(Default)]
pub struct ClaudeParser {
    ours: std::collections::HashMap<String, String>,
}

const PREFIX: &str = "mcp__jugg__";

impl ClaudeParser {
    pub fn parse(&mut self, v: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match v["type"].as_str() {
            Some("assistant") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => out.push(AgentEvent::TextDelta { text: block["text"].as_str().unwrap_or_default().into() }),
                        Some("tool_use") => {
                            let name = block["name"].as_str().unwrap_or_default();
                            if let Some(short) = name.strip_prefix(PREFIX) {
                                self.ours.insert(block["id"].as_str().unwrap_or_default().into(), short.into());
                                out.push(AgentEvent::ToolStarted { name: short.into(), args: block["input"].clone() });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for block in v["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let Some(name) = self.ours.remove(block["tool_use_id"].as_str().unwrap_or_default()) else { continue };
                    let text = match &block["content"] {
                        Value::String(s) => s.clone(),
                        Value::Array(a) => a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    // Structured results carry their own one-line summary.
                    let summary = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|j| j["summary"].as_str().map(str::to_string))
                        .unwrap_or_else(|| text.lines().find(|l| !l.trim().is_empty()).unwrap_or("done").trim_start_matches('#').trim().to_string());
                    out.push(AgentEvent::ToolFinished { name, summary, is_error: block["is_error"].as_bool().unwrap_or(false) });
                }
            }
            Some("result") => out.push(AgentEvent::Usage {
                input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
                model: "claude-code".into(),
            }),
            _ => {}
        }
        out
    }
}

#[async_trait]
impl Provider for CodexCli {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CodexCli
    }
    fn label(&self) -> String {
        "Codex".into()
    }
    async fn health(&self) -> Result<String, AiError> {
        version(&self.bin).await
    }

    async fn run(&self, req: AgentRequest, tx: mpsc::Sender<AgentEvent>) -> Result<(), AiError> {
        let args_toml = serde_json::to_string(&req.mcp_command.1).unwrap_or_else(|_| "[]".into());
        let env_toml = format!("{{{}}}", req.mcp_env.iter().map(|(k, v)| format!("{k}={}", json!(v))).collect::<Vec<_>>().join(","));
        let mut cmd = cli_command(&self.bin, req.sandbox.as_ref(), &req.ctx.job_dir);
        cmd.args(["exec", "--json", "--skip-git-repo-check", "--sandbox", "read-only"])
            .args(["-c", &format!("mcp_servers.jugg.command={}", json!(req.mcp_command.0))])
            .args(["-c", &format!("mcp_servers.jugg.args={args_toml}")])
            .args(["-c", &format!("mcp_servers.jugg.env={env_toml}")]);
        if let Some(m) = &self.model {
            cmd.args(["--model", m]);
        }
        cmd.arg("-");
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| AiError::Unavailable(e.to_string()))?;
        let input = format!("{}\n\n---\n\n{}", req.system, transcript(&req));
        pump(child, input, &tx, parse_codex).await
    }
}

/// Codex `exec --json`: `item.*` and `turn.*` events.
fn parse_codex(v: &Value) -> Vec<AgentEvent> {
    let item = &v["item"];
    match (v["type"].as_str(), item["type"].as_str()) {
        (Some("item.completed"), Some("agent_message")) => vec![AgentEvent::TextDelta { text: item["text"].as_str().unwrap_or_default().into() }],
        (Some("item.started"), Some("mcp_tool_call")) => vec![AgentEvent::ToolStarted { name: item["tool"].as_str().unwrap_or_default().into(), args: item["arguments"].clone() }],
        (Some("item.completed"), Some("mcp_tool_call")) => vec![AgentEvent::ToolFinished {
            name: item["tool"].as_str().unwrap_or_default().into(),
            summary: item["result"].to_string(),
            is_error: item["status"] == "failed",
        }],
        (Some("turn.completed"), _) => vec![AgentEvent::Usage {
            input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
            model: "codex".into(),
        }],
        (Some("error"), _) => vec![AgentEvent::Error { message: v["message"].as_str().unwrap_or("codex error").into() }],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_stream() {
        let mut p = ClaudeParser::default();
        let ev = p.parse(&json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"Hi"},
            {"type":"tool_use","id":"t1","name":"mcp__jugg__vectorize","input":{"input":"a.png"}},
            {"type":"tool_use","id":"t2","name":"ToolSearch","input":{}}]}}));
        assert_eq!(ev.len(), 2, "Claude Code internals are hidden");
        assert!(matches!(&ev[0], AgentEvent::TextDelta { text } if text == "Hi"));
        assert!(matches!(&ev[1], AgentEvent::ToolStarted { name, .. } if name == "vectorize"));
        let done = p.parse(&json!({"type":"user","message":{"content":[
            {"type":"tool_result","tool_use_id":"t2","content":"internal"},
            {"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"Vectorized → logo.svg (3 colours)"}]}]}}));
        assert_eq!(done.len(), 1);
        assert!(matches!(&done[0], AgentEvent::ToolFinished { name, summary, .. } if name == "vectorize" && summary.starts_with("Vectorized")));
    }

    #[test]
    fn sandbox_hides_everything_but_the_job() {
        let sb = CliSandbox { home: "/var/lib/juggernaught/agent-home".into(), read_only: vec!["/opt/juggernaught".into()], read_write: vec!["/var/lib/juggernaught/run".into()] };
        let c = cli_command("claude", Some(&sb), std::path::Path::new("/var/lib/juggernaught/jobs/x"));
        let args: Vec<String> = c.as_std().get_args().map(|a| a.to_string_lossy().to_string()).collect();
        let joined = args.join(" ");
        assert_eq!(c.as_std().get_program(), "bwrap");
        assert!(joined.contains("--clearenv") && joined.contains("--unshare-pid") && joined.contains("--die-with-parent"));
        assert!(!args.iter().any(|a| a == "/var/lib/juggernaught" || a.ends_with("/keys") || a.ends_with(".db")), "keys and database are never mounted: {joined}");
        assert!(DISALLOWED_CLAUDE_TOOLS.contains("Read") && DISALLOWED_CLAUDE_TOOLS.contains("Grep") && DISALLOWED_CLAUDE_TOOLS.contains("Bash"));
    }

    /// Runs only where bubblewrap works (Linux with user namespaces): proves the
    /// sandbox really hides the data dir and the parent's environment.
    #[tokio::test]
    async fn sandbox_really_isolates_on_linux() {
        if !std::process::Command::new("bwrap").args(["--ro-bind", "/", "/", "--proc", "/proc", "--unshare-pid", "true"]).status().map(|s| s.success()).unwrap_or(false) {
            eprintln!("bwrap unavailable here; skipping");
            return;
        }
        let base = tempfile::tempdir_in("/var/tmp").unwrap();
        let data = base.path().join("data");
        let keys = data.join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        std::fs::write(keys.join("kek-1.key"), b"secret").unwrap();
        let job = data.join("jobs/j1");
        std::fs::create_dir_all(&job).unwrap();
        let sb = CliSandbox { home: data.join("agent-home"), read_only: vec![], read_write: vec![job.clone()] };
        std::env::set_var("JUGG_TEST_SECRET", "leak");
        let script = format!(
            "test -e {k} && echo SEEN_KEY; test -e {d}/jobs && ls {d} | tr '\\n' ' ' | sed 's/^/DATA:/'; echo ok > {j}/out && echo WROTE; echo HOME=$HOME; env | grep -c JUGG_TEST_SECRET | sed 's/^/ENV:/'; ps -e 2>/dev/null | wc -l | sed 's/^/PROCS:/'",
            k = keys.join("kek-1.key").display(),
            d = data.display(),
            j = job.display()
        );
        let mut c = cli_command("sh", Some(&sb), &job);
        c.arg("-c").arg(&script);
        let out = c.output().await.unwrap();
        let text = format!("{}\nstderr: {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(!text.contains("SEEN_KEY"), "key visible inside sandbox: {text}");
        assert!(text.contains("WROTE") && std::fs::read_to_string(job.join("out")).unwrap().trim() == "ok", "job dir must be writable: {text}");
        assert!(text.contains(&format!("HOME={}", data.join("agent-home").display())), "{text}");
        assert!(text.contains("ENV:0"), "parent environment leaked: {text}");
        if let Some(line) = text.lines().find(|l| l.starts_with("DATA:")) {
            assert!(!line.contains("keys"), "data dir listing shows keys: {line}");
        }
    }

    #[test]
    fn parses_codex_stream() {
        let ev = parse_codex(&json!({"type":"item.completed","item":{"type":"agent_message","text":"Done"}}));
        assert!(matches!(&ev[0], AgentEvent::TextDelta { text } if text == "Done"));
        let u = parse_codex(&json!({"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}));
        assert!(matches!(&u[0], AgentEvent::Usage { input_tokens: 10, output_tokens: 5, .. }));
    }
}
