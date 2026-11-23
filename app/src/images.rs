//! Images, made with GPT Image in the owner's own ChatGPT plan.
//!
//! Automatic (default when available): the owner signs the official Codex CLI in
//! with their ChatGPT account once (`juggernaught connect codex-images`), and each
//! image request runs `codex exec` headless with its built-in image tool. The image
//! comes back into the chat and Assets with no steps for the owner.
//!
//! Hand-off (fallback): without Codex, or if it fails, the owner opens ChatGPT with
//! the brand-aware prompt filled in, makes the image there, and drops it back into
//! the chat. No browser automation: ChatGPT's bot checks stop automated browsers,
//! and we don't disguise one.
//!
//! Optional: with an OpenAI API key saved, the same prompt can be made through the
//! Images API instead (billed to the key, paced, see `API_LIMITS`).

use crate::server::AppState;
use crate::site::SiteEnv;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jugg_db::sqlx;
use serde_json::{json, Value};

pub const API: &str = "openai_api";
pub const CODEX: &str = "codex";

/// Pacing for the API fallback, so a runaway agent can't run up the owner's bill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub min_gap_secs: i64,
    pub per_hour: usize,
    pub per_day: usize,
    /// How long a request may wait in line for its slot before giving up.
    pub max_wait_secs: i64,
}

pub const API_LIMITS: Limits = Limits { min_gap_secs: 5, per_hour: 60, per_day: 300, max_wait_secs: 60 };
/// Codex runs one image at a time anyway; this only stops a runaway agent from
/// spending the owner's ChatGPT image allowance.
pub const CODEX_LIMITS: Limits = Limits { min_gap_secs: 2, per_hour: 30, per_day: 120, max_wait_secs: 600 };

/// Largest image accepted from the owner (bytes and pixels).
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
const MAX_PIXELS: u64 = 50_000_000;

#[derive(Debug, PartialEq)]
pub enum Admit {
    Go,
    Wait(i64),
    /// Not now; the earliest time a slot opens, and why.
    Refuse { retry_at: i64, reason: String },
}

/// Decide whether a generation may start now. `starts` = start times in the last 24h.
pub fn admit(now: i64, starts: &[i64], cooldown: Option<(i64, &str)>, l: &Limits) -> Admit {
    if let Some((until, why)) = cooldown {
        if until > now {
            return Admit::Refuse { retry_at: until, reason: why.to_string() };
        }
    }
    let day: Vec<i64> = starts.iter().copied().filter(|t| *t > now - 86_400).collect();
    if day.len() >= l.per_day {
        let oldest = day.iter().min().copied().unwrap_or(now);
        return Admit::Refuse { retry_at: oldest + 86_400, reason: format!("daily cap of {} images reached", l.per_day) };
    }
    let mut hour: Vec<i64> = day.iter().copied().filter(|t| *t > now - 3_600).collect();
    hour.sort();
    if hour.len() >= l.per_hour {
        return Admit::Refuse { retry_at: hour[hour.len() - l.per_hour] + 3_600, reason: format!("hourly cap of {} images reached", l.per_hour) };
    }
    match day.iter().max() {
        Some(last) if now - last < l.min_gap_secs => {
            let wait = l.min_gap_secs - (now - last);
            if wait <= l.max_wait_secs {
                Admit::Wait(wait)
            } else {
                Admit::Refuse { retry_at: last + l.min_gap_secs, reason: "spacing images out".into() }
            }
        }
        _ => Admit::Go,
    }
}

// Pacing fails closed: if the history can't be read, no image is made.
async fn recent_starts(env: &SiteEnv, backend: &str, now: i64) -> Result<Vec<i64>, String> {
    sqlx::query_scalar("SELECT started_at FROM image_generations WHERE backend = ? AND started_at > ?")
        .bind(backend)
        .bind(now - 86_400)
        .fetch_all(&env.pool)
        .await
        .map_err(|e| format!("image pacing unavailable: {e}"))
}

