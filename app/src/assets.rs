//! Asset library: everything tools produce lands here.
//!
//! After each turn (and each background job) the conversation's job directory
//! is swept: new files are copied into a content-addressed store
//! (`assets/<sha256>.<ext>`), typed from their sidecar metadata, run through the
//! brand gate, and recorded as `review` (passed) or `rejected` (with the report).
//! The owner approves to make an asset `final`.

use crate::api::AssetCard;
use crate::server::AppState;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use jugg_brand::gate::{check_file, Strictness};
use jugg_core::AssetKind;
use jugg_db::sqlx;
use jugg_tools::{JobSink, ToolContext, ToolError};
use serde_json::{json, Value};
use std::path::{Path as FsPath, PathBuf};
use tower::ServiceExt;

pub fn mime_for(ext: &str) -> &'static str {
    match ext {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "eps" => "application/postscript",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn infer_kind(ext: &str) -> AssetKind {
    match ext {
        "svg" => AssetKind::LogoVector,
        "png" | "jpg" | "jpeg" | "webp" => AssetKind::Image,
        "pdf" => AssetKind::Pdf,
        "mp4" | "webm" => AssetKind::Video,
        "wav" | "mp3" => AssetKind::Audio,
        "html" => AssetKind::WebPage,
        _ => AssetKind::Image,
    }
}

fn strictness(kind: AssetKind) -> Strictness {
    match kind {
        AssetKind::Logo | AssetKind::LogoVector | AssetKind::IconSet | AssetKind::BusinessCard | AssetKind::EmailSignature | AssetKind::Poster => Strictness::Exact,
        _ => Strictness::Loose,
    }
}

fn kind_str(k: AssetKind) -> String {
    serde_json::to_value(k).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "image".into())
}

/// Register new files from `job_dir`. Returns cards for the new assets.
pub async fn sweep(st: &AppState, conversation: &str, job_dir: &FsPath) -> Vec<AssetCard> {
    let Ok(rd) = std::fs::read_dir(job_dir) else { return vec![] };
    let files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // Previews named by a sidecar are attached to their asset, not listed separately.
    let mut previews = std::collections::HashSet::new();
    let mut metas = std::collections::HashMap::new();
    for f in files.iter().filter(|f| name(f).ends_with(".meta.json")) {
        if let Ok(v) = std::fs::read(f).map(|b| serde_json::from_slice::<Value>(&b).unwrap_or_default()) {
            if let Some(p) = v["preview"].as_str() {
                previews.insert(p.to_string());
            }
            for a in v["attachments"].as_array().into_iter().flatten().filter_map(|a| a.as_str()) {
                previews.insert(a.to_string());
            }
            metas.insert(name(f).trim_end_matches(".meta.json").to_string(), v);
        }
    }
    let brand = st.brand.read().await.clone();
    let store = st.cfg.assets_dir();
    let _ = std::fs::create_dir_all(&store);
    let mut out = Vec::new();
    for f in &files {
        let n = name(f);
        if n.starts_with('.') || n.ends_with(".meta.json") || previews.contains(&n) {
            continue;
        }
        let Ok(bytes) = std::fs::read(f) else { continue };
        let sha = jugg_crypto::sha256_hex(&bytes);
        let known: Option<String> = sqlx::query_scalar("SELECT id FROM assets WHERE sha256 = ?").bind(&sha).fetch_optional(&st.pool).await.ok().flatten();
        if known.is_some() {
            continue;
        }
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_ascii_lowercase();
        let meta = metas.get(&n).cloned().unwrap_or_default();
        let kind: AssetKind = serde_json::from_value(meta["kind"].clone()).unwrap_or_else(|_| infer_kind(&ext));
        let title = meta["title"].as_str().map(str::to_string).unwrap_or_else(|| n.clone());
        let stored = format!("{sha}.{ext}");
        if std::fs::write(store.join(&stored), &bytes).is_err() {
            continue;
        }
        let preview = match meta["preview"].as_str() {
            Some(p) => std::fs::read(job_dir.join(p)).ok().and_then(|b| {
                let pname = format!("{sha}.preview.png");
                std::fs::write(store.join(&pname), b).ok().map(|_| pname)
            }),
            None if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "svg") => Some(stored.clone()),
            None => None,
        };
        let report = check_file(&brand, &store.join(&stored), strictness(kind));
        let status = if report.passed { "review" } else { "rejected" };
        let id = jugg_core::new_id();
        let res = sqlx::query(
            "INSERT INTO assets (id, kind, title, status, storage_path, mime, bytes, sha256, encrypted, brand_version, gate_report_json, preview_path, conversation_id, created_at)
             VALUES (?,?,?,?,?,?,?,?,0,1,?,?,?,?)",
        )
        .bind(&id)
        .bind(kind_str(kind))
        .bind(&title)
        .bind(status)
        .bind(&stored)
        .bind(mime_for(&ext))
        .bind(bytes.len() as i64)
        .bind(&sha)
        .bind(serde_json::to_string(&report).unwrap_or_default())
        .bind(&preview)
        .bind(conversation)
        .bind(jugg_core::now_unix())
        .execute(&st.pool)
        .await;
        if res.is_ok() {
            let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), "agent", "asset.created", &json!({ "id": id, "kind": kind_str(kind), "status": status })).await;
            if let Some(card) = card(st, &id).await {
                out.push(card);
            }
        }
    }
    out
}

