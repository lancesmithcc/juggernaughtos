//! Server functions: the only door between the UI and the platform.
//! Every function that touches business data checks the session on the server.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Overview {
    pub business: String,
    pub has_logo: bool,
    pub setup_complete: bool,
    pub signed_in: bool,
    pub provider: String,
    pub provider_ready: bool,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolActivity {
    pub name: String,
    pub summary: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssetCard {
    pub id: String,
    pub kind: String,
    pub title: String,
    /// draft | review | rejected | final
    pub status: String,
    pub mime: String,
    pub url: String,
    pub preview_url: Option<String>,
    /// Brand gate summary + findings.
    pub summary: String,
    pub findings: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalView {
    pub id: String,
    pub summary: String,
    /// pending | approved | rejected
    pub status: String,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubmissionView {
    pub id: String,
    pub form: String,
    pub fields: Vec<(String, String)>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SiteView {
    pub public_url: String,
    pub live: bool,
    pub pages: Vec<(String, String, String)>,
    pub deploys: Vec<(String, String, i64)>,
    pub submissions: Vec<SubmissionView>,
    pub overrides: Vec<(String, String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobView {
    pub id: String,
    pub tool: String,
    pub status: String,
    pub progress: f64,
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnTicket {
    pub conversation_id: String,
    pub turn_id: String,
}

/// Mirror of `jugg_ai::AgentEvent` as it arrives over Server-Sent Events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta { text: String },
    ToolStarted { name: String },
    ToolFinished { name: String, summary: String, is_error: bool },
    ApprovalRequired { tool: String, summary: String },
    Usage {},
    Assets { items: Vec<AssetCard> },
    Approvals { items: Vec<ApprovalView> },
    Handoffs { items: Vec<HandoffView> },
    Done,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WizardInput {
    pub business: String,
    pub what_you_do: String,
    pub audience: String,
    pub traits: Vec<String>,
    pub primary: String,
    pub heading_font: String,
    pub body_font: String,
    pub owner_email: String,
    pub methods: Vec<String>,
    /// One-time setup token (production only).
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetupResult {
    pub recovery_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MethodsView {
    pub email: String,
    pub passkeys: Vec<String>,
    pub totp: bool,
    pub recovery_left: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TotpSetup {
    pub qr_png_base64: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProvidersView {
    pub active_kind: String,
    pub label: String,
    pub ready: bool,
    pub status: String,
    pub model: String,
    pub has_openrouter_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrandView {
    pub name: String,
    pub colors: Vec<(String, String, String)>,
    pub heading_font: String,
    pub body_font: String,
    pub design_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityView {
    pub posture: String,
    pub sensors: Vec<String>,
    pub tasks: Vec<(String, String, String)>,
    pub crypto: Vec<(String, String)>,
    pub sessions: Vec<(String, String, i64)>,
    pub checks: Vec<(String, String, String)>,
    pub incidents: Vec<(String, f64)>,
    pub bans: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StandardsView {
    pub rules: Vec<(String, String, bool, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolView {
    pub name: String,
    pub description: String,
    pub risk: String,
    pub available: bool,
}

// ---------------------------------------------------------------- server helpers

#[cfg(feature = "ssr")]
mod ctx {
    use crate::server::AppState;
    use leptos::prelude::*;

    pub fn state() -> Result<AppState, ServerFnError> {
        use_context::<AppState>().ok_or_else(|| ServerFnError::new("app state missing"))
    }

    pub async fn user(st: &AppState) -> Result<String, ServerFnError> {
        let headers: http::HeaderMap = leptos_axum::extract().await?;
        crate::server::session_user(st, &headers).await.ok_or_else(|| ServerFnError::new("Please sign in."))
    }

    pub async fn ip() -> String {
        leptos_axum::extract::<axum::extract::ConnectInfo<std::net::SocketAddr>>().await.map(|c| crate::server::client_ip(&c.0)).unwrap_or_else(|_| "unknown".into())
    }

    pub fn set_cookie(cookie: &str) {
        if let (Some(resp), Ok(v)) = (use_context::<leptos_axum::ResponseOptions>(), http::HeaderValue::from_str(cookie)) {
            resp.append_header(http::header::SET_COOKIE, v);
        }
    }

    pub async fn headers() -> http::HeaderMap {
        leptos_axum::extract().await.unwrap_or_default()
    }
}

// ---------------------------------------------------------------- overview + chat

#[server]
pub async fn get_overview() -> Result<Overview, ServerFnError> {
    let st = ctx::state()?;
    let setup_complete = st.owner_exists.load(std::sync::atomic::Ordering::Relaxed);
    let signed_in = ctx::user(&st).await.is_ok();
    let brand = st.brand.read().await.name.clone();
    if !signed_in {
        return Ok(Overview { business: brand, has_logo: false, setup_complete, signed_in, provider: String::new(), provider_ready: false, suggestions: vec![] });
    }
    let provider = st.provider().await;
    let provider_ready = provider.health().await.is_ok();
    let has_logo = st.brand.read().await.logo.mark_asset.is_some();
    Ok(Overview {
        business: brand,
        has_logo,
        setup_complete,
        signed_in,
        provider: provider.label(),
        provider_ready,
        suggestions: crate::server::suggestions(&st, setup_complete, provider_ready).await,
    })
}

#[server]
pub async fn start_turn(conversation_id: Option<String>, text: String) -> Result<TurnTicket, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    let text = text.trim().to_string();
    if text.is_empty() || text.len() > 20_000 {
        return Err(ServerFnError::new("Messages must be 1–20,000 characters."));
    }
    crate::server::start_turn(&st, &user, conversation_id, text).await.map_err(ServerFnError::new)
}

// ---------------------------------------------------------------- setup + sign-in

#[server]
pub async fn complete_setup(input: WizardInput) -> Result<SetupResult, ServerFnError> {
    let st = ctx::state()?;
    let (user, codes) = crate::server::complete_setup(&st, input).await.map_err(ServerFnError::new)?;
    let cookie = crate::server::new_session(&st, &user, "magic_link", &ctx::headers().await).await.map_err(ServerFnError::new)?;
    ctx::set_cookie(&cookie);
    Ok(SetupResult { recovery_codes: codes })
}

#[server]
/// Returns the link only in development, with no mail server, for a request from this
/// machine (so a local install can sign in without email). Otherwise `None`.
pub async fn request_magic_link(email: String) -> Result<Option<String>, ServerFnError> {
    let st = ctx::state()?;
    let ip = ctx::ip().await;
    if st.sign_in_blocked(&ip, None) {
        return Err(ServerFnError::new("Too many attempts. Try again in a few minutes."));
    }
    // Each request counts against the IP so the form can't be used to spam inboxes.
    st.sign_in_failed(&ip, None, "magic_link_requested");
    let local = ip.parse::<std::net::IpAddr>().is_ok_and(|a| a.is_loopback());
    // Development, on this machine only: say plainly when the address isn't the owner's.
    if st.cfg.dev && local && jugg_auth::find_user_by_email(&st.pool, &st.ring, &email).await.ok().flatten().is_none() {
        return Err(ServerFnError::new("No account on this box uses that email. Change it with: juggernaught owner set-email you@example.com (development-only message)"));
    }
    let link = crate::server::request_magic_link(&st, &email).await;
    Ok(link.filter(|_| st.cfg.dev && local))
}

#[cfg(feature = "ssr")]
async fn code_login(email: String, code: String, kind: &'static str) -> Result<(), ServerFnError> {
    {
        let st = ctx::state()?;
        let ip = ctx::ip().await;
        let account = jugg_crypto::blind_index(&st.ring, jugg_auth::normalise_email(&email).as_bytes()).map_err(|e| ServerFnError::new(e.to_string()))?;
        if st.sign_in_blocked(&ip, Some(&account)) {
            return Err(ServerFnError::new("Too many attempts. Try again in 15 minutes."));
        }
        let user = jugg_auth::find_user_by_email(&st.pool, &st.ring, &email).await.ok().flatten();
        let ok = match &user {
            Some(u) if kind == "totp" => jugg_auth::totp::verify(&st.pool, &st.ring, u, &code).await.is_ok(),
            Some(u) => jugg_auth::recovery::redeem(&st.pool, u, &code).await.is_ok(),
            None => false,
        };
        match (ok, user) {
            (true, Some(u)) => {
                if let Ok(mut l) = st.limiter.lock() {
                    l.succeed(&format!("acct:{account}"));
                }
                let cookie = crate::server::new_session(&st, &u, kind, &ctx::headers().await).await.map_err(ServerFnError::new)?;
                ctx::set_cookie(&cookie);
                Ok(())
            }
            _ => {
                st.sign_in_failed(&ip, Some(&account), if kind == "totp" { "totp_invalid" } else { "recovery_invalid" });
                Err(ServerFnError::new("That code didn't work."))
            }
        }
    }
}

#[server]
pub async fn login_totp(email: String, code: String) -> Result<(), ServerFnError> {
    code_login(email, code, "totp").await
}

#[server]
pub async fn login_recovery(email: String, code: String) -> Result<(), ServerFnError> {
    code_login(email, code, "recovery").await
}

#[server]
pub async fn get_sign_in_methods() -> Result<MethodsView, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    let e = |x: jugg_auth::AuthError| ServerFnError::new(x.to_string());
    let enc: Vec<u8> = jugg_db::sqlx::query_scalar("SELECT email_enc FROM users WHERE id = ?").bind(&user).fetch_one(&st.pool).await?;
    let email = jugg_crypto::open(&st.ring, &enc, format!("users:email:{user}").as_bytes()).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    Ok(MethodsView {
        email,
        passkeys: jugg_auth::passkey::list(&st.pool, &user).await.map_err(e)?.into_iter().map(|(_, l, _)| l).collect(),
        totp: jugg_auth::totp::is_active(&st.pool, &user).await.map_err(e)?,
        recovery_left: jugg_auth::recovery::remaining(&st.pool, &user).await.map_err(e)?,
    })
}

#[server]
pub async fn totp_begin() -> Result<TotpSetup, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    let issuer = format!("Juggernaught · {}", st.brand.read().await.name);
    let en = jugg_auth::totp::begin(&st.pool, &st.ring, &user, &issuer, "owner").await.map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(TotpSetup { qr_png_base64: en.qr_png_base64, secret: en.secret_base32 })
}

#[server]
pub async fn totp_confirm(code: String) -> Result<(), ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    jugg_auth::totp::confirm(&st.pool, &st.ring, &user, &code).await.map_err(|_| ServerFnError::new("That code didn't match. Check your phone's clock and try the newest code."))?;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), &user, "totp.enabled", &serde_json::json!({})).await;
    Ok(())
}

#[server]
pub async fn regenerate_recovery_codes() -> Result<Vec<String>, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    let codes = jugg_auth::recovery::regenerate(&st.pool, &user).await.map_err(|e| ServerFnError::new(e.to_string()))?;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), &user, "recovery.regenerated", &serde_json::json!({})).await;
    Ok(codes)
}

// ---------------------------------------------------------------- intelligence

#[server]
pub async fn get_providers() -> Result<ProvidersView, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    let row: Option<(String, String, Option<Vec<u8>>)> =
        jugg_db::sqlx::query_as("SELECT kind, config_json, secret_enc FROM providers WHERE id = 'active'").fetch_optional(&st.pool).await?;
    let p = st.provider().await;
    let health = p.health().await;
    let (kind, model, key) = match row {
        Some((k, c, s)) => (k, serde_json::from_str::<crate::server::ProviderConfig>(&c).ok().and_then(|c| c.model).unwrap_or_default(), s.is_some()),
        None => (format!("{:?}", p.kind()).to_lowercase(), String::new(), false),
    };
    Ok(ProvidersView {
        active_kind: kind,
        label: p.label(),
        ready: health.is_ok(),
        status: health.unwrap_or_else(|e| e.to_string()),
        model,
        has_openrouter_key: key,
    })
}

#[server]
pub async fn save_provider_settings(kind: String, model: Option<String>, api_key: Option<String>) -> Result<String, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    crate::server::save_provider(&st, &kind, model, api_key).await.map_err(ServerFnError::new)
}

// ---------------------------------------------------------------- panels

#[server]
pub async fn get_brand() -> Result<BrandView, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    let b = st.brand.read().await.clone();
    Ok(BrandView {
        name: b.name.clone(),
        colors: b.colors.iter().map(|c| (c.slug.clone(), c.hex.clone(), c.usage.clone())).collect(),
        heading_font: b.typography.heading.family.clone(),
        body_font: b.typography.body.family.clone(),
        design_md: b.to_design_md(),
    })
}

#[server]
pub async fn get_security() -> Result<SecurityView, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    Ok(crate::server::security_view(&st).await)
}

#[server]
pub async fn get_standards() -> Result<StandardsView, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    Ok(StandardsView {
        rules: jugg_web::RULES.iter().map(|r| (r.id.to_string(), format!("{:?}", r.level).to_lowercase(), r.chat_overridable, r.why.to_string())).collect(),
    })
}

#[server]
pub async fn get_tools() -> Result<Vec<ToolView>, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    let lock = std::fs::read_to_string(st.cfg.tools_dir.join("tools.lock.toml")).ok().and_then(|s| jugg_tools::lock::ToolsLock::parse(&s).ok()).unwrap_or_else(jugg_tools::lock::ToolsLock::embedded);
    let installed: std::collections::HashSet<String> = jugg_tools::install::status(&lock, &st.cfg.tools_dir).into_iter().filter(|t| t.installed).map(|t| t.name).collect();
    let mut out: Vec<ToolView> = st.registry.specs().into_iter().map(|s| ToolView { name: s.name, description: s.description, risk: format!("{:?}", s.risk).to_lowercase(), available: true }).collect();
    // Pinned tools that aren't installed yet, so the owner knows what's missing.
    for t in jugg_tools::install::status(&lock, &st.cfg.tools_dir).into_iter().filter(|t| !t.installed && !installed.contains(&t.name)) {
        out.push(ToolView { name: t.name, description: t.note, risk: "create".into(), available: false });
    }
    Ok(out)
}

// ---------------------------------------------------------------- assets

#[server]
pub async fn list_assets(kind: Option<String>) -> Result<Vec<AssetCard>, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    Ok(crate::assets::list(&st, kind).await)
}