async fn cooldown(env: &SiteEnv, backend: &str) -> Result<Option<(i64, String)>, String> {
    sqlx::query_as("SELECT until_at, reason FROM image_cooldowns WHERE backend = ?").bind(backend).fetch_optional(&env.pool).await.map_err(|e| format!("image pacing unavailable: {e}"))
}

/// Wait for a slot, or explain when the next one opens.
async fn slot(env: &SiteEnv, backend: &str, limits: &Limits) -> Result<(), String> {
    loop {
        let now = jugg_core::now_unix();
        let starts = recent_starts(env, backend, now).await?;
        let cd = cooldown(env, backend).await?;
        match admit(now, &starts, cd.as_ref().map(|(u, r)| (*u, r.as_str())), limits) {
            Admit::Go => return Ok(()),
            Admit::Wait(s) => tokio::time::sleep(std::time::Duration::from_secs(s.max(1) as u64)).await,
            Admit::Refuse { retry_at, reason } => {
                let mins = ((retry_at - now).max(60) + 59) / 60;
                return Err(format!("{reason}; next image possible in about {mins} min"));
            }
        }
    }
}

fn shape_hint(shape: &str) -> &'static str {
    match shape {
        "landscape" => " Landscape 3:2 aspect ratio.",
        "portrait" => " Portrait 2:3 aspect ratio.",
        _ => " Square 1:1 aspect ratio.",
    }
}

/// Make one image through the OpenAI Images API with the owner's saved key.
pub async fn generate_api(env: &SiteEnv, prompt: &str, shape: &str) -> Result<Vec<u8>, String> {
    let (key, model) = crate::studio::image_key(env).await.ok_or("no OpenAI API key is saved (Settings → Image generation)")?;
    paced(env, API, &API_LIMITS, run_api(&key, &model, prompt, shape)).await
}

/// Wait for a slot, record the attempt, run it, record the outcome.
async fn paced(env: &SiteEnv, backend: &str, limits: &Limits, make: impl std::future::Future<Output = Result<Vec<u8>, String>>) -> Result<Vec<u8>, String> {
    slot(env, backend, limits).await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO image_generations (backend, started_at) VALUES (?, ?) RETURNING id")
        .bind(backend)
        .bind(jugg_core::now_unix())
        .fetch_one(&env.pool)
        .await
        .map_err(|e| format!("image pacing unavailable: {e}"))?;
    let r = make.await.and_then(|b| to_png(&b));
    let note: String = r.as_ref().err().map(|e| e.chars().take(300).collect()).unwrap_or_default();
    let _ = sqlx::query("UPDATE image_generations SET finished_at = ?, outcome = ?, note = ? WHERE id = ?")
        .bind(jugg_core::now_unix())
        .bind(if r.is_ok() { "ok" } else { "error" })
        .bind(note)
        .bind(id)
        .execute(&env.pool)
        .await;
    r
}

