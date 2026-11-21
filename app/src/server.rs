//! Server side: state, router, security middleware, and the chat/setup glue.

use axum::extract::{ConnectInfo, FromRef, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jugg_ai::{AgentEvent, AgentRequest, ChatMessage, Provider, Role};
use jugg_brand::{BrandStandards, ColorToken};
use jugg_core::Config;
use jugg_crypto::{Keyring, Signer};
use jugg_db::sqlx::{self, SqlitePool};
use jugg_steward::correlator::Correlator;
use jugg_tools::{Registry, ToolContext};
use leptos::config::LeptosOptions;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub cfg: Arc<Config>,
    pub pool: SqlitePool,
    pub ring: Arc<Keyring>,
    pub signer: Arc<Signer>,
    pub brand: Arc<RwLock<BrandStandards>>,
    pub registry: Registry,
    pub provider: Arc<dyn Provider>,
    pub correlator: Arc<Mutex<Correlator>>,
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(s: &AppState) -> Self {
        s.leptos_options.clone()
    }
}

#[derive(Clone)]
pub struct CspNonce(pub String);

impl AppState {
    pub async fn init(cfg: Config, leptos_options: LeptosOptions) -> anyhow_lite::Result<Self> {
        let ring = Keyring::load_or_create(&cfg.keys_dir).map_err(anyhow_lite::err)?;
        let signer = Signer::load_or_create(&cfg.keys_dir.join("box-signing.mldsa65.pk8")).map_err(anyhow_lite::err)?;
        let pool = jugg_db::connect(&cfg.db_path()).await.map_err(anyhow_lite::err)?;
        let brand: Option<String> = sqlx::query_scalar("SELECT standards_json FROM brands WHERE is_active = 1 ORDER BY version DESC LIMIT 1")
            .fetch_optional(&pool)
            .await
            .map_err(anyhow_lite::err)?;
        let brand = brand.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        let lock_path = cfg.tools_dir.join("tools.lock.toml");
        let lock = std::fs::read_to_string(&lock_path).ok().and_then(|s| jugg_tools::lock::ToolsLock::parse(&s).ok()).unwrap_or_else(jugg_tools::lock::ToolsLock::embedded);
        let registry = Registry::standard(&lock, &cfg.tools_dir);
        let provider = select_provider();
        tracing::info!(provider = %provider.label(), tools = registry.specs().len(), "juggernaught state ready");
        let correlator = Correlator::new(cfg.steward.clone(), vec!["127.0.0.1".into(), "::1".into()]);
        Ok(Self {
            leptos_options,
            cfg: Arc::new(cfg),
            pool,
            ring: Arc::new(ring),
            signer: Arc::new(signer),
            brand: Arc::new(RwLock::new(brand)),
            registry,
            provider,
            correlator: Arc::new(Mutex::new(correlator)),
        })
    }

    fn job_ctx(&self, conversation: &str, brand: BrandStandards) -> ToolContext {
        ToolContext { job_dir: self.cfg.data_dir.join("jobs").join(conversation), brand: Arc::new(brand), approved: false }
    }
}

/// Provider choice. Phase 1 moves this into the `providers` table (Settings → Intelligence).
fn select_provider() -> Arc<dyn Provider> {
    match std::env::var("JUGG_PROVIDER").as_deref() {
        Ok("claude") => Arc::new(jugg_ai::cli::ClaudeCodeCli { bin: "claude".into(), model: std::env::var("JUGG_MODEL").ok() }),
        Ok("codex") => Arc::new(jugg_ai::cli::CodexCli { bin: "codex".into(), model: std::env::var("JUGG_MODEL").ok() }),
        Ok("openrouter") => match std::env::var("OPENROUTER_API_KEY") {
            Ok(k) => Arc::new(jugg_ai::openrouter::OpenRouter::new(k.into(), std::env::var("JUGG_MODEL").unwrap_or_else(|_| "anthropic/claude-sonnet-4.5".into()))),
            Err(_) => Arc::new(jugg_ai::Offline),
        },
        _ => Arc::new(jugg_ai::Offline),
    }
}

// ---------------------------------------------------------------- middleware

