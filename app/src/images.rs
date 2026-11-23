//! Images. Default: a hand-off to the owner's own ChatGPT. The agent writes the prompt
//! (brand palette and imagery rules included); the owner opens it in ChatGPT with one
//! click, makes the image there, and drops it back into the chat, where it lands in
//! Assets like any other output. No browser automation: ChatGPT's bot checks stop
//! automated browsers, and we don't disguise one.
//!
//! Optional: with an OpenAI API key saved, the owner can have the same prompt made
//! automatically through the Images API instead (paced, see `API_LIMITS`).

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
async fn recent_starts(env: &SiteEnv, now: i64) -> Result<Vec<i64>, String> {
    sqlx::query_scalar("SELECT started_at FROM image_generations WHERE backend = ? AND started_at > ?")
        .bind(API)
        .bind(now - 86_400)
        .fetch_all(&env.pool)
        .await
        .map_err(|e| format!("image pacing unavailable: {e}"))
}

async fn cooldown(env: &SiteEnv) -> Result<Option<(i64, String)>, String> {
    sqlx::query_as("SELECT until_at, reason FROM image_cooldowns WHERE backend = ?").bind(API).fetch_optional(&env.pool).await.map_err(|e| format!("image pacing unavailable: {e}"))
}

/// Wait for a slot, or explain when the next one opens.
async fn slot(env: &SiteEnv) -> Result<(), String> {
    loop {
        let now = jugg_core::now_unix();
        let starts = recent_starts(env, now).await?;
        let cd = cooldown(env).await?;
        match admit(now, &starts, cd.as_ref().map(|(u, r)| (*u, r.as_str())), &API_LIMITS) {
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
    slot(env).await?;
    let id: i64 = sqlx::query_scalar("INSERT INTO image_generations (backend, started_at) VALUES (?, ?) RETURNING id")
        .bind(API)
        .bind(jugg_core::now_unix())
        .fetch_one(&env.pool)
        .await
        .map_err(|e| format!("image pacing unavailable: {e}"))?;
    let r = run_api(&key, &model, prompt, shape).await.and_then(|b| to_png(&b));
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
    let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM image_handoffs WHERE conversation_id = ? AND status = 'waiting'")
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

type HandoffRow = (String, String, String, String, String, String, Option<String>);

async fn view(st: &AppState, row: HandoffRow) -> crate::api::HandoffView {
    let (id, prompt, shape, title, status, _conversation, asset_id) = row;
    let asset = match &asset_id {
        Some(a) => crate::assets::card(st, a).await,
        None => None,
    };
    crate::api::HandoffView { open_url: chatgpt_url(&prompt), id, prompt, shape, title, status, asset, api_key_set: crate::studio::image_key(&st.site).await.is_some() }
}

/// Requests made in this conversation since `since` that still wait for the owner.
pub async fn handoffs_since(st: &AppState, conversation: &str, since: i64) -> Vec<crate::api::HandoffView> {
    let rows: Vec<HandoffRow> = sqlx::query_as(
        "SELECT id, prompt, shape, title, status, conversation_id, asset_id FROM image_handoffs WHERE conversation_id = ? AND created_at >= ? AND status = 'waiting' ORDER BY created_at",
    )
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

/// Check, clean (re-encode as PNG, which drops metadata) and file the owner's image
/// under the request, through the same brand gate as everything else.
pub async fn attach(st: &AppState, user: &str, id: &str, bytes: &[u8]) -> Result<crate::api::HandoffView, String> {
    let row: Option<(String, String, String, String)> =
        sqlx::query_as("SELECT conversation_id, kind, title, status FROM image_handoffs WHERE id = ?").bind(id).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?;
    let (conversation, kind, title, status) = row.ok_or("This image request no longer exists.")?;
    if status != "waiting" {
        return Err("An image was already added for this request.".into());
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err("That file is too big (25 MB at most).".into());
    }
    let png = clean_image(bytes)?;
    let job = st.cfg.data_dir.join("jobs").join(&conversation);
    std::fs::create_dir_all(&job).map_err(|e| e.to_string())?;
    let name = format!("chatgpt-{}.png", &jugg_crypto::sha256_hex(&png)[..10]);
    let path = job.join(&name);
    std::fs::write(&path, &png).map_err(|e| e.to_string())?;
    let kind: jugg_core::AssetKind = serde_json::from_value(json!(kind)).unwrap_or(jugg_core::AssetKind::Image);
    jugg_tools::write_meta(&path, kind, &title, None).map_err(|e| e.to_string())?;
    let sha = jugg_crypto::sha256_hex(&png);
    crate::assets::sweep(st, &conversation, &job).await;
    let asset: String = sqlx::query_scalar("SELECT id FROM assets WHERE sha256 = ?").bind(&sha).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?.ok_or("That exact image is already in Assets.")?;
    sqlx::query("UPDATE image_handoffs SET status = 'done', asset_id = ?, done_at = ? WHERE id = ? AND status = 'waiting'")
        .bind(&asset)
        .bind(jugg_core::now_unix())
        .bind(id)
        .execute(&st.pool)
        .await
        .map_err(|e| e.to_string())?;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), user, "images.handoff_added", &json!({ "handoff": id, "asset": asset })).await;
    let row: HandoffRow = sqlx::query_as("SELECT id, prompt, shape, title, status, conversation_id, asset_id FROM image_handoffs WHERE id = ?").bind(id).fetch_one(&st.pool).await.map_err(|e| e.to_string())?;
    Ok(view(st, row).await)
}

/// Make a waiting request through the API key instead of ChatGPT.
pub async fn make_with_api(st: &AppState, user: &str, id: &str) -> Result<crate::api::HandoffView, String> {
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