fn to_png(raw: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(raw).map_err(|e| format!("not an image: {e}"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

async fn run_api(key: &str, model: &str, prompt: &str, shape: &str) -> Result<Vec<u8>, String> {
    let size = match shape {
        "landscape" => "1536x1024",
        "portrait" => "1024x1536",
        _ => "1024x1024",
    };
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(180)).build().map_err(|e| e.to_string())?;
    let resp: Value = http
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(key)
        .json(&json!({ "model": model, "prompt": prompt, "size": size, "n": 1 }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(err) = resp["error"]["message"].as_str() {
        return Err(format!("image model said: {err}"));
    }
    use base64::Engine;
    let b64 = resp["data"][0]["b64_json"].as_str().ok_or("no image returned")?;
    base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ Codex (ChatGPT plan)

/// Where the Codex CLI keeps its login and generated images on this box.
fn codex_home(cfg: &jugg_core::Config) -> std::path::PathBuf {
    if cfg.dev {
        if let Some(h) = std::env::var_os("CODEX_HOME") {
            return h.into();
        }
        return std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default().join(".codex");
    }
    crate::server::agent_home(cfg).join(".codex")
}

/// `codex …` as the agent user: inside the sandbox on Linux servers, with the
/// agent's own HOME elsewhere in production, and as-is in development.
fn codex_command(cfg: &jugg_core::Config, cwd: &std::path::Path) -> tokio::process::Command {
    let sandbox = crate::server::cli_sandbox(cfg, cwd);
    let mut c = jugg_ai::cli::cli_command("codex", sandbox.as_ref(), cwd);
    if sandbox.is_none() && !cfg.dev {
        c.env("HOME", crate::server::agent_home(cfg));
    }
    c
}

/// Should `codex login` use a device code (shown here, entered on chatgpt.com from
/// any phone or laptop) instead of opening a browser on this machine? Yes on servers,
/// over SSH, and on Linux without a desktop.
pub fn headless_login(dev: bool, has_var: impl Fn(&str) -> bool) -> bool {
    !dev || has_var("SSH_CONNECTION") || has_var("SSH_TTY") || (cfg!(target_os = "linux") && !has_var("DISPLAY") && !has_var("WAYLAND_DISPLAY"))
}

/// Arguments for the Codex sign-in.
pub fn codex_login_args(device_code: bool) -> Vec<&'static str> {
    if device_code {
        vec!["login", "--device-auth"]
    } else {
        vec!["login"]
    }
}

static CODEX_READY: std::sync::Mutex<Option<(i64, bool)>> = std::sync::Mutex::new(None);
static CODEX_ONE_AT_A_TIME: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// Codex is installed and signed in with a ChatGPT account (not an API key, which
/// would bill differently). Cached briefly: it shells out.
pub async fn codex_ready(cfg: &jugg_core::Config) -> bool {
    let now = jugg_core::now_unix();
    if let Some((at, ok)) = *CODEX_READY.lock().unwrap_or_else(|e| e.into_inner()) {
        if now - at < if ok { 300 } else { 30 } {
            return ok;
        }
    }
    let dir = cfg.data_dir.join("jobs");
    let _ = std::fs::create_dir_all(&dir);
    let mut c = codex_command(cfg, &dir);
    c.args(["login", "status"]).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let ok = match tokio::time::timeout(std::time::Duration::from_secs(20), c.output()).await {
        Ok(Ok(o)) => o.status.success() && {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            text.contains("Logged in") && text.contains("ChatGPT")
        },
        _ => false,
    };
    *CODEX_READY.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, ok));
    ok
}

/// What `codex exec --json` told us about one run.
#[derive(Debug, Default, PartialEq)]
struct CodexRun {
    thread: Option<String>,
    last_message: Option<String>,
    error: Option<String>,
}

fn read_codex_events(stdout: &str) -> CodexRun {
    let mut run = CodexRun::default();
    for v in stdout.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
        match (v["type"].as_str(), v["item"]["type"].as_str()) {
            (Some("thread.started"), _) => run.thread = v["thread_id"].as_str().map(str::to_string),
            (Some("item.completed"), Some("agent_message")) => run.last_message = v["item"]["text"].as_str().map(str::to_string),
            (Some("turn.failed"), _) => run.error = v["error"]["message"].as_str().map(str::to_string),
            (Some("error"), _) => run.error = v["message"].as_str().map(str::to_string),
            _ => {}
        }
    }
    run
}

/// Thread ids become a directory name: accept only what Codex actually issues.
fn safe_thread_id(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Make one image with GPT Image through the owner's signed-in Codex CLI.
async fn generate_codex(st: &AppState, prompt: &str, conversation: &str) -> Result<Vec<u8>, String> {
    let _turn = CODEX_ONE_AT_A_TIME.acquire().await.map_err(|e| e.to_string())?;
    let job = st.cfg.data_dir.join("jobs").join(conversation);
    std::fs::create_dir_all(&job).map_err(|e| e.to_string())?;
    let home = codex_home(&st.cfg);
    let run = async {
        let mut c = codex_command(&st.cfg, &job);
        c.args(["exec", "--json", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only", "--enable", "image_generation", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = c.spawn().map_err(|e| format!("couldn't start Codex: {e}"))?;
        let ask = format!(
            "Create exactly one image with your built-in image generation tool, then reply with the single word: done. \
             Don't run commands, read files, open skills or ask questions.\n\nImage prompt:\n{}",
            prompt.trim()
        );
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(ask.as_bytes()).await.map_err(|e| e.to_string())?;
        }
        let out = tokio::time::timeout(std::time::Duration::from_secs(360), child.wait_with_output())
            .await
            .map_err(|_| "Codex took longer than 6 minutes".to_string())?
            .map_err(|e| e.to_string())?;
        let events = read_codex_events(&String::from_utf8_lossy(&out.stdout));
        let thread = events.thread.clone().filter(|t| safe_thread_id(t)).ok_or_else(|| events.error.clone().unwrap_or_else(|| "Codex didn't start (is it still signed in?)".into()))?;
        let dir = home.join("generated_images").join(&thread);
        let newest = std::fs::read_dir(&dir).ok().and_then(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| ["png", "jpg", "jpeg", "webp"].contains(&x.to_string_lossy().to_ascii_lowercase().as_str())))
                .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
                .map(|e| e.path())
        });
        let bytes = newest.and_then(|p| std::fs::read(p).ok());
        // The image now lives in Assets; don't leave a second copy in Codex's folder.
        let _ = std::fs::remove_dir_all(&dir);
        bytes.ok_or_else(|| {
            let why = events.error.or(events.last_message).unwrap_or_default();
            let why: String = why.chars().take(200).collect();
            if why.is_empty() { "Codex finished without an image (your ChatGPT plan's image limit may be used up for now)".to_string() } else { format!("Codex finished without an image: {why}") }
        })
    };
    paced(&st.site, CODEX, &CODEX_LIMITS, run).await
}

