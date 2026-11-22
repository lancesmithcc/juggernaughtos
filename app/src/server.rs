//! Server side: state, router pieces, security + auth middleware, auth HTTP
//! flows, streaming chat, provider store, mail, setup.

use axum::extract::{ConnectInfo, FromRef, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use jugg_ai::{AgentEvent, AgentRequest, ChatMessage, Provider, Role};
use jugg_auth::passkey::{PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential};
use jugg_auth::ratelimit::Limiter;
use jugg_brand::BrandStandards;
use jugg_core::Config;
use jugg_crypto::{Keyring, Signer};
use jugg_db::sqlx::{self, SqlitePool};
use jugg_steward::correlator::Correlator;
use jugg_steward::{Event as SecEvent, Severity};
use jugg_tools::{Registry, ToolContext};
use leptos::config::LeptosOptions;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;

pub type ProviderSlot = Arc<RwLock<Arc<dyn Provider>>>;

#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub cfg: Arc<Config>,
    pub pool: SqlitePool,
    pub ring: Arc<Keyring>,
    pub signer: Arc<Signer>,
    pub brand: Arc<RwLock<BrandStandards>>,
    pub registry: Registry,
    pub provider: ProviderSlot,
    pub correlator: Arc<Mutex<Correlator>>,
    pub limiter: Arc<Mutex<Limiter>>,
    pub webauthn: Arc<jugg_auth::passkey::WebauthnCtx>,
    pub pending_reg: Arc<Pending<(String, PasskeyRegistration)>>,
    pub pending_auth: Arc<Pending<PasskeyAuthentication>>,
    pub turns: Arc<Pending<Turn>>,
    pub owner_exists: Arc<AtomicBool>,
    /// One-time token required to run the setup wizard on a production box.
    pub setup_token: Arc<Mutex<Option<String>>>,
    pub mailer: Option<Arc<Mailer>>,
    pub steward: jugg_steward::client::StewardClient,
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(s: &AppState) -> Self {
        s.leptos_options.clone()
    }
}

#[derive(Clone)]
pub struct CspNonce(pub String);

/// Short-lived server-side state keyed by an unguessable id (passkey
/// ceremonies, chat turns). Entries expire and are single-use.
pub struct Pending<T> {
    ttl: i64,
    map: Mutex<HashMap<String, (T, i64)>>,
}

impl<T> Pending<T> {
    pub fn new(ttl: i64) -> Self {
        Self { ttl, map: Mutex::new(HashMap::new()) }
    }
    pub fn put(&self, value: T) -> String {
        let id = token();
        let now = jugg_core::now_unix();
        if let Ok(mut m) = self.map.lock() {
            m.retain(|_, (_, at)| now - *at < self.ttl);
            m.insert(id.clone(), (value, now));
        }
        id
    }
    pub fn take(&self, id: &str) -> Option<T> {
        let now = jugg_core::now_unix();
        let (v, at) = self.map.lock().ok()?.remove(id)?;
        (now - at < self.ttl).then_some(v)
    }
}

pub struct Turn {
    pub user_id: String,
    pub conversation_id: String,
    pub text: String,
}

fn token() -> String {
    base64_lite::encode_url(&jugg_crypto::random_bytes::<24>())
}

impl AppState {
    pub async fn init(cfg: Config, leptos_options: LeptosOptions) -> Result<Self, String> {
        let ring = Keyring::load_or_create(&cfg.keys_dir).map_err(err)?;
        let signer = Signer::load_or_create(&cfg.keys_dir.join("box-signing.mldsa65.pk8")).map_err(err)?;
        let pool = jugg_db::connect(&cfg.db_path()).await.map_err(err)?;
        let brand: Option<String> = sqlx::query_scalar("SELECT standards_json FROM brands WHERE is_active = 1 ORDER BY version DESC LIMIT 1").fetch_optional(&pool).await.map_err(err)?;
        let brand = brand.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        let lock_path = cfg.tools_dir.join("tools.lock.toml");
        let lock = std::fs::read_to_string(&lock_path).ok().and_then(|s| jugg_tools::lock::ToolsLock::parse(&s).ok()).unwrap_or_else(jugg_tools::lock::ToolsLock::embedded);
        let registry = Registry::standard(&lock, &cfg.tools_dir);
        let provider = load_provider(&pool, &ring).await;
        let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE role='owner'").fetch_one(&pool).await.map_err(err)?;
        let (rp_id, origin) = (rp_id(&cfg), admin_origin(&cfg));
        let webauthn = jugg_auth::passkey::build(&rp_id, &origin).map_err(err)?;
        let mailer = Mailer::from_config(&cfg).map(Arc::new);

        let setup_token = if owners == 0 && !cfg.dev {
            let t = token();
            let path = cfg.keys_dir.join("setup-token");
            let _ = std::fs::remove_file(&path);
            write_private(&path, t.as_bytes()).map_err(err)?;
            tracing::warn!("No owner yet. Finish setup at {origin}/setup?token=<contents of {}> (or run `juggernaught setup-url`)", path.display());
            Some(t)
        } else {
            None
        };

        tracing::info!(provider = %provider.label(), tools = registry.specs().len(), rp_id, "juggernaught state ready");
        let correlator = Correlator::new(cfg.steward.clone(), vec!["127.0.0.1".into(), "::1".into()]);
        let cfg_socket = cfg.steward_socket.clone();
        Ok(Self {
            leptos_options,
            cfg: Arc::new(cfg),
            pool,
            ring: Arc::new(ring),
            signer: Arc::new(signer),
            brand: Arc::new(RwLock::new(brand)),
            registry,
            provider: Arc::new(RwLock::new(provider)),
            correlator: Arc::new(Mutex::new(correlator)),
            limiter: Arc::new(Mutex::new(Limiter::sign_in())),
            webauthn: Arc::new(webauthn),
            pending_reg: Arc::new(Pending::new(300)),
            pending_auth: Arc::new(Pending::new(300)),
            turns: Arc::new(Pending::new(120)),
            owner_exists: Arc::new(AtomicBool::new(owners > 0)),
            setup_token: Arc::new(Mutex::new(setup_token)),
            mailer,
            steward: jugg_steward::client::StewardClient::spawn(cfg_socket),
        })
    }

