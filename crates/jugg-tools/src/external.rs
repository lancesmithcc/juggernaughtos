//! Adapters for the external creative suite. Each is a thin, typed wrapper over
//! the tool's own CLI; anything more open-ended goes through the tool's MCP
//! server (photocraft / vectorcraft expose 500+ commands that way).

use crate::lock::{LockedTool, ToolsLock};
use crate::sandbox::{run, SandboxSpec};
use crate::{Registry, Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use async_trait::async_trait;
use jugg_core::Risk;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How a tool call maps onto CLI arguments.
type ArgBuilder = fn(&Value, &ToolContext) -> Result<(Vec<String>, Vec<PathBuf>), ToolError>;

pub struct CliTool {
    spec: ToolSpec,
    locked: LockedTool,
    root: PathBuf,
    build: ArgBuilder,
}

#[async_trait]
impl Tool for CliTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let (argv, outputs) = (self.build)(&args, ctx)?;
        let spec = SandboxSpec {
            program: self.root.join(self.locked.program()),
            args: argv,
            tool_root: self.root.clone(),
            job_dir: ctx.job_dir.clone(),
            network: self.locked.network,
            timeout: Duration::from_secs(self.locked.timeout_secs),
            env: vec![],
        };
        let r = run(&spec).await?;
        if r.status != 0 {
            return Err(ToolError::Failed(format!("{} exited {}: {}", self.spec.name, r.status, r.stderr_tail)));
        }
        Ok(ToolOutput { summary: format!("{} finished", self.spec.name), files: outputs, data: json!({ "stdout": r.stdout }) })
    }
}

fn s(v: &Value, k: &str) -> Result<String, ToolError> {
    v[k].as_str().map(str::to_string).ok_or_else(|| ToolError::Args(format!("`{k}` is required")))
}

/// Paths passed to sandboxed tools are relative to `/work` (the job dir).
fn rel(v: &Value, k: &str, ctx: &ToolContext) -> Result<String, ToolError> {
    let p = s(v, k)?;
    crate::job_path(ctx, &p)?;
    Ok(p)
}

/// Which lock entry backs a registry tool (None = built in, always available).
pub fn lock_name(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "speak" => "kokoros",
        "pdf_combine" => "pdfcraft",
        "photo_edit" => "photocraft",
        "vector_export" => "vectorcraft",
        "render_video" => "hyperframes",
        "design_audit" => "impeccable",
        _ => return None,
    })
}