#[server]
pub async fn decide_asset(id: String, approve: bool, reason: Option<String>) -> Result<AssetCard, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    crate::assets::decide(&st, &user, &id, approve, reason).await.map_err(ServerFnError::new)
}

#[server]
pub async fn list_jobs() -> Result<Vec<JobView>, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    Ok(crate::assets::jobs(&st).await)
}

// ---------------------------------------------------------------- website + approvals

#[server]
pub async fn decide_approval(id: String, approve: bool) -> Result<ApprovalView, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    crate::site::decide_approval(&st, &user, &id, approve).await.map_err(ServerFnError::new)
}

#[server]
pub async fn get_site() -> Result<SiteView, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    let pages = st.site.pages().await.into_iter().map(|(p, s, _)| (p.slug, p.title, s)).collect();
    let deploys: Vec<(String, String, i64)> = jugg_db::sqlx::query_as("SELECT id, status, created_at FROM site_deploys ORDER BY created_at DESC LIMIT 10").fetch_all(&st.pool).await?;
    let overrides: Vec<(String, String, String)> = jugg_db::sqlx::query_as("SELECT rule, target, reason FROM standards_overrides WHERE revoked_at IS NULL ORDER BY created_at DESC LIMIT 20").fetch_all(&st.pool).await?;
    Ok(SiteView { public_url: st.site.public_url(), live: deploys.iter().any(|d| d.1 == "live"), pages, deploys, submissions: crate::site::submissions(&st).await, overrides })
}