    fn job_ctx(&self, conversation: &str, brand: BrandStandards) -> ToolContext {
        ToolContext { job_dir: self.cfg.data_dir.join("jobs").join(conversation), brand: Arc::new(brand), approved: false }
    }

    pub async fn provider(&self) -> Arc<dyn Provider> {
        self.provider.read().await.clone()
    }

    /// Record a failed sign-in: rate limiter + steward correlator.
    pub fn sign_in_failed(&self, ip: &str, account: Option<&str>, kind: &str) {
        let now = jugg_core::now_unix();
        if let Ok(mut l) = self.limiter.lock() {
            l.fail(&format!("ip:{ip}"), now);
            if let Some(a) = account {
                l.fail(&format!("acct:{a}"), now);
            }
        }
        let ev = SecEvent::new("auth", kind, Severity::Low, 0.3, ip, json!({}));
        if let Ok(mut c) = self.correlator.lock() {
            c.ingest(&ev);
        }
        self.steward.send(ev);
    }

    pub fn sign_in_blocked(&self, ip: &str, account: Option<&str>) -> bool {
        let now = jugg_core::now_unix();
        self.limiter.lock().map(|mut l| l.blocked(&format!("ip:{ip}"), now) || account.is_some_and(|a| l.blocked(&format!("acct:{a}"), now))).unwrap_or(true)
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn admin_origin(cfg: &Config) -> String {
    match (&cfg.domain, cfg.dev) {
        (Some(d), false) => format!("https://admin.{d}"),
        _ => format!("http://localhost:{}", cfg.listen_http.rsplit(':').next().unwrap_or("3700")),
    }
}

fn rp_id(cfg: &Config) -> String {
    match (&cfg.domain, cfg.dev) {
        (Some(d), false) => d.clone(),
        _ => "localhost".into(),
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)?.write_all(bytes)
}

// ---------------------------------------------------------------- providers

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    #[serde(default)]
    pub model: Option<String>,
}

pub fn build_provider(kind: &str, model: Option<String>, secret: Option<String>) -> Arc<dyn Provider> {
    match kind {
        "claude_code_cli" => Arc::new(jugg_ai::cli::ClaudeCodeCli { bin: "claude".into(), model }),
        "codex_cli" => Arc::new(jugg_ai::cli::CodexCli { bin: "codex".into(), model }),
        "openrouter" => match secret {
            Some(k) if !k.is_empty() => Arc::new(jugg_ai::openrouter::OpenRouter::new(k.into(), model.unwrap_or_else(|| "anthropic/claude-sonnet-4.5".into()))),
            _ => Arc::new(jugg_ai::Offline),
        },
        _ => Arc::new(jugg_ai::Offline),
    }
}

/// Active provider from Settings; env vars are a developer fallback.
async fn load_provider(pool: &SqlitePool, ring: &Keyring) -> Arc<dyn Provider> {
    let row: Option<(String, String, Option<Vec<u8>>)> =
        sqlx::query_as("SELECT kind, config_json, secret_enc FROM providers WHERE enabled = 1 ORDER BY priority LIMIT 1").fetch_optional(pool).await.ok().flatten();
    if let Some((kind, cfg, secret)) = row {
        let model = serde_json::from_str::<ProviderConfig>(&cfg).ok().and_then(|c| c.model);
        let secret = secret.and_then(|s| jugg_crypto::open(ring, &s, b"providers:secret:active").ok()).and_then(|b| String::from_utf8(b).ok());
        return build_provider(&kind, model, secret);
    }
    match std::env::var("JUGG_PROVIDER").as_deref() {
        Ok("claude") => build_provider("claude_code_cli", std::env::var("JUGG_MODEL").ok(), None),
        Ok("codex") => build_provider("codex_cli", std::env::var("JUGG_MODEL").ok(), None),
        Ok("openrouter") => build_provider("openrouter", std::env::var("JUGG_MODEL").ok(), std::env::var("OPENROUTER_API_KEY").ok()),
        _ => Arc::new(jugg_ai::Offline),
    }
}

pub async fn save_provider(st: &AppState, kind: &str, model: Option<String>, api_key: Option<String>) -> Result<String, String> {
    if !["claude_code_cli", "codex_cli", "openrouter"].contains(&kind) {
        return Err("Unknown provider.".into());
    }
    let model = model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    let existing: Option<Option<Vec<u8>>> = sqlx::query_scalar("SELECT secret_enc FROM providers WHERE id = 'active'").fetch_optional(&st.pool).await.map_err(err)?;
    let secret_enc = match api_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        Some(k) => Some(jugg_crypto::seal(&st.ring, k.as_bytes(), b"providers:secret:active").map_err(err)?),
        None if kind == "openrouter" => existing.flatten(),
        None => None,
    };
    let secret = secret_enc.as_ref().and_then(|s| jugg_crypto::open(&st.ring, s, b"providers:secret:active").ok()).and_then(|b| String::from_utf8(b).ok());
    let provider = build_provider(kind, model.clone(), secret);
    let health = provider.health().await;
    let cfg = serde_json::to_string(&ProviderConfig { kind: kind.into(), model }).map_err(err)?;
    sqlx::query(
        "INSERT INTO providers (id, kind, label, config_json, secret_enc, priority, enabled, created_at) VALUES ('active',?,?,?,?,1,1,?)
         ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, label=excluded.label, config_json=excluded.config_json, secret_enc=excluded.secret_enc",
    )
    .bind(kind)
    .bind(provider.label())
    .bind(cfg)
    .bind(secret_enc)
    .bind(jugg_core::now_unix())
    .execute(&st.pool)
    .await
    .map_err(err)?;
    jugg_db::audit::append(&st.pool, Some(&st.signer), "owner", "provider.changed", &json!({ "kind": kind })).await.map_err(err)?;
    let label = provider.label();
    *st.provider.write().await = provider;
    Ok(match health {
        Ok(h) => format!("{label} connected ({h})."),
        Err(e) => format!("{label} saved, but it isn't ready yet: {e}"),
    })
}

