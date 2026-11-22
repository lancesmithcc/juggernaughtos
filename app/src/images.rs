//! Image generation. Default: the owner's own ChatGPT session (GPT Image) driven by
//! Playwright (`tools/chatgpt-image`). Optional fallback: the OpenAI Images API with
//! the owner's key. Generations are paced so the ChatGPT account is never hammered:
//! one at a time, a minimum gap between starts, hourly and daily caps, and a cooldown
//! whenever ChatGPT reports a limit, shows a bot check or keeps failing.

use crate::site::SiteEnv;
use jugg_db::sqlx;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const CHATGPT: &str = "chatgpt";
pub const API: &str = "openai_api";

/// Pacing for the ChatGPT backend. Conservative on purpose: a paid ChatGPT plan allows
/// far more, but bursts are what trip abuse heuristics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub min_gap_secs: i64,
    pub per_hour: usize,
    pub per_day: usize,
    /// How long a request may wait in line for its slot before giving up.
    pub max_wait_secs: i64,
}

pub const CHATGPT_LIMITS: Limits = Limits { min_gap_secs: 90, per_hour: 10, per_day: 40, max_wait_secs: 240 };
pub const API_LIMITS: Limits = Limits { min_gap_secs: 5, per_hour: 60, per_day: 300, max_wait_secs: 60 };

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

/// Seconds to pause after a ChatGPT "limit" message. Reads hints like "in 2 hours",
/// "in 45 minutes"; otherwise three hours.
pub fn limit_cooldown(retry_hint: Option<&str>) -> i64 {
    let h = retry_hint.unwrap_or_default().to_ascii_lowercase();
    let n: i64 = h.split(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).and_then(|s| s.parse().ok()).unwrap_or(0);
    let secs = if h.contains("hour") {
        n * 3_600
    } else if h.contains("min") {
        n * 60
    } else if h.contains("day") {
        n * 86_400
    } else {
        0
    };
    if secs > 0 {
        (secs + 300).min(2 * 86_400)
    } else {
        3 * 3_600
    }
}

/// One image request at a time across the box. It also guards the browser profile,
/// which only one Chromium may open; the sign-in view holds it while it runs.
static GATE: LazyLock<Arc<tokio::sync::Mutex<()>>> = LazyLock::new(|| Arc::new(tokio::sync::Mutex::new(())));

/// Last known ChatGPT sign-in state: 0 unknown, 1 signed in, 2 signed out, 3 bot check.
static SESSION: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn set_session(v: u8) {
    SESSION.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub fn session_state() -> &'static str {
    match SESSION.load(std::sync::atomic::Ordering::Relaxed) {
        1 => "signed_in",
        2 => "signed_out",
        3 => "bot_check",
        _ => "unknown",
    }
}

pub fn profile_dir(env: &SiteEnv) -> PathBuf {
    env.cfg.data_dir.join("chatgpt").join("profile")
}

/// The installed worker (`<tools_dir>/chatgpt-image/<version>/chatgpt-image`), or the
/// repository copy in development once `npm ci` has run there.
pub fn worker(env: &SiteEnv) -> Option<PathBuf> {
    let lock = std::fs::read_to_string(env.cfg.tools_dir.join("tools.lock.toml")).ok().and_then(|s| jugg_tools::lock::ToolsLock::parse(&s).ok()).unwrap_or_else(jugg_tools::lock::ToolsLock::embedded);
    if let Some(t) = lock.tool.get("chatgpt-image") {
        let p = jugg_tools::install::root(&env.cfg.tools_dir, "chatgpt-image", t).join(&t.bin);
        if p.exists() {
            return Some(p);
        }
    }
    if env.cfg.dev {
        let mut dirs: Vec<PathBuf> = env.cfg.data_dir.ancestors().map(Path::to_path_buf).collect();
        dirs.extend(std::env::current_dir().ok());
        for d in dirs {
            let p = d.join("tools/chatgpt-image");
            if p.join("node_modules/playwright-core").exists() && p.join("browsers").exists() {
                return Some(p.join("chatgpt-image"));
            }
        }
    }
    None
}

