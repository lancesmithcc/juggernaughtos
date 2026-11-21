//! Tools implemented in-process (no subprocess, no sandbox needed).

use crate::{job_path, Registry, Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use async_trait::async_trait;
use jugg_core::Risk;
use serde_json::{json, Value};

pub fn register_all(r: &mut Registry) {
    r.register(Vectorize);
    r.register(BrandGuide);
    r.register(LintCss);
}

/// Logo → SVG using the vendored Vectorverter 5000 engine (linked, not shelled out).
pub struct Vectorize;

#[async_trait]
impl Tool for Vectorize {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "vectorize".into(),
            description: "Trace a raster image (PNG/JPEG/WebP) into a clean, editable SVG. Use for logos once the owner has picked one. Optionally restrict to brand palette colours.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["input"],
                "properties": {
                    "input": {"type": "string", "description": "image path inside the job dir"},
                    "output": {"type": "string", "description": "svg path inside the job dir"},
                    "preset": {"type": "string", "enum": ["auto", "logo", "illustration", "line-art", "detailed"]},
                    "brand_palette": {"type": "boolean", "description": "trace with exactly the brand colours"}
                }
            }),
            risk: Risk::Create,
        }
    }

    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = job_path(ctx, args["input"].as_str().ok_or_else(|| ToolError::Args("input is required".into()))?)?;
        let out_rel = args["output"].as_str().map(str::to_string).unwrap_or_else(|| {
            std::path::Path::new(args["input"].as_str().unwrap_or("logo")).with_extension("svg").to_string_lossy().to_string()
        });
        let output = job_path(ctx, &out_rel)?;
        let preset = args["preset"].as_str().unwrap_or("logo").to_string();
        let palette: Option<Vec<String>> = args["brand_palette"].as_bool().unwrap_or(false).then(|| ctx.brand.colors.iter().map(|c| c.hex.clone()).collect());

        let report = tokio::task::spawn_blocking(move || -> Result<String, ToolError> {
            let preset: vectorverter_core::Preset = serde_json::from_value(json!(preset)).map_err(|_| ToolError::Args("unknown preset".into()))?;
            let mut settings = preset.settings();
            settings.palette = palette;
            settings.validate().map_err(ToolError::Args)?;
            let out = vectorverter_core::trace_file(&input, &settings).map_err(|e| ToolError::Failed(e.to_string()))?;
            std::fs::write(&output, out.svg.as_bytes()).map_err(|e| ToolError::Failed(e.to_string()))?;
            Ok(format!("{} colours, {} paths", out.stats.colors, out.stats.paths))
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))??;

        Ok(ToolOutput { summary: format!("Vectorized → {out_rel} ({report})"), files: vec![out_rel.into()], data: Value::Null })
    }
}

/// Returns the active brand standards so the model can ground its work.
pub struct BrandGuide;

#[async_trait]
impl Tool for BrandGuide {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "brand_guide".into(),
            description: "Read the business's brand standards (colours, fonts, logo rules, imagery rules, voice). Call before creating any asset or copy.".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            risk: Risk::Read,
        }
    }
    async fn run(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput { summary: ctx.brand.to_design_md(), files: vec![], data: serde_json::to_value(&*ctx.brand).unwrap_or_default() })
    }
}

/// Token gate for CSS the model writes.
pub struct LintCss;

#[async_trait]
impl Tool for LintCss {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "lint_css".into(),
            description: "Check CSS against the brand tokens. Off-palette colours and off-brand fonts are errors.".into(),
            input_schema: json!({"type": "object", "required": ["css"], "properties": {"css": {"type": "string"}}}),
            risk: Risk::Read,
        }
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let css = args["css"].as_str().ok_or_else(|| ToolError::Args("css is required".into()))?;
        let findings = jugg_brand::lint_css(&ctx.brand, css);
        let summary = if findings.is_empty() { "CSS passes the brand gate.".to_string() } else { format!("{} finding(s)", findings.len()) };
        Ok(ToolOutput { summary, files: vec![], data: serde_json::to_value(findings).unwrap_or_default() })
    }
}