// ---------------------------------------------------------------- mail

pub struct Mailer {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    from: lettre::message::Mailbox,
}

impl Mailer {
    /// SMTP from config; the password comes from a systemd credential
    /// (`LoadCredential=smtp_password`) or `JUGG_SMTP_PASSWORD`, never the config file.
    fn from_config(cfg: &Config) -> Option<Self> {
        let url = cfg.mail.smtp_url.as_ref()?;
        let from = cfg.mail.from.as_ref()?.parse().ok()?;
        let password = std::env::var_os("CREDENTIALS_DIRECTORY")
            .and_then(|d| std::fs::read_to_string(std::path::Path::new(&d).join("smtp_password")).ok())
            .or_else(|| std::env::var("JUGG_SMTP_PASSWORD").ok())
            .map(|p| p.trim().to_string());
        let mut b = lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::from_url(url).ok()?;
        if let Some(p) = password {
            let user = url.split("://").nth(1).and_then(|r| r.rsplit_once('@')).map(|(u, _)| u.to_string()).unwrap_or_default();
            b = b.credentials(lettre::transport::smtp::authentication::Credentials::new(user, p));
        }
        Some(Self { transport: b.build(), from })
    }

    pub async fn send(&self, to: &str, subject: &str, text: String, html: String) -> Result<(), String> {
        use lettre::AsyncTransport;
        let msg = lettre::Message::builder()
            .from(self.from.clone())
            .to(to.parse().map_err(err)?)
            .subject(subject)
            .multipart(lettre::message::MultiPart::alternative_plain_html(text, html))
            .map_err(err)?;
        self.transport.send(msg).await.map(|_| ()).map_err(err)
    }
}

// ---------------------------------------------------------------- middleware

pub fn client_ip(addr: &SocketAddr) -> String {
    addr.ip().to_string()
}