/// Start making a waiting request with Codex in the background. The chat polls
/// `status`; the image lands in Assets even if the owner closes the tab.
pub async fn start_codex(st: &AppState, id: &str) -> Result<(), String> {
    let row: Option<(String, String)> = sqlx::query_as(
        "UPDATE image_handoffs SET status = 'making', note = NULL WHERE id = ? AND status = 'waiting' RETURNING prompt, conversation_id",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .map_err(|e| e.to_string())?;
    let Some((prompt, conversation)) = row else { return Err("This image is already being made or was added.".into()) };
    let owner: String = sqlx::query_scalar("SELECT user_id FROM conversations WHERE id = ?").bind(&conversation).fetch_optional(&st.pool).await.ok().flatten().unwrap_or_else(|| "agent".into());
    let (st, id) = (st.clone(), id.to_string());
    tokio::spawn(async move {
        let made = generate_codex(&st, &prompt, &conversation).await;
        let failed = match made {
            Ok(png) => attach(&st, &owner, &id, &png).await.err(),
            Err(e) => Some(e),
        };
        if let Some(e) = failed {
            tracing::warn!(handoff = %id, error = %e, "codex image failed");
            let _ = sqlx::query("UPDATE image_handoffs SET status = 'waiting', note = ? WHERE id = ? AND status = 'making'")
                .bind(e.chars().take(300).collect::<String>())
                .bind(&id)
                .execute(&st.pool)
                .await;
        }
    });
    Ok(())
}

// ------------------------------------------------------------------ hand-off to ChatGPT

/// The text the owner sends in ChatGPT.
pub fn chatgpt_prompt(full_prompt: &str, shape: &str) -> String {
    format!("Create an image. {}{}", full_prompt.trim(), shape_hint(shape))
}

/// Opens ChatGPT with the prompt filled in.
pub fn chatgpt_url(text: &str) -> String {
    format!("https://chatgpt.com/?q={}", url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>())
}

/// Record a request for the owner. Returns its id.
pub async fn create_handoff(env: &SiteEnv, conversation: &str, prompt: &str, kind: &str, shape: &str, title: &str) -> Result<String, String> {
    let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM image_handoffs WHERE conversation_id = ? AND status IN ('waiting','making')")
        .bind(conversation)
        .fetch_one(&env.pool)
        .await
        .map_err(|e| e.to_string())?;
    if waiting >= 6 {
        return Err("six images are already waiting for the owner in this chat; wait until they add some".into());
    }
    let id = jugg_core::new_id();
    sqlx::query("INSERT INTO image_handoffs (id, conversation_id, prompt, kind, shape, title, created_at) VALUES (?,?,?,?,?,?,?)")
        .bind(&id)
        .bind(conversation)
        .bind(chatgpt_prompt(prompt, shape))
        .bind(kind)
        .bind(shape)
        .bind(title)
        .bind(jugg_core::now_unix())
        .execute(&env.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(id)
}

type HandoffRow = (String, String, String, String, String, String, Option<String>, Option<String>);
macro_rules! handoff_row {
    ($tail:literal) => {
        concat!("SELECT id, prompt, shape, title, status, conversation_id, asset_id, note FROM image_handoffs ", $tail)
    };
}

async fn view(st: &AppState, row: HandoffRow) -> crate::api::HandoffView {
    let (id, prompt, shape, title, status, _conversation, asset_id, note) = row;
    let asset = match &asset_id {
        Some(a) => crate::assets::card(st, a).await,
        None => None,
    };
    let file = match &asset_id {
        Some(a) => sqlx::query_scalar::<_, String>("SELECT sha256 FROM assets WHERE id = ?").bind(a).fetch_optional(&st.pool).await.ok().flatten().map(|sha| handoff_file_name(&sha)),
        None => None,
    };
    crate::api::HandoffView {
        open_url: chatgpt_url(&prompt),
        id,
        prompt,
        shape,
        title,
        status,
        asset,
        note,
        api_key_set: crate::studio::image_key(&st.site).await.is_some(),
        codex_ready: codex_ready(&st.cfg).await,
        file,
    }
}

/// Requests made in this conversation since `since` that aren't done yet. New ones
/// start in Codex right away when it's signed in, so the owner has nothing to do.
pub async fn handoffs_since(st: &AppState, conversation: &str, since: i64) -> Vec<crate::api::HandoffView> {
    let fresh: Vec<String> = sqlx::query_scalar("SELECT id FROM image_handoffs WHERE conversation_id = ? AND created_at >= ? AND status = 'waiting' AND note IS NULL ORDER BY created_at")
        .bind(conversation)
        .bind(since)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    if !fresh.is_empty() && codex_ready(&st.cfg).await {
        for id in &fresh {
            if let Err(e) = start_codex(st, id).await {
                tracing::warn!(handoff = %id, error = %e, "couldn't start codex image");
            }
        }
    }
    let rows: Vec<HandoffRow> = sqlx::query_as(handoff_row!("WHERE conversation_id = ? AND created_at >= ? AND status IN ('waiting','making') ORDER BY created_at"))
        .bind(conversation)
        .bind(since)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let mut out = Vec::new();
    for r in rows {
        out.push(view(st, r).await);
    }
    out
}

/// One request as the owner may see it (only their own conversations).
pub async fn status(st: &AppState, user: &str, id: &str) -> Result<crate::api::HandoffView, String> {
    let row: Option<HandoffRow> = sqlx::query_as(handoff_row!("WHERE id = ? AND conversation_id IN (SELECT id FROM conversations WHERE user_id = ?)"))
        .bind(id)
        .bind(user)
        .fetch_optional(&st.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(view(st, row.ok_or("This image request no longer exists.")?).await)
}

/// Owner asked to try Codex again after a failure.
pub async fn retry_codex(st: &AppState, user: &str, id: &str) -> Result<crate::api::HandoffView, String> {
    status(st, user, id).await?;
    if !codex_ready(&st.cfg).await {
        return Err("Codex isn't signed in on this box. Run: juggernaught connect codex-images".into());
    }
    start_codex(st, id).await?;
    status(st, user, id).await
}

fn handoff_file_name(sha: &str) -> String {
    format!("chatgpt-{}.png", &sha[..sha.len().min(10)])
}

/// Check, clean (re-encode as PNG, which drops metadata) and file the owner's image
/// under the request, through the same brand gate as everything else.
pub async fn attach(st: &AppState, user: &str, id: &str, bytes: &[u8]) -> Result<crate::api::HandoffView, String> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT conversation_id, kind, title, status FROM image_handoffs WHERE id = ?").bind(id).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?;
    let (conversation, kind, title, status) = row.ok_or("This image request no longer exists.")?;
    if status != "waiting" && status != "making" {
        return Err("An image was already added for this request.".into());
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err("That file is too big (25 MB at most).".into());
    }
    let png = clean_image(bytes)?;
    let job = st.cfg.data_dir.join("jobs").join(&conversation);
    std::fs::create_dir_all(&job).map_err(|e| e.to_string())?;
    let name = handoff_file_name(&jugg_crypto::sha256_hex(&png));
    let path = job.join(&name);
    std::fs::write(&path, &png).map_err(|e| e.to_string())?;
    let kind: jugg_core::AssetKind = serde_json::from_value(json!(kind)).unwrap_or(jugg_core::AssetKind::Image);
    jugg_tools::write_meta(&path, kind, &title, None).map_err(|e| e.to_string())?;
    let sha = jugg_crypto::sha256_hex(&png);
    crate::assets::sweep(st, &conversation, &job).await;
    let asset: String = sqlx::query_scalar("SELECT id FROM assets WHERE sha256 = ?").bind(&sha).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?.ok_or("That exact image is already in Assets.")?;
    sqlx::query("UPDATE image_handoffs SET status = 'done', asset_id = ?, done_at = ?, note = NULL WHERE id = ? AND status IN ('waiting','making')")
        .bind(&asset)
        .bind(jugg_core::now_unix())
        .bind(id)
        .execute(&st.pool)
        .await
        .map_err(|e| e.to_string())?;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), user, "images.handoff_added", &json!({ "handoff": id, "asset": asset })).await;
    let row: HandoffRow = sqlx::query_as(handoff_row!("WHERE id = ?")).bind(id).fetch_one(&st.pool).await.map_err(|e| e.to_string())?;
    Ok(view(st, row).await)
}

