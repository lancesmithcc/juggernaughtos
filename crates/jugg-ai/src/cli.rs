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

pub struct ClaudeCodeCli {
    pub bin: String,
    pub model: Option<String>,
}

pub struct CodexCli {
    pub bin: String,
    pub model: Option<String>,
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
        let mcp = json!({"mcpServers": {"jugg": {"command": req.mcp_command.0, "args": req.mcp_command.1}}});
        let mut cmd = Command::new(&self.bin);
        cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--strict-mcp-config"])
            .args(["--mcp-config", &mcp.to_string()])
            .args(["--allowedTools", "mcp__jugg"])
            // Built-in tools off: the CLI may only act through Juggernaught's tools.
            .args(["--disallowedTools", "Bash,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Task"])
            .args(["--append-system-prompt", &req.system]);
        if let Some(m) = &self.model {
            cmd.args(["--model", m]);
        }
        cmd.current_dir(&req.ctx.job_dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
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
        let mut cmd = Command::new(&self.bin);
        cmd.args(["exec", "--json", "--skip-git-repo-check", "--sandbox", "read-only"])
            .args(["-c", &format!("mcp_servers.jugg.command={}", json!(req.mcp_command.0))])
            .args(["-c", &format!("mcp_servers.jugg.args={args_toml}")]);
        if let Some(m) = &self.model {
            cmd.args(["--model", m]);
        }
        cmd.arg("-");
        cmd.current_dir(&req.ctx.job_dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
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
    fn parses_codex_stream() {
        let ev = parse_codex(&json!({"type":"item.completed","item":{"type":"agent_message","text":"Done"}}));
        assert!(matches!(&ev[0], AgentEvent::TextDelta { text } if text == "Done"));
        let u = parse_codex(&json!({"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}));
        assert!(matches!(&u[0], AgentEvent::Usage { input_tokens: 10, output_tokens: 5, .. }));
    }
}