/// Honeytokens + security headers + per-request CSP nonce, in front of everything.
pub async fn security_layer(State(st): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, mut req: Request, next: Next) -> Response {
    let ip = client_ip(&addr);
    let path = req.uri().path().to_string();

    if let Some(ev) = jugg_steward::honeytokens::classify(&path, &ip) {
        let actions = st.correlator.lock().map(|mut c| c.ingest(&ev)).unwrap_or_default();
        tracing::warn!(%ip, %path, ?actions, "honeytoken hit");
        st.steward.send(ev);
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

    let nonce = base64_lite::encode(&jugg_crypto::random_bytes::<18>());
    req.extensions_mut().insert(CspNonce(nonce.clone()));
    let mut res = next.run(req).await;

    if res.status() == StatusCode::NOT_FOUND {
        let ev = jugg_steward::honeytokens::not_found(&path, &ip);
        if let Ok(mut c) = st.correlator.lock() {
            c.ingest(&ev);
        }
        st.steward.send(ev);
    }

    let h = res.headers_mut();
    for (k, v) in jugg_web::SECURITY_HEADERS {
        if st.cfg.dev && *k == "strict-transport-security" {
            continue;
        }
        if let (Ok(k), Ok(v)) = (header::HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
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
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        if let Ok(v) = HeaderValue::from_str(&csp) {
            h.insert(header::CONTENT_SECURITY_POLICY, v);
        }
    }
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    res
}

/// Every admin page needs a session. Server functions and the chat stream
/// check the session themselves (they answer 401 instead of redirecting).
pub async fn auth_gate(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let owner = st.owner_exists.load(Ordering::Relaxed);
    if path == "/setup" && owner {
        return Redirect::to("/").into_response();
    }
    let public = matches!(path, "/login" | "/setup" | "/favicon.svg" | "/brand.css")
        || ["/auth/", "/pkg/", "/fonts/", "/api/"].iter().any(|p| path.starts_with(p));
    if public || session_user(&st, req.headers()).await.is_some() {
        return next.run(req).await;
    }
    Redirect::to(if owner { "/login" } else { "/setup" }).into_response()
}

pub async fn session_user(st: &AppState, headers: &HeaderMap) -> Option<String> {
    let token = headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|c| jugg_auth::session::token_from_cookie_header(c, st.cfg.dev))?;
    jugg_auth::session::validate(&st.pool, &token).await.ok().map(|s| s.user_id)
}

pub async fn new_session(st: &AppState, user_id: &str, method: &str, headers: &HeaderMap) -> Result<String, String> {
    let ua = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(device_label);
    let token = jugg_auth::session::create(&st.pool, user_id, method, ua.as_deref()).await.map_err(err)?;
    jugg_db::audit::append(&st.pool, Some(&st.signer), user_id, "session.created", &json!({ "method": method, "device": ua })).await.map_err(err)?;
    Ok(jugg_auth::session::set_cookie(&token, st.cfg.dev))
}

fn device_label(ua: &str) -> String {
    let os = ["iPhone", "iPad", "Android", "Mac OS X", "Windows", "Linux"].into_iter().find(|o| ua.contains(o)).unwrap_or("Unknown device");
    let browser = ["Edg/", "Firefox/", "Chrome/", "Safari/"].into_iter().find(|b| ua.contains(b)).map(|b| b.trim_end_matches('/')).unwrap_or("browser");
    format!("{browser} on {}", os.replace("Mac OS X", "Mac"))
}

/// `/brand.css`: the owner's brand tokens mapped onto the admin theme.
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
    let heading = &b.typography.heading.family;
    if heading != "Plus Jakarta Sans" {
        css.push_str(&format!(".greeting,.panel-head h1,.setup-title,.specimen-h{{font-family:'{heading}',var(--font)}}"));
    }
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], css)
}

// ---------------------------------------------------------------- auth routes

fn json_err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn with_cookie(cookie: String, body: Response) -> Response {
    let mut r = body;
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        r.headers_mut().append(header::SET_COOKIE, v);
    }
    r
}

/// GET /auth/magic/{token} — the link from the email.
pub async fn magic_redeem(State(st): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Path(token): Path<String>, headers: HeaderMap) -> Response {
    let ip = client_ip(&addr);
    if st.sign_in_blocked(&ip, None) {
        return Redirect::to("/login?e=limited").into_response();
    }
    match jugg_auth::magic::redeem(&st.pool, &token).await {
        Ok(user) => match new_session(&st, &user, "magic_link", &headers).await {
            Ok(cookie) => with_cookie(cookie, Redirect::to("/").into_response()),
            Err(_) => Redirect::to("/login?e=server").into_response(),
        },
        Err(_) => {
            st.sign_in_failed(&ip, None, "magic_link_invalid");
            Redirect::to("/login?e=link").into_response()
        }
    }
}