pub async fn card(st: &AppState, id: &str) -> Option<AssetCard> {
    let row: (String, String, String, String, String, Option<String>, Option<String>, i64) =
        sqlx::query_as("SELECT id, kind, title, status, mime, gate_report_json, preview_path, created_at FROM assets WHERE id = ?").bind(id).fetch_optional(&st.pool).await.ok()??;
    Some(to_card(row))
}

pub fn to_card((id, kind, title, status, mime, report, preview, created_at): (String, String, String, String, String, Option<String>, Option<String>, i64)) -> AssetCard {
    let report: Value = report.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default();
    AssetCard {
        url: format!("/files/asset/{id}"),
        preview_url: preview.map(|_| format!("/files/asset/{id}/preview")),
        summary: report["summary"].as_str().unwrap_or_default().to_string(),
        findings: report["findings"].as_array().map(|a| a.iter().filter_map(|f| f["message"].as_str().map(str::to_string)).collect()).unwrap_or_default(),
        id,
        kind,
        title,
        status,
        mime,
        created_at,
    }
}

pub async fn list(st: &AppState, kind: Option<String>) -> Vec<AssetCard> {
    let rows: Vec<(String, String, String, String, String, Option<String>, Option<String>, i64)> = match kind.filter(|k| !k.is_empty()) {
        Some(k) => sqlx::query_as("SELECT id, kind, title, status, mime, gate_report_json, preview_path, created_at FROM assets WHERE kind = ? ORDER BY created_at DESC LIMIT 200").bind(k).fetch_all(&st.pool).await,
        None => sqlx::query_as("SELECT id, kind, title, status, mime, gate_report_json, preview_path, created_at FROM assets ORDER BY created_at DESC LIMIT 200").fetch_all(&st.pool).await,
    }
    .unwrap_or_default();
    rows.into_iter().map(to_card).collect()
}

/// Approve (→ final) or reject an asset. Approving a gate-rejected asset is an
/// owner override: allowed, warned in the UI, and recorded.
pub async fn decide(st: &AppState, user: &str, id: &str, approve: bool, reason: Option<String>) -> Result<AssetCard, String> {
    let status: String = sqlx::query_scalar("SELECT status FROM assets WHERE id = ?").bind(id).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?.ok_or("Asset not found.")?;
    let new = if approve { "final" } else { "rejected" };
    if approve && status == "rejected" {
        sqlx::query("INSERT INTO standards_overrides (id, rule, target, reason, user_id, created_at) VALUES (?,?,?,?,?,?)")
            .bind(jugg_core::new_id())
            .bind("brand.gate")
            .bind(id)
            .bind(reason.unwrap_or_else(|| "owner approved despite brand gate".into()))
            .bind(user)
            .bind(jugg_core::now_unix())
            .execute(&st.pool)
            .await
            .map_err(|e| e.to_string())?;
    }
    sqlx::query("UPDATE assets SET status = ?, decided_at = ? WHERE id = ?").bind(new).bind(jugg_core::now_unix()).bind(id).execute(&st.pool).await.map_err(|e| e.to_string())?;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), user, if approve { "asset.approved" } else { "asset.rejected" }, &json!({ "id": id, "was": status })).await;
    card(st, id).await.ok_or_else(|| "Asset not found.".into())
}

