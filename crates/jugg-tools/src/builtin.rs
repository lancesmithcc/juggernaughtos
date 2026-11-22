//! Tools implemented in-process (no subprocess, no sandbox needed).

use crate::{job_path, Registry, Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use async_trait::async_trait;
use jugg_core::Risk;
use serde_json::{json, Value};

pub fn register_all(r: &mut Registry) {
    r.register(Vectorize);
    r.register(BrandGuide);
    r.register(LintCss);
    r.register(BusinessCard);
    r.register(EmailSignature);
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
            long_running: false,
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

        crate::write_meta(&job_path(ctx, &out_rel)?, jugg_core::AssetKind::LogoVector, "Vectorized logo", None)?;
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
            long_running: false,
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
            long_running: false,
        }
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let css = args["css"].as_str().ok_or_else(|| ToolError::Args("css is required".into()))?;
        let findings = jugg_brand::lint_css(&ctx.brand, css);
        let summary = if findings.is_empty() { "CSS passes the brand gate.".to_string() } else { format!("{} finding(s)", findings.len()) };
        Ok(ToolOutput { summary, files: vec![], data: serde_json::to_value(findings).unwrap_or_default() })
    }
}

fn slug(s: &str) -> String {
    let s: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-").chars().take(40).collect()
}

/// Print-ready business card (front + back PDF with bleed) from the brand.
pub struct BusinessCard;

#[async_trait]
impl Tool for BusinessCard {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "business_card".into(),
            description: "Design a print-ready business card (3.5×2 in, 0.125 in bleed, front + back PDF) in the brand's colours and fonts. Returns the PDF and preview images.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {"type": "string", "description": "person's name"},
                    "title": {"type": "string"},
                    "phone": {"type": "string"},
                    "email": {"type": "string"},
                    "website": {"type": "string"},
                    "address": {"type": "string"},
                    "logo_svg": {"type": "string", "description": "optional SVG logo path inside the job dir (e.g. from vectorize)"}
                }
            }),
            risk: Risk::Create,
            long_running: false,
        }
    }

    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let mut input: jugg_print::CardInput = serde_json::from_value(args.clone()).map_err(|e| ToolError::Args(e.to_string()))?;
        if input.name.trim().is_empty() || input.name.len() > 80 {
            return Err(ToolError::Args("name is required (max 80 characters)".into()));
        }
        if let Some(p) = args["logo_svg"].as_str() {
            input.logo_svg = Some(std::fs::read_to_string(job_path(ctx, p)?).map_err(|e| ToolError::Args(format!("logo: {e}")))?);
        }
        let brand = ctx.brand.clone();
        let card = tokio::task::spawn_blocking(move || jugg_print::business_card(&brand, &input)).await.map_err(|e| ToolError::Failed(e.to_string()))?.map_err(|e| ToolError::Failed(e.to_string()))?;
        let base = format!("business-card-{}", slug(args["name"].as_str().unwrap_or("card")));
        let (pdf, front, back) = (format!("{base}.pdf"), format!("{base}-front.png"), format!("{base}-back.png"));
        let w = |rel: &str, bytes: &[u8]| -> Result<(), ToolError> { std::fs::write(job_path(ctx, rel)?, bytes).map_err(|e| ToolError::Failed(e.to_string())) };
        w(&pdf, &card.pdf)?;
        w(&front, &card.front_png)?;
        w(&back, &card.back_png)?;
        let title = format!("Business card · {}", args["name"].as_str().unwrap_or_default());
        crate::write_meta_with(&job_path(ctx, &pdf)?, jugg_core::AssetKind::BusinessCard, &title, Some(&front), &[&back])?;
        Ok(ToolOutput {
            summary: format!("Business card ready: {pdf} (3.75×2.25 in with 0.125 in bleed; front and back). Preview images: {front}, {back}."),
            files: vec![pdf.into()],
            data: json!({ "trim_in": [3.5, 2.0], "bleed_in": 0.125, "pages": 2 }),
        })
    }
}

/// Email signature (HTML with inline styles, as email clients require).
pub struct EmailSignature;

#[async_trait]
impl Tool for EmailSignature {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "email_signature".into(),
            description: "Create an on-brand email signature (HTML that works in Gmail, Outlook and Apple Mail). logo_url must be an https URL on the business's site.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {"type": "string"}, "title": {"type": "string"}, "phone": {"type": "string"},
                    "email": {"type": "string"}, "website": {"type": "string"}, "logo_url": {"type": "string"}
                }
            }),
            risk: Risk::Create,
            long_running: false,
        }
    }

    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input: jugg_print::SignatureInput = serde_json::from_value(args.clone()).map_err(|e| ToolError::Args(e.to_string()))?;
        if input.name.trim().is_empty() {
            return Err(ToolError::Args("name is required".into()));
        }
        let html = jugg_print::email_signature(&ctx.brand, &input);
        let file = format!("email-signature-{}.html", slug(&input.name));
        std::fs::write(job_path(ctx, &file)?, &html).map_err(|e| ToolError::Failed(e.to_string()))?;
        crate::write_meta(&job_path(ctx, &file)?, jugg_core::AssetKind::EmailSignature, &format!("Email signature · {}", input.name), None)?;
        Ok(ToolOutput { summary: format!("Email signature ready: {file}. To install it, open the file, select all, copy, and paste into the email app's signature settings."), files: vec![file.into()], data: Value::Null })
    }
}