fn worker_command(env: &SiteEnv, bin: &Path) -> tokio::process::Command {
    let mut c = tokio::process::Command::new(bin);
    let home = env.cfg.data_dir.join("chatgpt");
    let _ = std::fs::create_dir_all(&home);
    c.env("HOME", &home).kill_on_drop(true).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    if std::env::var_os("PATH").is_none() {
        c.env("PATH", "/usr/local/bin:/usr/bin:/bin");
    }
    c
}

// Pacing fails closed: if the history can't be read, no image is made.
async fn recent_starts(env: &SiteEnv, backend: &str, now: i64) -> Result<Vec<i64>, String> {
    sqlx::query_scalar("SELECT started_at FROM image_generations WHERE backend = ? AND started_at > ? AND outcome NOT IN ('not_signed_in')")
        .bind(backend)
        .bind(now - 86_400)
        .fetch_all(&env.pool)
        .await
        .map_err(|e| format!("image pacing unavailable: {e}"))
}

async fn cooldown(env: &SiteEnv, backend: &str) -> Result<Option<(i64, String)>, String> {
    sqlx::query_as("SELECT until_at, reason FROM image_cooldowns WHERE backend = ?").bind(backend).fetch_optional(&env.pool).await.map_err(|e| format!("image pacing unavailable: {e}"))
}

async fn set_cooldown(env: &SiteEnv, backend: &str, secs: i64, reason: &str) {
    let until = jugg_core::now_unix() + secs;
    let _ = sqlx::query("INSERT INTO image_cooldowns (backend, until_at, reason) VALUES (?,?,?) ON CONFLICT(backend) DO UPDATE SET until_at = excluded.until_at, reason = excluded.reason")
        .bind(backend)
        .bind(until)
        .bind(reason)
        .execute(&env.pool)
        .await;
    let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), "system", "images.paused", &json!({ "backend": backend, "until": until, "reason": reason })).await;
}

async fn begin(env: &SiteEnv, backend: &str) -> Result<i64, String> {
    sqlx::query_scalar("INSERT INTO image_generations (backend, started_at) VALUES (?, ?) RETURNING id")
        .bind(backend)
        .bind(jugg_core::now_unix())
        .fetch_one(&env.pool)
        .await
        .map_err(|e| format!("image pacing unavailable: {e}"))
}

async fn finish(env: &SiteEnv, id: i64, outcome: &str, note: &str) {
    let _ = sqlx::query("UPDATE image_generations SET finished_at = ?, outcome = ?, note = ? WHERE id = ?").bind(jugg_core::now_unix()).bind(outcome).bind(note.chars().take(300).collect::<String>()).bind(id).execute(&env.pool).await;
}

/// Wait for a slot under `limits`, or explain when the next one opens.
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

pub struct Generated {
    pub bytes: Vec<u8>,
    pub backend: &'static str,
    pub note: String,
}

fn shape_hint(shape: &str) -> &'static str {
    match shape {
        "landscape" => " Landscape 3:2 aspect ratio.",
        "portrait" => " Portrait 2:3 aspect ratio.",
        _ => " Square 1:1 aspect ratio.",
    }
}

/// Generate one image. ChatGPT first; the API key (when set) only when ChatGPT can't.
pub async fn generate(env: &SiteEnv, prompt: &str, shape: &str) -> Result<Generated, String> {
    let _one_at_a_time = GATE.lock().await;
    let why_not_chatgpt: String;
    if session_state() == "signed_out" && worker(env).is_some() {
        // Known not connected: answer at once so the owner gets the Connect window right away.
        why_not_chatgpt = "ChatGPT isn't connected yet; the owner now sees a Connect ChatGPT window".into();
    } else if let Some(bin) = worker(env) {
        match slot(env, CHATGPT, &CHATGPT_LIMITS).await {
            Ok(()) => match run_chatgpt(env, &bin, &format!("{prompt}{}", shape_hint(shape))).await {
                Ok(bytes) => return Ok(Generated { bytes, backend: CHATGPT, note: "made with your ChatGPT account".into() }),
                Err(e) => why_not_chatgpt = e,
            },
            Err(e) if e.contains("human check") => why_not_chatgpt = format!("{e}. Sign in to ChatGPT (a window opens for the owner) to clear it"),
            Err(e) => why_not_chatgpt = format!("ChatGPT image pacing: {e}"),
        }
    } else {
        why_not_chatgpt = "the ChatGPT image maker isn't set up on this server yet; the owner now sees a Connect ChatGPT window that sets it up".into();
    }
    if let Some((key, model)) = crate::studio::image_key(env).await {
        slot(env, API, &API_LIMITS).await.map_err(|e| format!("{why_not_chatgpt}. API fallback: {e}"))?;
        let id = begin(env, API).await?;
        let r = run_api(&key, &model, prompt, shape).await;
        finish(env, id, if r.is_ok() { "ok" } else { "error" }, r.as_ref().err().map(String::as_str).unwrap_or("")).await;
        return r.map(|bytes| Generated { bytes, backend: API, note: format!("made with the OpenAI API key because {why_not_chatgpt}") });
    }
    Err(why_not_chatgpt)
}