/// GET /files/asset/{id}[/preview] — owner only, served inert (sandboxed CSP).
pub async fn serve(State(st): State<AppState>, Path(params): Path<Vec<String>>, req: Request) -> Response {
    if crate::server::session_user(&st, req.headers()).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let id = params.first().cloned().unwrap_or_default();
    let preview = params.get(1).map(|s| s == "preview").unwrap_or(false);
    let row: Option<(String, Option<String>, String, String)> =
        sqlx::query_as("SELECT storage_path, preview_path, mime, title FROM assets WHERE id = ?").bind(&id).fetch_optional(&st.pool).await.ok().flatten();
    let Some((stored, preview_path, mime, title)) = row else { return StatusCode::NOT_FOUND.into_response() };
    let (file, mime) = if preview {
        match preview_path {
            Some(p) => {
                let m = if p.ends_with(".png") { "image/png".to_string() } else { mime };
                (p, m)
            }
            None => return StatusCode::NOT_FOUND.into_response(),
        }
    } else {
        (stored, mime)
    };
    // Paths come from our own DB, but never trust them blindly.
    if file.contains('/') || file.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = st.cfg.assets_dir().join(&file);
    let mut res = match tower_http::services::ServeFile::new(&path).oneshot(req).await {
        Ok(r) => r.map(Body::new),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; media-src 'self'; sandbox"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    let ext = file.rsplit('.').next().unwrap_or("bin");
    let safe: String = title.chars().map(|c| if c.is_ascii_alphanumeric() || c == ' ' || c == '-' { c } else { '-' }).collect();
    if let Ok(v) = HeaderValue::from_str(&format!("inline; filename=\"{}.{ext}\"", safe.trim())) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    res
}

// ------------------------------------------------------------------ background jobs

/// Long tools are queued in SQLite; works from the app and the MCP bridge alike.
pub struct DbJobSink {
    pub pool: jugg_db::sqlx::SqlitePool,
}

#[async_trait::async_trait]
impl JobSink for DbJobSink {
    async fn enqueue(&self, tool: &str, args: Value, ctx: &ToolContext) -> Result<String, ToolError> {
        let conversation = ctx.job_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        jugg_db::jobs::enqueue(&self.pool, "tool", &json!({ "tool": tool, "args": args, "conversation": conversation }), 40).await.map_err(|e| ToolError::Failed(e.to_string()))
    }
}

/// Runs queued tool jobs one at a time (renders are heavy; the box is small).
pub async fn worker(st: AppState) {
    let direct = st.registry.direct();
    loop {
        let _ = jugg_db::jobs::reap_stale(&st.pool, 15 * 60).await;
        let job = match jugg_db::jobs::claim(&st.pool, "worker-1").await {
            Ok(Some(j)) => j,
            _ => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }
        };
        let payload: Value = serde_json::from_str(&job.payload_json).unwrap_or_default();
        let conversation = payload["conversation"].as_str().unwrap_or("jobs").to_string();
        if conversation.contains('/') || conversation.contains("..") {
            let _ = jugg_db::jobs::finish(&st.pool, &job.id, Err("bad job payload".into())).await;
            continue;
        }
        let ctx = ToolContext { job_dir: st.cfg.data_dir.join("jobs").join(&conversation), brand: std::sync::Arc::new(st.brand.read().await.clone()), approved: false };
        let tool = payload["tool"].as_str().unwrap_or_default().to_string();
        tracing::info!(job = %job.id, %tool, "job started");
        let pool = st.pool.clone();
        let id = job.id.clone();
        let beat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                let _ = jugg_db::jobs::heartbeat(&pool, &id, 0.5).await;
            }
        });
        let result = direct.call(&tool, payload["args"].clone(), &ctx).await;
        beat.abort();
        let assets = sweep(&st, &conversation, &ctx.job_dir).await;
        let outcome = match result {
            Ok(out) => Ok(json!({ "summary": out.summary, "assets": assets.iter().map(|a| &a.id).collect::<Vec<_>>() })),
            Err(e) => Err(e.to_string()),
        };
        tracing::info!(job = %job.id, ok = outcome.is_ok(), "job finished");
        let _ = jugg_db::jobs::finish(&st.pool, &job.id, outcome).await;
    }
}

pub async fn jobs(st: &AppState) -> Vec<crate::api::JobView> {
    let rows: Vec<(String, String, String, f64, Option<String>, i64)> =
        sqlx::query_as("SELECT id, payload_json, status, progress, error, created_at FROM jobs WHERE status IN ('queued','running') OR (status = 'failed' AND created_at > ?) ORDER BY created_at DESC LIMIT 20")
            .bind(jugg_core::now_unix() - 86_400)
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
    rows.into_iter()
        .map(|(id, p, status, progress, error, created_at)| {
            let tool = serde_json::from_str::<Value>(&p).ok().and_then(|v| v["tool"].as_str().map(str::to_string)).unwrap_or_default();
            crate::api::JobView { id, tool, status, progress, error, created_at }
        })
        .collect()
}
