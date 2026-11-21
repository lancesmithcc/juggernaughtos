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
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}

#[derive(Default, Clone)]
pub struct Registry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
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
            return Err(ToolError::NeedsApproval);
        }
        std::fs::create_dir_all(&ctx.job_dir).map_err(|e| ToolError::Failed(e.to_string()))?;
        tool.run(args, ctx).await
    }
}

/// Resolve a model-supplied relative path inside the job dir; rejects escapes.
pub fn job_path(ctx: &ToolContext, rel: &str) -> Result<PathBuf, ToolError> {
    let p = std::path::Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(ToolError::Args(format!("path must stay inside the job directory: {rel}")));
    }
    Ok(ctx.job_dir.join(p))
}