async fn run_chatgpt(env: &SiteEnv, bin: &Path, prompt: &str) -> Result<Vec<u8>, String> {
    let work = env.cfg.data_dir.join("chatgpt").join("work");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let tag = jugg_crypto::sha256_hex(&jugg_crypto::random_bytes::<16>())[..12].to_string();
    let (prompt_file, out_file) = (work.join(format!("{tag}.txt")), work.join(format!("{tag}.img")));
    std::fs::write(&prompt_file, prompt).map_err(|e| e.to_string())?;
    let id = begin(env, CHATGPT).await?;
    let mut cmd = worker_command(env, bin);
    cmd.arg("generate").arg("--profile").arg(profile_dir(env)).arg("--prompt-file").arg(&prompt_file).arg("--out").arg(&out_file).args(["--timeout", "480"]);
    cmd.stdout(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(560), cmd.output()).await;
    let _ = std::fs::remove_file(&prompt_file);
    let result: Value = match output {
        Ok(Ok(o)) => String::from_utf8_lossy(&o.stdout).lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok().filter(|v| v["done"] == true)).unwrap_or(json!({"ok": false, "reason": "error", "message": "worker gave no result"})),
        Ok(Err(e)) => json!({"ok": false, "reason": "error", "message": e.to_string()}),
        Err(_) => json!({"ok": false, "reason": "timeout", "message": "worker timed out"}),
    };
    let reason = result["reason"].as_str().unwrap_or("error").to_string();
    let message = result["message"].as_str().unwrap_or_default().to_string();
    if result["ok"] == true {
        let raw = std::fs::read(&out_file).map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&out_file);
        let png = raw.and_then(|r| to_png(&r));
        finish(env, id, if png.is_ok() { "ok" } else { "error" }, result["capture"].as_str().unwrap_or("")).await;
        set_session(1);
        let _ = sqlx::query("DELETE FROM image_cooldowns WHERE backend = ? AND reason LIKE 'repeated errors%'").bind(CHATGPT).execute(&env.pool).await;
        return png;
    }
    let _ = std::fs::remove_file(&out_file);
    finish(env, id, &reason, &message).await;
    match reason.as_str() {
        "limit" => {
            let secs = limit_cooldown(result["retry"].as_str());
            set_cooldown(env, CHATGPT, secs, "ChatGPT said the image limit was reached").await;
            Err(format!("ChatGPT's image limit was reached; pausing ChatGPT images for about {} min", secs / 60))
        }
        "blocked" => {
            set_session(3);
            set_cooldown(env, CHATGPT, 30 * 60, "ChatGPT asked for a human check").await;
            Err("ChatGPT wants a quick human check. Sign in to ChatGPT (a window opens for the owner) and complete it".into())
        }
        "not_signed_in" => {
            set_session(2);
            Err("not connected to ChatGPT yet. Sign in to ChatGPT (a window opens for the owner)".into())
        }
        "refused" => Err(format!("ChatGPT didn't make the image: {message}")),
        _ => {
            let errors: i64 = sqlx::query_scalar("SELECT count(*) FROM (SELECT outcome FROM image_generations WHERE backend = ? ORDER BY id DESC LIMIT 2) WHERE outcome IN ('error','timeout')")
                .bind(CHATGPT)
                .fetch_one(&env.pool)
                .await
                .unwrap_or(0);
            if errors >= 2 {
                set_cooldown(env, CHATGPT, 10 * 60, "repeated errors talking to ChatGPT").await;
            }
            Err(format!("ChatGPT image failed ({reason}): {message}"))
        }
    }
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

// ------------------------------------------------------------------ one-click install

