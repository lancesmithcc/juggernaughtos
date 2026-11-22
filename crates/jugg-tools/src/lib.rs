//! The tool layer: everything the AI can *do*.
//!
//! * [`Tool`] — one capability with a JSON-Schema input, a [`Risk`] level and a runner.
//! * [`Registry`] — what the agent loop lists to API providers and what the
//!   MCP bridge ([`mcp`]) exposes to Claude Code / Codex CLI subprocesses, so
//!   every provider sees the exact same tools and the same approval policy.
//! * [`sandbox`] — how external binaries run (bubblewrap on Linux: read-only
//!   system, job dir as the only writable path, no network unless declared).
//! * [`lock`] — `tools.lock.toml`, the pinned versions the updater manages.

pub mod builtin;
pub mod external;
pub mod install;
pub mod lock;
pub mod mcp;
pub mod sandbox;

use async_trait::async_trait;
use jugg_core::Risk;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub input_schema: Value,
    pub risk: Risk,
    /// Minutes-long work (video renders, big batches) runs as a background job.
    #[serde(default)]
    pub long_running: bool,
}

/// Where a tool call runs: one private directory per job.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub job_dir: PathBuf,
    pub brand: Arc<jugg_brand::BrandStandards>,
    /// Set once the owner approved this specific call (Publish / Destructive).
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Short text the model reads.
    pub summary: String,
    /// Files produced, relative to the job dir. Become draft assets.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool {0}")]
    Unknown(String),
    #[error("invalid arguments: {0}")]
    Args(String),
    #[error("needs owner approval")]
    NeedsApproval,
    #[error("tool not installed: {0}")]
    NotInstalled(String),
    #[error("tool failed: {0}")]
    Failed(String),
    #[error("timed out after {0}s")]
    Timeout(u64),
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// One sentence the owner reads before approving a Publish/Destructive call.
    fn approval_summary(&self, _args: &Value) -> String {
        format!("Run {}", self.spec().name.replace('_', " "))
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}

/// Where long-running calls go instead of blocking the conversation.
#[async_trait]
pub trait JobSink: Send + Sync {
    /// Queue `tool(args)` for the job worker; returns a job id.
    async fn enqueue(&self, tool: &str, args: Value, ctx: &ToolContext) -> Result<String, ToolError>;
}

/// Where Publish / Destructive calls go to wait for the owner.
#[async_trait]
pub trait ApprovalSink: Send + Sync {
    /// Record a pending approval; the UI shows Approve / Decline in the chat.
    async fn request(&self, tool: &str, args: Value, summary: String, ctx: &ToolContext) -> Result<String, ToolError>;
}

#[derive(Default, Clone)]
pub struct Registry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    sink: Option<Arc<dyn JobSink>>,
    approvals: Option<Arc<dyn ApprovalSink>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: impl Tool + 'static) {
        let spec = tool.spec();
        self.tools.insert(spec.name, Arc::new(tool));
    }

    /// Native tools plus every external tool listed in the lock file.
    pub fn standard(lock: &lock::ToolsLock, tools_dir: &std::path::Path) -> Self {
        let mut r = Self::new();
        builtin::register_all(&mut r);
        external::register_all(&mut r, lock, tools_dir);
        r
    }

    /// Send long-running tools to the job queue.
    pub fn with_job_sink(mut self, sink: Arc<dyn JobSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn with_approvals(mut self, sink: Arc<dyn ApprovalSink>) -> Self {
        self.approvals = Some(sink);
        self
    }

    /// The same registry without sinks (job worker, approved actions).
    pub fn direct(&self) -> Self {
        Self { tools: self.tools.clone(), sink: None, approvals: None }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Run a tool, enforcing the approval policy in one place for every caller.
    pub async fn call(&self, name: &str, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self.get(name).ok_or_else(|| ToolError::Unknown(name.into()))?;
        if tool.spec().risk.needs_approval() && !ctx.approved {
            let Some(approvals) = &self.approvals else { return Err(ToolError::NeedsApproval) };
            let summary = tool.approval_summary(&args);
            let id = approvals.request(name, args, summary.clone(), ctx).await?;
            return Ok(ToolOutput {
                summary: format!("Waiting for the owner: “{summary}”. An Approve button is in the chat. Tell them what will happen and stop; don't retry."),
                files: vec![],
                data: serde_json::json!({ "approval_id": id }),
            });
        }
        std::fs::create_dir_all(&ctx.job_dir).map_err(|e| ToolError::Failed(e.to_string()))?;
        if let (true, Some(sink)) = (tool.spec().long_running, &self.sink) {
            let id = sink.enqueue(name, args, ctx).await?;
            return Ok(ToolOutput {
                summary: format!("Started in the background (job {}). The result will appear in Assets when it's done; tell the owner that.", &id[id.len().saturating_sub(8)..]),
                files: vec![],
                data: serde_json::json!({ "job_id": id }),
            });
        }
        tool.run(args, ctx).await
    }
}

/// Sidecar `<file>.meta.json` telling the asset library what a file is.
/// `attachments` are companion files (e.g. a card's back preview) that belong to
/// this asset and must not be listed as assets of their own.
pub fn write_meta(file: &std::path::Path, kind: jugg_core::AssetKind, title: &str, preview: Option<&str>) -> Result<(), ToolError> {
    write_meta_with(file, kind, title, preview, &[])
}

pub fn write_meta_with(file: &std::path::Path, kind: jugg_core::AssetKind, title: &str, preview: Option<&str>, attachments: &[&str]) -> Result<(), ToolError> {
    let meta = serde_json::json!({ "kind": kind, "title": title, "preview": preview, "attachments": attachments });
    let mut name = file.as_os_str().to_owned();
    name.push(".meta.json");
    std::fs::write(std::path::PathBuf::from(name), meta.to_string()).map_err(|e| ToolError::Failed(e.to_string()))
}

/// Resolve a model-supplied relative path inside the job dir; rejects escapes.
pub fn job_path(ctx: &ToolContext, rel: &str) -> Result<PathBuf, ToolError> {
    let p = std::path::Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(ToolError::Args(format!("path must stay inside the job directory: {rel}")));
    }
    Ok(ctx.job_dir.join(p))
}