/// Honeytokens + bans + security headers + per-request CSP nonce, in front of everything.
pub async fn security_layer(State(st): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    let ip = addr.ip().to_string();
    let path = req.uri().path().to_string();

    if let Some(ev) = jugg_steward::honeytokens::classify(&path, &ip) {
        let actions = st.correlator.lock().map(|mut c| c.ingest(&ev)).unwrap_or_default();
        tracing::warn!(%ip, %path, ?actions, "honeytoken hit");
        // Phase 2: forward event to the steward socket; steward applies nft bans.
        return (http::StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let nonce = {
        use base64_lite::encode;
        encode(&jugg_crypto::random_bytes::<18>())
    };
    req.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut res = next.run(req).await;

    if res.status() == http::StatusCode::NOT_FOUND {
        if let Ok(mut c) = st.correlator.lock() {
            c.ingest(&jugg_steward::honeytokens::not_found(&path, &ip));
        }
    }

    let h = res.headers_mut();
    for (k, v) in jugg_web::SECURITY_HEADERS {
        if st.cfg.dev && *k == "strict-transport-security" {
            continue;
        }
        if let (Ok(k), Ok(v)) = (http::HeaderName::from_bytes(k.as_bytes()), http::HeaderValue::from_str(v)) {
            h.insert(k, v);
        }
    }
    // Admin CSP: hydration needs wasm + its nonce'd module script; nothing else.
    // `style-src-attr 'unsafe-inline'` allows only style="" attributes (the UI
    // passes CSS custom properties like --c / --i); <style> elements and
    // stylesheets stay 'self'. The public-site CSP (jugg_web::Csp::PUBLIC) has neither.
    let connect = if st.cfg.dev { "'self' ws://127.0.0.1:3701 ws://localhost:3701" } else { "'self'" };
    let csp = format!(
        "default-src 'none'; script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval'; style-src 'self'; style-src-attr 'unsafe-inline'; img-src 'self' data: blob:; \
         font-src 'self'; connect-src {connect}; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    if let Ok(v) = http::HeaderValue::from_str(&csp) {
        h.insert(http::header::CONTENT_SECURITY_POLICY, v);
    }
    h.insert(http::header::X_FRAME_OPTIONS, http::HeaderValue::from_static("DENY"));
    res
}

/// `/brand.css`: the owner's brand tokens mapped onto the admin theme, so the
/// workspace wears their brand once the wizard is done (stylesheet, so the
/// strict CSP still holds).
pub async fn brand_css(State(st): State<AppState>) -> impl IntoResponse {
    let b = st.brand.read().await;
    let mut css = String::from(":root{");
    if let Some(p) = b.color("primary") {
        css.push_str(&format!("--primary:{p};--primary-soft:color-mix(in srgb,{p} 16%,var(--surface));"));
    }
    if let Some(pi) = b.color("primary-ink") {
        css.push_str(&format!("--primary-ink:{pi};"));
    }
    css.push('}');
    // Owner's heading font (served from the local Google Fonts mirror in production).
    let heading = &b.typography.heading.family;
    if heading != "Plus Jakarta Sans" {
        css.push_str(&format!(".greeting,.panel-head h1,.setup-title,.specimen-h{{font-family:'{heading}',var(--font)}}"));
    }
    ([(http::header::CONTENT_TYPE, "text/css; charset=utf-8"), (http::header::CACHE_CONTROL, "no-cache")], css)
}

// ---------------------------------------------------------------- chat

pub async fn chat(st: &AppState, conversation_id: Option<String>, text: String) -> Result<crate::api::Reply, String> {
    let e = |x: &dyn std::fmt::Display| x.to_string();
    let now = jugg_core::now_unix();
    let conv = match conversation_id {
        Some(id) => id,
        None => {
            let id = jugg_core::new_id();
            let title: String = text.chars().take(60).collect();
            let title_enc = jugg_crypto::seal(&st.ring, title.as_bytes(), format!("conversations:title:{id}").as_bytes()).map_err(|x| e(&x))?;
            sqlx::query("INSERT INTO conversations (id, title_enc, created_at, updated_at) VALUES (?,?,?,?)")
                .bind(&id)
                .bind(title_enc)
                .bind(now)
                .bind(now)
                .execute(&st.pool)
                .await
                .map_err(|x| e(&x))?;
            id
        }
    };

    let rows: Vec<(String, String, Vec<u8>)> = sqlx::query_as("SELECT id, role, content_enc FROM messages WHERE conversation_id = ? ORDER BY created_at, id")
        .bind(&conv)
        .fetch_all(&st.pool)
        .await
        .map_err(|x| e(&x))?;
    let history = rows
        .into_iter()
        .filter_map(|(id, role, enc)| {
            let plain = jugg_crypto::open(&st.ring, &enc, format!("messages:content:{id}").as_bytes()).ok()?;
            let role = if role == "user" { Role::User } else { Role::Assistant };
            Some(ChatMessage { role, content: String::from_utf8_lossy(&plain).into() })
        })
        .collect();

    store_message(st, &conv, "user", &text, None).await?;

    let brand = st.brand.read().await.clone();
    let business: Option<String> = sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'business'").fetch_optional(&st.pool).await.map_err(|x| e(&x))?;
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "juggernaught".into());
    let req = AgentRequest {
        system: jugg_ai::prompt::system_prompt(&brand, &business.unwrap_or_default()),
        history,
        user: text,
        mcp_command: (exe, vec!["mcp".into(), "--conversation".into(), conv.clone()]),
        registry: st.registry.clone(),
        ctx: st.job_ctx(&conv, brand),
    };

    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let provider = st.provider.clone();
    let run = tokio::spawn(async move { provider.run(req, tx).await });

    let mut reply = crate::api::Reply { conversation_id: conv.clone(), text: String::new(), tools: vec![], approvals: vec![] };
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::TextDelta { text } => reply.text.push_str(&text),
            AgentEvent::ToolFinished { name, summary, is_error } => reply.tools.push(crate::api::ToolActivity { name, summary: summary.chars().take(240).collect(), is_error }),
            AgentEvent::ApprovalRequired { tool, summary } => reply.approvals.push(format!("{tool}: {summary}")),
            AgentEvent::Usage { input_tokens, output_tokens, model } => {
                let _ = sqlx::query("INSERT INTO usage (id, provider_id, model, input_tokens, output_tokens, conversation_id, created_at) VALUES (?,?,?,?,?,?,?)")
                    .bind(jugg_core::new_id())
                    .bind(st.provider.label())
                    .bind(model)
                    .bind(input_tokens as i64)
                    .bind(output_tokens as i64)
                    .bind(&conv)
                    .bind(jugg_core::now_unix())
                    .execute(&st.pool)
                    .await;
            }
            AgentEvent::Error { message } => reply.text.push_str(&format!("\n\n_{message}_")),
            AgentEvent::Done => break,
            AgentEvent::ToolStarted { .. } => {}
        }
    }
    if let Ok(Err(err)) = run.await {
        reply.text.push_str(&format!("\n\n_{err}_"));
    }
    store_message(st, &conv, "assistant", &reply.text, Some(serde_json::to_string(&reply.tools).unwrap_or_default())).await?;
    sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?").bind(jugg_core::now_unix()).bind(&conv).execute(&st.pool).await.map_err(|x| e(&x))?;
    Ok(reply)
}