pub async fn passkey_register_start(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = session_user(&st, &headers).await else { return json_err(StatusCode::UNAUTHORIZED, "Sign in first.") };
    let name = format!("{} owner", st.brand.read().await.name);
    match jugg_auth::passkey::start_registration(&st.webauthn, &st.pool, &st.ring, &user, &name).await {
        Ok((options, state)) => {
            let state_id = st.pending_reg.put((user, state));
            Json(json!({ "state_id": state_id, "options": options })).into_response()
        }
        Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
pub struct RegisterFinish {
    state_id: String,
    label: Option<String>,
    credential: RegisterPublicKeyCredential,
}

pub async fn passkey_register_finish(State(st): State<AppState>, headers: HeaderMap, Json(body): Json<RegisterFinish>) -> Response {
    let Some(user) = session_user(&st, &headers).await else { return json_err(StatusCode::UNAUTHORIZED, "Sign in first.") };
    let Some((owner, state)) = st.pending_reg.take(&body.state_id) else { return json_err(StatusCode::BAD_REQUEST, "That took too long. Try again.") };
    if owner != user {
        return json_err(StatusCode::FORBIDDEN, "Session mismatch.");
    }
    let label = body.label.filter(|l| !l.trim().is_empty()).unwrap_or_else(|| headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(device_label).unwrap_or_else(|| "Passkey".into()));
    match jugg_auth::passkey::finish_registration(&st.webauthn, &st.pool, &st.ring, &user, &label, &body.credential, &state).await {
        Ok(()) => {
            let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), &user, "passkey.added", &json!({ "label": label })).await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(_) => json_err(StatusCode::BAD_REQUEST, "The passkey couldn't be verified."),
    }
}

pub async fn passkey_login_start(State(st): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Response {
    if st.sign_in_blocked(&client_ip(&addr), None) {
        return json_err(StatusCode::TOO_MANY_REQUESTS, "Too many attempts. Try again in a few minutes.");
    }
    match jugg_auth::passkey::start_login_any(&st.webauthn, &st.pool, &st.ring).await {
        Ok((options, state)) => Json(json!({ "state_id": st.pending_auth.put(state), "options": options })).into_response(),
        Err(_) => json_err(StatusCode::NOT_FOUND, "No passkeys are set up yet. Use an email link."),
    }
}

#[derive(Deserialize)]
pub struct LoginFinish {
    state_id: String,
    credential: PublicKeyCredential,
}

pub async fn passkey_login_finish(State(st): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<LoginFinish>) -> Response {
    let ip = client_ip(&addr);
    let Some(state) = st.pending_auth.take(&body.state_id) else { return json_err(StatusCode::BAD_REQUEST, "That took too long. Try again.") };
    match jugg_auth::passkey::finish_login_any(&st.webauthn, &st.pool, &st.ring, &body.credential, &state).await {
        Ok(user) => match new_session(&st, &user, "passkey", &headers).await {
            Ok(cookie) => with_cookie(cookie, Json(json!({ "ok": true })).into_response()),
            Err(e) => json_err(StatusCode::INTERNAL_SERVER_ERROR, &e),
        },
        Err(_) => {
            st.sign_in_failed(&ip, None, "passkey_invalid");
            json_err(StatusCode::UNAUTHORIZED, "That passkey wasn't accepted.")
        }
    }
}

pub async fn logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(t) = headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(|c| jugg_auth::session::token_from_cookie_header(c, st.cfg.dev)) {
        let _ = jugg_auth::session::revoke(&st.pool, &t).await;
    }
    with_cookie(jugg_auth::session::clear_cookie(st.cfg.dev), Redirect::to("/login").into_response())
}

// ---------------------------------------------------------------- chat

/// Store the owner's message and hand back a single-use ticket for the stream.
pub async fn start_turn(st: &AppState, user_id: &str, conversation_id: Option<String>, text: String) -> Result<crate::api::TurnTicket, String> {
    let now = jugg_core::now_unix();
    let conv = match conversation_id {
        Some(id) => {
            let owned: Option<String> = sqlx::query_scalar("SELECT id FROM conversations WHERE id = ? AND user_id = ?").bind(&id).bind(user_id).fetch_optional(&st.pool).await.map_err(err)?;
            owned.ok_or("Conversation not found.")?
        }
        None => {
            let id = jugg_core::new_id();
            let title: String = text.chars().take(60).collect();
            let title_enc = jugg_crypto::seal(&st.ring, title.as_bytes(), format!("conversations:title:{id}").as_bytes()).map_err(err)?;
            sqlx::query("INSERT INTO conversations (id, title_enc, user_id, created_at, updated_at) VALUES (?,?,?,?,?)")
                .bind(&id)
                .bind(title_enc)
                .bind(user_id)
                .bind(now)
                .bind(now)
                .execute(&st.pool)
                .await
                .map_err(err)?;
            id
        }
    };
    let turn_id = st.turns.put(Turn { user_id: user_id.into(), conversation_id: conv.clone(), text });
    Ok(crate::api::TurnTicket { conversation_id: conv, turn_id })
}

/// GET /api/chat/stream/{turn_id} — Server-Sent Events of the agent's work.
pub async fn chat_stream(State(st): State<AppState>, Path(turn_id): Path<String>, headers: HeaderMap) -> Response {
    let Some(user) = session_user(&st, &headers).await else { return StatusCode::UNAUTHORIZED.into_response() };
    let Some(turn) = st.turns.take(&turn_id) else { return StatusCode::NOT_FOUND.into_response() };
    if turn.user_id != user {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
    tokio::spawn(run_turn(st.clone(), turn, tx));
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        let ev = rx.recv().await?;
        let sse = SseEvent::default().json_data(&ev).unwrap_or_else(|_| SseEvent::default().data("{}"));
        Some((Ok::<_, Infallible>(sse), rx))
    });
    let mut res = Sse::new(stream).keep_alive(KeepAlive::default()).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

async fn run_turn(st: AppState, turn: Turn, out: tokio::sync::mpsc::Sender<AgentEvent>) {
    let Turn { conversation_id: conv, text, .. } = turn;
    let history = load_history(&st, &conv).await;
    if let Err(e) = store_message(&st, &conv, "user", &text, None, "owner").await {
        let _ = out.send(AgentEvent::Error { message: e }).await;
        let _ = out.send(AgentEvent::Done).await;
        return;
    }
    let brand = st.brand.read().await.clone();
    let business: Option<String> = sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'business'").fetch_optional(&st.pool).await.ok().flatten();
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "juggernaught".into());
    let provider = st.provider().await;
    let label = provider.label();
    let ctx = st.job_ctx(&conv, brand.clone());
    if let Err(e) = std::fs::create_dir_all(&ctx.job_dir) {
        let _ = out.send(AgentEvent::Error { message: format!("Couldn't prepare the workspace: {e}") }).await;
        let _ = out.send(AgentEvent::Done).await;
        return;
    }
    let req = AgentRequest {
        system: jugg_ai::prompt::system_prompt(&brand, &business.unwrap_or_default()),
        history,
        user: text,
        mcp_command: (exe, vec!["mcp".into(), "--conversation".into(), conv.clone()]),
        registry: st.registry.clone(),
        ctx,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let run = tokio::spawn(async move { provider.run(req, tx).await });

    let mut reply = String::new();
    let mut tools = Vec::new();
    let mut client_gone = false;
    while let Some(ev) = rx.recv().await {
        match &ev {
            AgentEvent::TextDelta { text } => reply.push_str(text),
            AgentEvent::ToolFinished { name, summary, is_error } => tools.push(json!({ "name": name, "summary": summary, "is_error": is_error })),
            AgentEvent::Error { message } => reply.push_str(&format!("\n\n_{message}_")),
            AgentEvent::Usage { input_tokens, output_tokens, model } => {
                let _ = sqlx::query("INSERT INTO usage (id, provider_id, model, input_tokens, output_tokens, conversation_id, created_at) VALUES (?,?,?,?,?,?,?)")
                    .bind(jugg_core::new_id())
                    .bind(&label)
                    .bind(model)
                    .bind(*input_tokens as i64)
                    .bind(*output_tokens as i64)
                    .bind(&conv)
                    .bind(jugg_core::now_unix())
                    .execute(&st.pool)
                    .await;
            }
            _ => {}
        }
        if matches!(ev, AgentEvent::Done) {
            break;
        }
        // Keep working (and saving) even if the owner closed the tab.
        if !client_gone && out.send(ev).await.is_err() {
            client_gone = true;
        }
    }
    if let Ok(Err(e)) = run.await {
        let msg = format!("\n\n_{e}_");
        reply.push_str(&msg);
        let _ = out.send(AgentEvent::TextDelta { text: msg }).await;
    }
    let _ = store_message(&st, &conv, "assistant", &reply, Some(serde_json::Value::Array(tools).to_string()), &label).await;
    let _ = sqlx::query("UPDATE conversations SET updated_at = ? WHERE id = ?").bind(jugg_core::now_unix()).bind(&conv).execute(&st.pool).await;
    let _ = out.send(AgentEvent::Done).await;
}

async fn load_history(st: &AppState, conv: &str) -> Vec<ChatMessage> {
    let rows: Vec<(String, String, Vec<u8>)> = sqlx::query_as("SELECT id, role, content_enc FROM messages WHERE conversation_id = ? ORDER BY created_at, id")
        .bind(conv)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|(id, role, enc)| {
            let plain = jugg_crypto::open(&st.ring, &enc, format!("messages:content:{id}").as_bytes()).ok()?;
            let role = if role == "user" { Role::User } else { Role::Assistant };
            Some(ChatMessage { role, content: String::from_utf8_lossy(&plain).into() })
        })
        .collect()
}