/// "" idle, "running", "ok", or "failed: …"
static INSTALL: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

pub fn install_state() -> String {
    INSTALL.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Install the ChatGPT image maker from the admin. Works where the app may write its
/// tools folder (development, single-user boxes); hardened servers keep that folder
/// root-only, so the owner gets the one command to run instead.
pub fn start_install(env: &SiteEnv) {
    {
        let mut st = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
        if *st == "running" {
            return;
        }
        *st = "running".into();
    }
    let tools_dir = env.cfg.tools_dir.clone();
    tokio::task::spawn_blocking(move || {
        let lock = std::fs::read_to_string(tools_dir.join("tools.lock.toml")).ok().and_then(|s| jugg_tools::lock::ToolsLock::parse(&s).ok()).unwrap_or_else(jugg_tools::lock::ToolsLock::embedded);
        let r = match lock.tool.get("chatgpt-image") {
            Some(t) => std::fs::create_dir_all(&tools_dir).map_err(|e| e.to_string()).and_then(|_| jugg_tools::install::install(&tools_dir, "chatgpt-image", t)),
            None => Err("this release has no chatgpt-image entry".into()),
        };
        *INSTALL.lock().unwrap_or_else(|e| e.into_inner()) = match r {
            Ok(_) => "ok".into(),
            Err(e) => format!("failed: {}", e.chars().take(300).collect::<String>()),
        };
    });
}

// ------------------------------------------------------------------ status for Settings

pub struct Usage {
    pub installed: bool,
    pub session: &'static str,
    pub used_hour: usize,
    pub used_day: usize,
    pub paused_until: Option<i64>,
    pub paused_reason: Option<String>,
}

pub async fn usage(env: &SiteEnv) -> Usage {
    let now = jugg_core::now_unix();
    let starts = recent_starts(env, CHATGPT, now).await.unwrap_or_default();
    let cd = cooldown(env, CHATGPT).await.ok().flatten().filter(|(u, _)| *u > now);
    Usage {
        installed: worker(env).is_some(),
        session: session_state(),
        used_hour: starts.iter().filter(|t| **t > now - 3_600).count(),
        used_day: starts.len(),
        paused_until: cd.as_ref().map(|c| c.0),
        paused_reason: cd.map(|c| c.1),
    }
}

/// Ask the worker whether the profile is signed in (opens the browser briefly).
pub async fn check_session(env: &SiteEnv) -> Result<&'static str, String> {
    let bin = worker(env).ok_or("the ChatGPT image tool is not installed")?;
    let _g = GATE.try_lock().map_err(|_| "busy making an image or signing in; try again in a minute".to_string())?;
    let mut cmd = worker_command(env, &bin);
    cmd.arg("status").arg("--profile").arg(profile_dir(env)).stdout(std::process::Stdio::piped());
    let o = tokio::time::timeout(std::time::Duration::from_secs(90), cmd.output()).await.map_err(|_| "timed out".to_string())?.map_err(|e| e.to_string())?;
    let v: Value = String::from_utf8_lossy(&o.stdout).lines().rev().find_map(|l| serde_json::from_str(l).ok()).unwrap_or(Value::Null);
    set_session(if v["challenge"] == true {
        3
    } else if v["signed_in"] == true {
        1
    } else {
        2
    });
    Ok(session_state())
}

// ------------------------------------------------------------------ remote sign-in

/// A headless browser on the server the owner drives from the admin: we stream
/// screenshots and forward their clicks and typing. Nothing typed is stored.
struct SignIn {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    frame: Arc<Mutex<Option<FrameState>>>,
    done: Arc<std::sync::atomic::AtomicBool>,
    _gate: tokio::sync::OwnedMutexGuard<()>,
}

#[derive(Clone)]
struct FrameState {
    jpeg: String,
    signed_in: bool,
    challenge: bool,
    on_auth_page: bool,
}

static SIGN_IN: LazyLock<tokio::sync::Mutex<Option<SignIn>>> = LazyLock::new(|| tokio::sync::Mutex::new(None));

