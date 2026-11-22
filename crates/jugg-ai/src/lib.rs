//! Intelligence layer. Providers are interchangeable; tools are not.
//!
//! Two provider shapes, one tool surface:
//! * **Agentic CLIs** (Claude Code, Codex) run their own loop. We launch them
//!   headless with *only* our MCP server (`juggernaught mcp`) attached and
//!   their built-in shell/file tools disabled, and translate their JSON event
//!   streams into [`AgentEvent`]s.
//! * **Chat APIs** (OpenRouter, Anthropic, OpenAI) are driven by our own
//!   [`agent_loop`] against the same [`jugg_tools::Registry`].
//!
//! Either way the approval policy lives in `Registry::call`, so a provider can
//! never publish or delete without the owner clicking Approve.

pub mod agent_loop;
pub mod cli;
pub mod openrouter;
pub mod prompt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    ClaudeCodeCli,
    CodexCli,
    OpenRouter,
    AnthropicApi,
    OpenAiApi,
    /// No provider configured yet: guides the owner through setup.
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// Streamed to the admin UI over SSE.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TextDelta { text: String },
    ToolStarted { name: String, args: serde_json::Value },
    ToolFinished { name: String, summary: String, is_error: bool },
    ApprovalRequired { tool: String, summary: String },
    Usage { input_tokens: u64, output_tokens: u64, model: String },
    /// New assets registered from this turn's work (asset cards for the UI).
    Assets { items: serde_json::Value },
    /// Actions waiting for the owner's Approve / Decline.
    Approvals { items: serde_json::Value },
    Done,
    Error { message: String },
}

pub struct AgentRequest {
    pub system: String,
    pub history: Vec<ChatMessage>,
    pub user: String,
    /// Command that starts our MCP server (for CLI providers).
    pub mcp_command: (String, Vec<String>),
    pub registry: jugg_tools::Registry,
    pub ctx: jugg_tools::ToolContext,
}

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("provider not available: {0}")]
    Unavailable(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn label(&self) -> String;
    /// Cheap readiness check shown in Settings → Intelligence.
    async fn health(&self) -> Result<String, AiError>;
    async fn run(&self, req: AgentRequest, tx: mpsc::Sender<AgentEvent>) -> Result<(), AiError>;
}

/// Used until the owner connects a provider in the setup wizard.
pub struct Offline;

#[async_trait]
impl Provider for Offline {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Offline
    }
    fn label(&self) -> String {
        "Not connected".into()
    }
    async fn health(&self) -> Result<String, AiError> {
        Err(AiError::Unavailable("no provider connected".into()))
    }
    async fn run(&self, _req: AgentRequest, tx: mpsc::Sender<AgentEvent>) -> Result<(), AiError> {
        let text = "I'm not connected to an intelligence provider yet. Open Settings → Intelligence and connect \
                    Claude Code, Codex, or an OpenRouter key, and I'll get to work.";
        for chunk in text.split_inclusive(' ') {
            let _ = tx.send(AgentEvent::TextDelta { text: chunk.into() }).await;
            tokio::time::sleep(std::time::Duration::from_millis(18)).await;
        }
        let _ = tx.send(AgentEvent::Done).await;
        Ok(())
    }
}