async fn store_message(st: &AppState, conv: &str, role: &str, content: &str, tools: Option<String>, provider: &str) -> Result<(), String> {
    let id = jugg_core::new_id();
    let enc = jugg_crypto::seal(&st.ring, content.as_bytes(), format!("messages:content:{id}").as_bytes()).map_err(err)?;
    sqlx::query("INSERT INTO messages (id, conversation_id, role, content_enc, tool_calls_json, provider, created_at) VALUES (?,?,?,?,?,?,?)")
        .bind(&id)
        .bind(conv)
        .bind(role)
        .bind(enc)
        .bind(tools)
        .bind(provider)
        .bind(jugg_core::now_unix())
        .execute(&st.pool)
        .await
        .map_err(err)?;
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

// ---------------------------------------------------------------- setup

/// Create the brand and the owner, start their session, return recovery codes.
pub async fn complete_setup(st: &AppState, input: crate::api::WizardInput) -> Result<(String, Vec<String>), String> {
    if !st.cfg.dev {
        let expected = st.setup_token.lock().map_err(err)?.clone();
        match expected {
            Some(t) if jugg_crypto::ct_eq(t.as_bytes(), input.token.trim().as_bytes()) => {}
            _ => return Err("This setup link is missing or invalid. Run `juggernaught setup-url` on the server.".into()),
        }
    }
    if st.owner_exists.load(Ordering::SeqCst) {
        return Err("This workspace is already set up.".into());
    }
    let name = input.business.trim();
    if name.is_empty() || name.len() > 120 {
        return Err("Business name is required.".into());
    }
    if !input.owner_email.contains('@') || input.owner_email.len() > 254 {
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
    let font_ok = |f: &str| !f.is_empty() && f.len() < 60 && f.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ');
    if font_ok(&input.heading_font) {
        brand.typography.heading.family = input.heading_font.clone();
    }
    if font_ok(&input.body_font) {
        brand.typography.body.family = input.body_font.clone();
    }
    brand.voice.traits = input.traits.iter().take(3).map(|t| t.to_lowercase()).collect();

    let problems: Vec<String> = brand.validate().into_iter().filter(|f| f.severity == jugg_brand::Severity::Error).map(|f| f.message).collect();
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }

    let mut tx = st.pool.begin().await.map_err(err)?;
    sqlx::query("UPDATE brands SET is_active = 0").execute(&mut *tx).await.map_err(err)?;
    sqlx::query("INSERT INTO brands (id, version, standards_json, is_active, created_at) VALUES ('brand', 1, ?, 1, ?)")
        .bind(serde_json::to_string(&brand).map_err(err)?)
        .bind(jugg_core::now_unix())
        .execute(&mut *tx)
        .await
        .map_err(err)?;
    let methods: Vec<&String> = input.methods.iter().filter(|m| ["passkey", "totp", "magic_link"].contains(&m.as_str())).collect();
    let business = json!({ "what_you_do": input.what_you_do, "audience": input.audience, "signin_methods": methods });
    sqlx::query("INSERT OR REPLACE INTO settings (key, value_json, updated_at) VALUES ('business', ?, ?)").bind(business.to_string()).bind(jugg_core::now_unix()).execute(&mut *tx).await.map_err(err)?;
    tx.commit().await.map_err(err)?;

    let user = jugg_auth::create_user(&st.pool, &st.ring, &input.owner_email, jugg_core::Role::Owner).await.map_err(err)?;
    let codes = jugg_auth::recovery::regenerate(&st.pool, &user).await.map_err(err)?;
    jugg_db::audit::append(&st.pool, Some(&st.signer), "setup", "workspace.created", &json!({ "business": name, "methods": methods })).await.map_err(err)?;
    st.owner_exists.store(true, Ordering::SeqCst);
    if let Ok(mut t) = st.setup_token.lock() {
        *t = None;
    }
    let _ = std::fs::remove_file(st.cfg.keys_dir.join("setup-token"));
    *st.brand.write().await = brand;
    Ok((user, codes))
}

fn is_hex(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

pub async fn request_magic_link(st: &AppState, email: &str) {
    let Ok(Some(uid)) = jugg_auth::find_user_by_email(&st.pool, &st.ring, email).await else { return };
    let token = match jugg_auth::magic::issue(&st.pool, &uid).await {
        Ok(t) => t,
        Err(e) => return tracing::error!(error = %e, "magic link issue failed"),
    };
    let link = format!("{}/auth/magic/{token}", admin_origin(&st.cfg));
    let business = st.brand.read().await.name.clone();
    match &st.mailer {
        Some(m) => {
            let text = format!("Sign in to {business}'s Juggernaught workspace:\n\n{link}\n\nThis link works once and expires in 15 minutes. If you didn't ask for it, ignore this email.");
            let html = format!(
                "<p>Sign in to <strong>{b}</strong>'s Juggernaught workspace.</p><p><a href=\"{l}\">Sign in</a></p><p style=\"color:#666\">This link works once and expires in 15 minutes. If you didn't ask for it, ignore this email.</p>",
                b = html_escape(&business),
                l = html_escape(&link)
            );
            if let Err(e) = m.send(email, "Your sign-in link", text, html).await {
                tracing::error!(error = %e, "magic link email failed");
            }
        }
        None if st.cfg.dev => tracing::info!("dev magic link (no SMTP configured): {link}"),
        None => tracing::error!("magic link requested but SMTP is not configured ([mail] in juggernaught.toml)"),
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub async fn security_view(st: &AppState) -> crate::api::SecurityView {
    let ram = total_ram_mb();
    let steward = st.steward.status().await;
    let mut sessions: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT COALESCE(device_label,'Unknown device'), method, last_seen_at FROM sessions WHERE revoked_at IS NULL AND expires_at > ? ORDER BY last_seen_at DESC LIMIT 10")
            .bind(jugg_core::now_unix())
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
    sessions.truncate(10);
    crate::api::SecurityView {
        posture: posture(st, steward.as_ref()),
        checks: steward.as_ref().map(|s| s.checks.iter().map(|c| (c.name.clone(), c.status.clone(), c.detail.clone())).collect()).unwrap_or_default(),
        incidents: steward.as_ref().map(|s| s.open_incidents.iter().map(|i| (i.subject.clone(), i.score)).collect()).unwrap_or_default(),
        bans: steward.as_ref().map(|s| s.active_bans).unwrap_or(0),
        sensors: jugg_steward::sensor_plan(ram).into_iter().map(str::to_string).collect(),
        tasks: jugg_steward::maintenance::TASKS.iter().map(|t| (t.name.to_string(), t.schedule.to_string(), t.what.to_string())).collect(),
        crypto: vec![
            ("Connections".into(), "TLS 1.3, hybrid X25519MLKEM768 (post-quantum) preferred".into()),
            ("Private data".into(), "AES-256-GCM per field and per file, location-bound".into()),
            ("Keys leaving the box".into(), "ML-KEM-768 + X25519 hybrid wrap".into()),
            ("Updates, backups, audit log".into(), "ML-DSA-65 signatures".into()),
            ("Sign-in".into(), "Passkeys, authenticator app, email links. No passwords stored, anywhere.".into()),
        ],
        sessions: sessions.into_iter().map(|(d, m, at)| (d, m.replace('_', " "), at)).collect(),
    }
}

fn posture(st: &AppState, s: Option<&jugg_steward::daemon::Status>) -> String {
    match s {
        None if st.cfg.dev => "Development mode: the Steward daemon isn't running (start it with `juggernaught steward`). Honeytokens, sign-in limits and the in-app correlator are live.".into(),
        None => "The Steward isn't responding. It restarts automatically; if this persists, run `juggernaught doctor`.".into(),
        Some(s) => {
            let failing = s.checks.iter().filter(|c| c.status == "fail").count();
            let warns = s.checks.iter().filter(|c| c.status == "warn").count();
            let fw = if s.firewall == "nftables" { "Firewall enforcing" } else { "Firewall simulated (development)" };
            match (failing, s.open_incidents.len(), warns) {
                (0, 0, 0) => format!("All quiet. {fw}; {} security events in the last day, {} addresses banned.", s.events_24h, s.active_bans),
                (0, 0, w) => format!("Running well, {w} thing(s) to keep an eye on. {fw}."),
                (f, i, _) => format!("Needs attention: {f} failing check(s), {i} open incident(s). {fw}."),
            }
        }
    }
}

fn total_ram_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|kb| kb.parse::<u64>().ok()))
        .map(|kb| kb / 1024)
        .unwrap_or(16_384)
}

/// TLS provider check: rustls on aws-lc-rs, hybrid PQ key exchange first.
pub fn tls_provider_groups() -> Vec<String> {
    rustls::crypto::aws_lc_rs::default_provider().kx_groups.iter().map(|g| format!("{:?}", g.name())).collect()
}

mod base64_lite {
    const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    fn enc(bytes: &[u8], t: &[u8; 64], pad: bool) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for c in bytes.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                if i <= c.len() {
                    out.push(t[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else if pad {
                    out.push('=');
                }
            }
        }
        out
    }
    pub fn encode(bytes: &[u8]) -> String {
        enc(bytes, STD, true)
    }
    pub fn encode_url(bytes: &[u8]) -> String {
        enc(bytes, URL, false)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tls_prefers_post_quantum_hybrid() {
        let groups = super::tls_provider_groups();
        assert_eq!(groups.first().map(String::as_str), Some("X25519MLKEM768"), "{groups:?}");
    }

    #[test]
    fn pending_is_single_use() {
        let p = super::Pending::new(60);
        let id = p.put(7);
        assert_eq!(p.take(&id), Some(7));
        assert_eq!(p.take(&id), None);
        assert_eq!(super::Pending::<u8>::new(60).take("nope"), None);
    }

    #[test]
    fn device_labels() {
        assert_eq!(super::device_label("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit Chrome/131 Safari/537"), "Chrome on Mac");
    }
}