pub async fn sign_in_start(env: &SiteEnv) -> Result<(), String> {
    let bin = worker(env).ok_or("the ChatGPT image tool is not installed (juggernaught tools install chatgpt-image)")?;
    let mut slot = SIGN_IN.lock().await;
    if let Some(s) = slot.as_ref() {
        if !s.done.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(());
        }
    }
    *slot = None;
    let gate = GATE.clone().try_lock_owned().map_err(|_| "an image is being made right now; try again in a minute".to_string())?;
    let mut cmd = worker_command(env, &bin);
    cmd.arg("login").arg("--profile").arg(profile_dir(env)).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let frame = Arc::new(Mutex::new(None));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (f2, d2) = (frame.clone(), done.clone());
    let (pool, signer) = (env.pool.clone(), env.signer.clone());
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(f) = v["frame"].as_str() {
                let url = v["url"].as_str().unwrap_or_default();
                *f2.lock().unwrap_or_else(|e| e.into_inner()) = Some(FrameState {
                    jpeg: f.to_string(),
                    signed_in: v["signed_in"] == true,
                    challenge: v["challenge"] == true,
                    on_auth_page: url.contains("auth.openai.com") || url.contains("/auth/") || url.contains("/log-in"),
                });
            }
            if v["signed_in"] == true && v.get("frame").is_none() {
                set_session(1);
                let _ = sqlx::query("DELETE FROM image_cooldowns WHERE backend = ? AND reason LIKE '%human check%'").bind(CHATGPT).execute(&pool).await;
                let _ = jugg_db::audit::append(&pool, Some(&signer), "owner", "images.chatgpt_signed_in", &json!({})).await;
            }
        }
        d2.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    *slot = Some(SignIn { child, stdin, frame, done, _gate: gate });
    Ok(())
}

pub struct SignInFrame {
    pub jpeg_b64: Option<String>,
    pub signed_in: bool,
    pub active: bool,
    /// What the owner should do now: starting | log_in | signing_in | human_check | done | closed
    pub step: &'static str,
}

pub async fn sign_in_frame() -> SignInFrame {
    let mut slot = SIGN_IN.lock().await;
    let Some(s) = slot.as_ref() else {
        let signed_in = session_state() == "signed_in";
        return SignInFrame { jpeg_b64: None, signed_in, active: false, step: if signed_in { "done" } else { "closed" } };
    };
    let done = s.done.load(std::sync::atomic::Ordering::Relaxed);
    let f = s.frame.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if done {
        // Release the browser profile (and the image gate) as soon as the worker exits.
        *slot = None;
    }
    let signed_in = f.as_ref().is_some_and(|x| x.signed_in) || (done && session_state() == "signed_in");
    let step = match &f {
        _ if signed_in => "done",
        None => "starting",
        Some(x) if x.challenge => "human_check",
        Some(x) if x.on_auth_page => "signing_in",
        Some(_) => "log_in",
    };
    if signed_in {
        set_session(1);
    }
    SignInFrame { signed_in, jpeg_b64: f.map(|x| x.jpeg), active: !done, step }
}

/// Forward one input event. `kind`: click | text | key | scroll | stop.
pub async fn sign_in_input(kind: &str, x: f64, y: f64, text: &str) -> Result<(), String> {
    let mut slot = SIGN_IN.lock().await;
    let s = slot.as_mut().ok_or("the sign-in window is closed")?;
    let msg = match kind {
        "click" => json!({ "click": { "x": x, "y": y } }),
        "text" => json!({ "text": text.chars().take(500).collect::<String>() }),
        "key" => json!({ "key": text }),
        "scroll" => json!({ "scroll": y }),
        "stop" => json!({ "stop": true }),
        _ => return Err("unknown input".into()),
    };
    s.stdin.write_all(format!("{msg}\n").as_bytes()).await.map_err(|e| e.to_string())?;
    s.stdin.flush().await.map_err(|e| e.to_string())?;
    if kind == "stop" {
        let mut s = slot.take().expect("checked above");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), s.child.wait()).await;
        let _ = s.child.kill().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const L: Limits = CHATGPT_LIMITS;

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
    fn reads_retry_hints() {
        assert_eq!(limit_cooldown(Some("2 hours")), 2 * 3_600 + 300);
        assert_eq!(limit_cooldown(Some("45 minutes")), 45 * 60 + 300);
        assert_eq!(limit_cooldown(Some("tomorrow")), 3 * 3_600);
        assert_eq!(limit_cooldown(None), 3 * 3_600);
    }
}
