//! Minimal MCP (Model Context Protocol) server over stdio, exposing the
//! [`Registry`]. Claude Code (`claude -p --mcp-config …`) and Codex CLI
//! (`codex exec -c mcp_servers.jugg…`) launch `juggernaught mcp` and see the
//! same tools, schemas and approval rules as the API providers.
//!
//! Implements `initialize`, `tools/list`, `tools/call`, `ping`
//! (JSON-RPC 2.0, newline-delimited, protocol 2025-06-18).

use crate::{Registry, ToolContext, ToolError};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub async fn handle(registry: &Registry, ctx: &ToolContext, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req["method"].as_str().unwrap_or_default();
    // Notifications (no id) get no response.
    let id = id?;
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "juggernaught", "version": env!("CARGO_PKG_VERSION")}
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": registry.specs().into_iter().map(|s| json!({
                "name": s.name, "description": s.description, "inputSchema": s.input_schema,
                "annotations": {"destructiveHint": s.risk == jugg_core::Risk::Destructive,
                                "readOnlyHint": s.risk == jugg_core::Risk::Read}
            })).collect::<Vec<_>>()
        })),
        "tools/call" => {
            let name = req["params"]["name"].as_str().unwrap_or_default();
            let args = req["params"].get("arguments").cloned().unwrap_or(json!({}));
            match registry.call(name, args, ctx).await {
                Ok(out) => Ok(json!({
                    "content": [{"type": "text", "text": out.summary}],
                    "structuredContent": {"files": out.files, "data": out.data}
                })),
                // Tool errors are results the model can react to, not protocol errors.
                Err(ToolError::NeedsApproval) => Ok(json!({
                    "content": [{"type": "text", "text": "This action needs the owner's approval. An approval request was shown in the chat; wait for it."}],
                    "isError": true
                })),
                Err(e) => Ok(json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true})),
            }
        }
        _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

/// Serve on stdin/stdout until EOF.
pub async fn serve_stdio(registry: Registry, ctx: ToolContext) -> std::io::Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&registry, &ctx, &req).await,
            Err(e) => Some(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":e.to_string()}})),
        };
        if let Some(r) = resp {
            out.write_all(r.to_string().as_bytes()).await?;
            out.write_all(b"\n").await?;
            out.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext { job_dir: dir.into(), brand: Arc::new(jugg_brand::BrandStandards::default()), approved: false }
    }

    #[tokio::test]
    async fn lists_and_calls_tools() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::standard(&crate::lock::ToolsLock::embedded(), dir.path());
        let list = handle(&reg, &ctx(dir.path()), &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await.unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"vectorize") && names.contains(&"speak") && names.contains(&"brand_guide"), "{names:?}");

        let call = handle(&reg, &ctx(dir.path()), &json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"lint_css","arguments":{"css":"a{color:#ff00ff}"}}})).await.unwrap();
        assert_eq!(call["result"]["structuredContent"]["data"][0]["rule"], "brand.color.off_palette");

        // External tool not installed in the temp tools dir → clean error result.
        let missing = handle(&reg, &ctx(dir.path()), &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"speak","arguments":{"text":"hi","output":"a.wav"}}})).await.unwrap();
        assert_eq!(missing["result"]["isError"], true);
    }

    #[tokio::test]
    async fn vectorize_traces_a_logo() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = image::RgbaImage::from_pixel(64, 64, image::Rgba([250, 248, 244, 255]));
        for y in 16..48 {
            for x in 16..48 {
                img.put_pixel(x, y, image::Rgba([194, 80, 42, 255]));
            }
        }
        img.save(dir.path().join("logo.png")).unwrap();
        let reg = Registry::standard(&crate::lock::ToolsLock::default(), dir.path());
        let out = reg.call("vectorize", json!({"input":"logo.png","brand_palette":true}), &ctx(dir.path())).await.unwrap();
        let svg = std::fs::read_to_string(dir.path().join("logo.svg")).unwrap();
        assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"), "{}", &svg[..60.min(svg.len())]);
        assert!(out.summary.contains("Vectorized"));
        assert!(reg.call("vectorize", json!({"input":"../etc/passwd"}), &ctx(dir.path())).await.is_err(), "path escape must fail");
    }
}