pub fn register_all(r: &mut Registry, lock: &ToolsLock, tools_dir: &Path) {
    let root = |name: &str, t: &LockedTool| tools_dir.join(name).join(&t.version);
    let mut add = |name: &str, spec: ToolSpec, build: ArgBuilder| {
        // Only offer the agent tools that are actually installed on this box.
        if let Some(t) = lock.tool.get(name) {
            if !t.program().is_empty() && root(name, t).join(t.program()).exists() {
                r.register(CliTool { spec, locked: t.clone(), root: root(name, t), build });
            }
        }
    };

    add(
        "kokoros",
        ToolSpec {
            name: "speak".into(),
            description: "Text-to-speech with Kokoro (local, offline). Produces a WAV plus word timings for captions.".into(),
            input_schema: json!({"type":"object","required":["text","output"],"properties":{
                "text":{"type":"string"},"output":{"type":"string","description":"wav path in job dir"},
                "voice":{"type":"string","description":"e.g. af_heart, am_michael"}}}),
            risk: Risk::Create,
            long_running: false,
        },
        |a, ctx| {
            let out = rel(a, "output", ctx)?;
            let mut v = vec!["text".into(), s(a, "text")?, "--output".into(), out.clone(), "--timestamps".into()];
            if let Some(voice) = a["voice"].as_str() {
                v.extend(["--style".into(), voice.into()]);
            }
            Ok((v, vec![out.into()]))
        },
    );

    add(
        "pdfcraft",
        ToolSpec {
            name: "pdf_combine".into(),
            description: "Combine PDFs into one document (brochures, proposals, print packs).".into(),
            input_schema: json!({"type":"object","required":["inputs","output"],"properties":{
                "inputs":{"type":"array","items":{"type":"string"}},"output":{"type":"string"}}}),
            risk: Risk::Create,
            long_running: false,
        },
        |a, ctx| {
            let out = rel(a, "output", ctx)?;
            let mut v = vec!["combine".to_string()];
            for i in a["inputs"].as_array().ok_or_else(|| ToolError::Args("inputs must be an array".into()))? {
                let p = i.as_str().ok_or_else(|| ToolError::Args("inputs must be strings".into()))?;
                crate::job_path(ctx, p)?;
                v.push(p.into());
            }
            v.extend(["--out".into(), out.clone()]);
            Ok((v, vec![out.into()]))
        },
    );

    add(
        "photocraft",
        ToolSpec {
            name: "photo_edit".into(),
            description: "Edit a photo or PSD headlessly with PhotoCraft commands (crop, adjust, filters, layers, export). `commands` is a list of {cmd, params}.".into(),
            input_schema: json!({"type":"object","required":["input","output","commands"],"properties":{
                "input":{"type":"string"},"output":{"type":"string"},
                "commands":{"type":"array","items":{"type":"object","required":["cmd"],"properties":{"cmd":{"type":"string"},"params":{"type":"object"}}}}}}),
            risk: Risk::Create,
            long_running: false,
        },
        |a, ctx| {
            let out = rel(a, "output", ctx)?;
            let mut v = vec!["run".to_string(), rel(a, "input", ctx)?];
            for c in a["commands"].as_array().ok_or_else(|| ToolError::Args("commands must be an array".into()))? {
                v.extend(["--cmd".into(), c["cmd"].as_str().unwrap_or_default().into()]);
                if let Some(p) = c.get("params") {
                    v.extend(["--params".into(), p.to_string()]);
                }
            }
            v.extend(["--out".into(), out.clone()]);
            Ok((v, vec![out.into()]))
        },
    );

    add(
        "vectorcraft",
        ToolSpec {
            name: "vector_export".into(),
            description: "Render a VectorCraft document to PDF/SVG/PNG (business cards, posters, print layouts).".into(),
            input_schema: json!({"type":"object","required":["input","output"],"properties":{"input":{"type":"string"},"output":{"type":"string"}}}),
            risk: Risk::Create,
            long_running: false,
        },
        |a, ctx| {
            let out = rel(a, "output", ctx)?;
            Ok((vec!["run".into(), "--in".into(), rel(a, "input", ctx)?, "--export".into(), out.clone()], vec![out.into()]))
        },
    );

    add(
        "hyperframes",
        ToolSpec {
            name: "render_video".into(),
            description: "Render a HyperFrames HTML composition to MP4 (social clips, explainers, title cards). Long-running: runs as a background job.".into(),
            input_schema: json!({"type":"object","required":["project","output"],"properties":{"project":{"type":"string"},"output":{"type":"string"}}}),
            risk: Risk::Create,
            long_running: true,
        },
        |a, ctx| {
            let out = rel(a, "output", ctx)?;
            Ok((vec!["render".into(), rel(a, "project", ctx)?, "--output".into(), out.clone()], vec![out.into()]))
        },
    );

    add(
        "impeccable",
        ToolSpec {
            name: "design_audit".into(),
            description: "Audit an HTML page or asset for design quality and AI-slop patterns (impeccable). Part of the brand gate.".into(),
            input_schema: json!({"type":"object","required":["input"],"properties":{"input":{"type":"string"}}}),
            risk: Risk::Read,
            long_running: false,
        },
        // The engine's command is `detect` (impeccable engine-v0.1.x).
        |a, ctx| Ok((vec!["detect".into(), rel(a, "input", ctx)?, "--json".into()], vec![])),
    );
}