/// The owner pressing Publish in the Website panel is the approval.
#[server]
pub async fn publish_site() -> Result<String, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    st.site.publish(&user).await.map_err(ServerFnError::new)
}

#[server]
pub async fn rollback_site() -> Result<String, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    st.site.rollback(&user).await.map_err(ServerFnError::new)
}

// ---------------------------------------------------------------- brand studio

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogoConceptView {
    pub key: String,
    pub label: String,
    /// SVG as a data URI (inert in <img>).
    pub image: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrandKitView {
    pub summary: String,
    pub assets: Vec<AssetCard>,
}

#[server]
pub async fn logo_concept_previews() -> Result<Vec<LogoConceptView>, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    let b = st.brand.read().await.clone();
    let _ = st.site.fonts(&b).await; // brand fonts on disk before drawing
    let list = tokio::task::spawn_blocking(move || crate::studio::concepts_preview(&b)).await.map_err(|e| ServerFnError::new(e.to_string()))?.map_err(ServerFnError::new)?;
    Ok(list.into_iter().map(|(key, label, image)| LogoConceptView { key, label, image }).collect())
}

#[server]
pub async fn adopt_logo_concept(concept: String) -> Result<BrandKitView, ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    let c = crate::studio::parse_concept(&concept).ok_or_else(|| ServerFnError::new("Unknown logo concept."))?;
    let (summary, ids) = crate::studio::adopt(&st.site, c).await.map_err(ServerFnError::new)?;
    Ok(BrandKitView { summary, assets: crate::studio::cards(&st, &ids).await })
}