/// Make a waiting request through the API key instead of ChatGPT.
pub async fn make_with_api(st: &AppState, user: &str, id: &str) -> Result<crate::api::HandoffView, String> {
    status(st, user, id).await?;
    let row: Option<(String, String, String)> = sqlx::query_as("SELECT prompt, shape, status FROM image_handoffs WHERE id = ?").bind(id).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?;
    let (prompt, shape, status) = row.ok_or("This image request no longer exists.")?;
    if status != "waiting" {
        return Err("An image was already added for this request.".into());
    }
    let png = generate_api(&st.site, &prompt, &shape).await?;
    attach(st, user, id, &png).await
}

fn clean_image(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let (w, h) = reader.into_dimensions().map_err(|_| "That isn't an image we can read (PNG, JPEG, WebP or GIF).".to_string())?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err("That image is too large (50 megapixels at most).".into());
    }
    to_png(bytes).map_err(|_| "That isn't an image we can read (PNG, JPEG, WebP or GIF).".into())
}

/// POST /files/handoff/{id}: the owner's image for a waiting request (raw body).
pub async fn upload(State(st): State<AppState>, Path(id): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(user) = crate::server::session_user(&st, &headers).await else { return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "Sign in first." }))).into_response() };
    if let Err(e) = status(&st, &user, &id).await {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": e }))).into_response();
    }
    match attach(&st, &user, &id, &body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const L: Limits = Limits { min_gap_secs: 90, per_hour: 10, per_day: 40, max_wait_secs: 240 };

    #[test]
    fn device_code_sign_in_on_servers_and_over_ssh() {
        let none = |_: &str| false;
        assert!(headless_login(false, none), "production boxes never have a browser");
        assert!(headless_login(true, |v| v == "SSH_CONNECTION"), "ssh into a dev box");
        if !cfg!(target_os = "linux") {
            assert!(!headless_login(true, none), "local desktop keeps the browser sign-in");
        }
        assert_eq!(codex_login_args(true), ["login", "--device-auth"]);
        assert_eq!(codex_login_args(false), ["login"]);
    }

    #[test]
    fn reads_codex_run() {
        let out = r#"{"type":"thread.started","thread_id":"01a2069b-9949-7403-b517-e9dc07cfde4f"}
{"type":"item.completed","item":{"type":"error","message":"instructions too long"}}
{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
not json
{"type":"turn.completed","usage":{"input_tokens":1}}"#;
        let r = read_codex_events(out);
        assert_eq!(r.thread.as_deref(), Some("01a2069b-9949-7403-b517-e9dc07cfde4f"));
        assert_eq!(r.last_message.as_deref(), Some("done"));
        assert_eq!(r.error, None, "item-level warnings aren't failures");
        let failed = read_codex_events(r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#);
        assert_eq!(failed.error.as_deref(), Some("usage limit reached"));
    }

    #[test]
    fn thread_ids_cannot_escape_the_images_folder() {
        assert!(safe_thread_id("01a2069b-9949-7403-b517-e9dc07cfde4f"));
        assert!(!safe_thread_id("../../etc"));
        assert!(!safe_thread_id(""));
        assert!(!safe_thread_id("a/b"));
    }

    #[test]
    fn paces_one_image_per_gap() {
        assert_eq!(admit(10_000, &[], None, &L), Admit::Go);
        assert_eq!(admit(10_000, &[9_950], None, &L), Admit::Wait(40));
        assert_eq!(admit(10_000, &[9_900], None, &L), Admit::Go);
    }

    #[test]
    fn caps_per_hour_and_day() {
        let now = 1_000_000;
        let hour: Vec<i64> = (0..L.per_hour as i64).map(|i| now - 3_000 + i * 100).collect();
        match admit(now + 200, &hour, None, &L) {
            Admit::Refuse { retry_at, reason } => {
                assert!(reason.contains("hourly"));
                assert_eq!(retry_at, hour[0] + 3_600);
            }
            other => panic!("{other:?}"),
        }
        let day: Vec<i64> = (0..L.per_day as i64).map(|i| now - 80_000 + i * 1_000).collect();
        assert!(matches!(admit(now, &day, None, &L), Admit::Refuse { ref reason, .. } if reason.contains("daily")));
    }

    #[test]
    fn cooldown_wins_until_it_expires() {
        assert!(matches!(admit(100, &[], Some((500, "limit")), &L), Admit::Refuse { retry_at: 500, .. }));
        assert_eq!(admit(600, &[], Some((500, "limit")), &L), Admit::Go);
    }

    #[test]
    fn chatgpt_link_carries_the_prompt() {
        let t = chatgpt_prompt("A climber at dusk & warm light", "landscape");
        assert!(t.starts_with("Create an image. A climber"));
        let u = chatgpt_url(&t);
        assert!(u.starts_with("https://chatgpt.com/?q=Create+an+image."));
        assert!(u.contains("%26") && !u.contains(' '));
    }

    #[test]
    fn rejects_non_images() {
        assert!(clean_image(b"not an image").is_err());
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(4, 4).write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(clean_image(png.get_ref()).is_ok());
    }
}