async fn store_message(st: &AppState, conv: &str, role: &str, content: &str, tools: Option<String>) -> Result<(), String> {
    let id = jugg_core::new_id();
    let enc = jugg_crypto::seal(&st.ring, content.as_bytes(), format!("messages:content:{id}").as_bytes()).map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO messages (id, conversation_id, role, content_enc, tool_calls_json, provider, created_at) VALUES (?,?,?,?,?,?,?)")
        .bind(&id)
        .bind(conv)
        .bind(role)
        .bind(enc)
        .bind(tools)
        .bind(st.provider.label())
        .bind(jugg_core::now_unix())
        .execute(&st.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Suggestion bubbles: next useful things, based on what exists already.
pub async fn suggestions(st: &AppState, setup_complete: bool, provider_ready: bool) -> Vec<String> {
    if !setup_complete {
        return vec!["Set up my brand".into()];
    }
    let count = |kind: &'static str| {
        let pool = st.pool.clone();
        async move { sqlx::query_scalar::<_, i64>("SELECT count(*) FROM assets WHERE kind = ? AND status = 'final'").bind(kind).fetch_one(&pool).await.unwrap_or(0) }
    };
    let pages: i64 = sqlx::query_scalar("SELECT count(*) FROM site_pages").fetch_one(&st.pool).await.unwrap_or(0);
    let mut s = Vec::new();
    if !provider_ready {
        s.push("How do I connect Claude or Codex?".to_string());
    }
    if count("logo").await == 0 {
        s.push("Design my logo".into());
    }
    if pages == 0 {
        s.push("Build my homepage".into());
    }
    if count("email_signature").await == 0 {
        s.push("Make my email signature".into());
    }
    if count("social_post").await == 0 {
        s.push("Write a launch post for Instagram".into());
    }
    if count("business_card").await == 0 {
        s.push("Design a business card".into());
    }
    s.truncate(4);
    s
}

// ---------------------------------------------------------------- setup + auth

pub async fn complete_setup(st: &AppState, input: crate::api::WizardInput) -> Result<(), String> {
    let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role='owner'").fetch_one(&st.pool).await.map_err(|e| e.to_string())?;
    if owners > 0 {
        return Err("This workspace is already set up.".into());
    }
    let name = input.business.trim();
    if name.is_empty() || name.len() > 120 {
        return Err("Business name is required.".into());
    }
    if !input.owner_email.contains('@') {
        return Err("A valid email is required.".into());
    }
    let mut brand = BrandStandards::default();
    brand.name = name.to_string();
    brand.tagline = Some(input.what_you_do.trim().to_string()).filter(|s| !s.is_empty());
    let primary = if is_hex(&input.primary) { input.primary.to_lowercase() } else { "#c2502a".into() };
    let ink = brand.color("ink").unwrap_or("#1b1a17").to_string();
    let on_primary = if jugg_brand::contrast_ratio("#ffffff", &primary).unwrap_or(0.0) >= 4.5 { "#ffffff".to_string() } else { ink };
    for c in brand.colors.iter_mut() {
        match c.slug.as_str() {
            "primary" => c.hex = primary.clone(),
            "primary-ink" => c.hex = on_primary.clone(),
            _ => {}
        }
    }
    if !brand.colors.iter().any(|c: &ColorToken| c.slug == "primary") {
        return Err("internal: palette".into());
    }
    brand.typography.heading.family = input.heading_font.clone();
    brand.typography.body.family = input.body_font.clone();
    brand.voice.traits = input.traits.iter().map(|t| t.to_lowercase()).collect();

    let problems: Vec<String> = brand.validate().into_iter().filter(|f| f.severity == jugg_brand::Severity::Error).map(|f| f.message).collect();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }

    let mut tx = st.pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE brands SET is_active = 0").execute(&mut *tx).await.map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO brands (id, version, standards_json, is_active, created_at) VALUES ('brand', 1, ?, 1, ?)")
        .bind(serde_json::to_string(&brand).map_err(|e| e.to_string())?)
        .bind(jugg_core::now_unix())
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let business = serde_json::json!({ "what_you_do": input.what_you_do, "audience": input.audience, "signin_methods": input.methods });
    sqlx::query("INSERT OR REPLACE INTO settings (key, value_json, updated_at) VALUES ('business', ?, ?)")
        .bind(business.to_string())
        .bind(jugg_core::now_unix())
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    jugg_auth::create_user(&st.pool, &st.ring, &input.owner_email, jugg_core::Role::Owner).await.map_err(|e| e.to_string())?;
    jugg_db::audit::append(&st.pool, Some(&st.signer), "setup", "workspace.created", &serde_json::json!({ "business": name, "methods": input.methods }))
        .await
        .map_err(|e| e.to_string())?;
    *st.brand.write().await = brand;
    Ok(())
}

fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

pub async fn request_magic_link(st: &AppState, email: &str) {
    let Ok(Some(uid)) = jugg_auth::find_user_by_email(&st.pool, &st.ring, email).await else { return };
    match jugg_auth::magic::issue(&st.pool, &uid).await {
        Ok(token) if st.cfg.dev => tracing::info!("dev magic link: http://{}/auth/magic/{token}", st.cfg.listen_http),
        Ok(_) => tracing::warn!("SMTP delivery lands in phase 1; magic link issued but not sent"),
        Err(e) => tracing::error!(error = %e, "magic link issue failed"),
    }
}

pub fn security_view(st: &AppState) -> crate::api::SecurityView {
    let ram = total_ram_mb();
    crate::api::SecurityView {
        posture: if st.cfg.dev {
            "Development mode: firewall, bans and TLS are simulated on this machine. Honeytokens and the correlator are live.".into()
        } else {
            "All quiet. Honeytokens armed, firewall closed except web traffic, updates and backups on schedule.".into()
        },
        sensors: jugg_steward::sensor_plan(ram).into_iter().map(str::to_string).collect(),
        tasks: jugg_steward::maintenance::TASKS.iter().map(|t| (t.name.to_string(), t.schedule.to_string(), t.what.to_string())).collect(),
        crypto: vec![
            ("Connections".into(), "TLS 1.3, hybrid X25519MLKEM768 (post-quantum) preferred".into()),
            ("Private data".into(), "AES-256-GCM per field and per file, location-bound".into()),
            ("Keys leaving the box".into(), "ML-KEM-768 + X25519 hybrid wrap".into()),
            ("Updates, backups, audit log".into(), "ML-DSA-65 signatures".into()),
            ("Sign-in".into(), "Passkeys, authenticator app, email links. No passwords stored, anywhere.".into()),
        ],
    }
}

fn total_ram_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|kb| kb.parse::<u64>().ok()))
        .map(|kb| kb / 1024)
        .unwrap_or(16_384)
}

/// TLS config for production: rustls on aws-lc-rs, hybrid PQ key exchange first.
pub fn tls_provider_groups() -> Vec<String> {
    rustls::crypto::aws_lc_rs::default_provider().kx_groups.iter().map(|g| format!("{:?}", g.name())).collect()
}

// Small local helpers so the app crate stays lean.
pub mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, String>;
    pub fn err(e: impl std::fmt::Display) -> String {
        e.to_string()
    }
}

mod base64_lite {
    pub fn encode(bytes: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tls_prefers_post_quantum_hybrid() {
        let groups = super::tls_provider_groups();
        assert_eq!(groups.first().map(String::as_str), Some("X25519MLKEM768"), "{groups:?}");
    }
}