#[server]
pub async fn image_settings() -> Result<(bool, String), ServerFnError> {
    let st = ctx::state()?;
    ctx::user(&st).await?;
    Ok(match crate::studio::image_key(&st.site).await {
        Some((_, model)) => (true, model),
        None => (false, "gpt-image-1".into()),
    })
}

#[server]
pub async fn save_image_settings(key: Option<String>, model: Option<String>) -> Result<(), ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    crate::studio::save_image_key(&st.site, key, model).await.map_err(ServerFnError::new)?;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), &user, "provider.images_changed", &serde_json::json!({})).await;
    Ok(())
}

// ------------------------------------------------------------------ image generation (ChatGPT default)


/// An image the owner makes in their own ChatGPT and drops back into the chat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffView {
    pub id: String,
    pub title: String,
    /// The exact text to send in ChatGPT (brand rules included).
    pub prompt: String,
    pub shape: String,
    /// https://chatgpt.com/?q=… with the prompt filled in.
    pub open_url: String,
    /// waiting | done
    pub status: String,
    pub asset: Option<AssetCard>,
    /// An OpenAI API key is saved, so "make it automatically" is offered.
    pub api_key_set: bool,
}

#[server]
pub async fn handoff_use_api(id: String) -> Result<HandoffView, ServerFnError> {
    let st = ctx::state()?;
    let user = ctx::user(&st).await?;
    crate::images::make_with_api(&st, &user, &id).await.map_err(ServerFnError::new)
}







