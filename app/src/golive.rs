//! Go live: put this Juggernaught on the internet under the owner's own domain.
//!
//! Phase A (here): the domain and Cloudflare. Juggernaught does everything it can on
//! its own (availability and registrar lookups, adding the domain to Cloudflare,
//! watching nameservers move) and stops at the steps only the owner can do: making
//! accounts, paying, creating an API token, changing nameservers at their registrar.
//! A background check notices when those are done and carries on.
//!
//! The Cloudflare token is sealed like other provider secrets and never leaves the
//! server. Juggernaught never buys anything: the owner pays on Cloudflare's own page.

use crate::api::{GoLiveStep, GoLiveView};
use crate::site::SiteEnv;
use jugg_db::sqlx;
use serde_json::{json, Value};

const TOKEN_AAD: &[u8] = b"golive:cloudflare_token";
const CF_API: &str = "https://api.cloudflare.com/client/v4";

/// Cloudflare's token page with exactly the permissions Go live needs pre-selected:
/// add the domain (Zone Edit), manage its DNS (DNS Edit), find the account
/// (Account Settings Read).
pub fn token_url() -> String {
    let perms = r#"[{"key":"zone","type":"edit"},{"key":"dns","type":"edit"},{"key":"account_settings","type":"read"}]"#;
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!("https://dash.cloudflare.com/profile/api-tokens?permissionGroupKeys={}&accountId=%2A&zoneId=all&name={}", enc(perms), enc("Juggernaught Go live"))
}

pub const SIGN_UP_URL: &str = "https://dash.cloudflare.com/sign-up";
/// Cloudflare's own register page (sells at cost); `:account` picks the owner's account.
pub const BUY_URL: &str = "https://dash.cloudflare.com/?to=/:account/domains/register";

/// `https://www.Example.com/` → `example.com`. Only plain ASCII hostnames with a dot.
pub fn normalise_domain(input: &str) -> Result<String, String> {
    let mut d = input.trim().to_ascii_lowercase();
    for p in ["https://", "http://"] {
        if let Some(rest) = d.strip_prefix(p) {
            d = rest.to_string();
        }
    }
    let d = d.split(['/', '?', '#']).next().unwrap_or_default().trim_end_matches('.');
    let d = d.strip_prefix("www.").unwrap_or(d);
    let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let labels: Vec<&str> = d.split('.').collect();
    if d.len() > 253 || labels.len() < 2 || !labels.iter().all(|l| label_ok(l)) || labels.last().is_some_and(|t| t.chars().all(|c| c.is_ascii_digit())) {
        return Err("That doesn't look like a domain. Type it like yourbusiness.com".into());
    }
    Ok(d.to_string())
}

#[derive(Debug, Default, Clone)]
struct Row {
    domain: Option<String>,
    registered: Option<bool>,
    registrar: Option<String>,
    has_token: bool,
    zone_id: Option<String>,
    zone_status: Option<String>,
    nameservers: Vec<String>,
    current_ns: Vec<String>,
    target: Option<String>,
    last_error: Option<String>,
    checked_at: Option<i64>,
}

type Raw = (Option<String>, Option<i64>, Option<String>, Option<Vec<u8>>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>);

async fn load(env: &SiteEnv) -> Row {
    let r: Option<Raw> = sqlx::query_as(
        "SELECT domain, registered, registrar, cf_token_enc, cf_zone_id, zone_status, nameservers, current_ns, target, last_error, checked_at FROM golive WHERE id = 'golive'",
    )
    .fetch_optional(&env.pool)
    .await
    .ok()
    .flatten();
    let Some((domain, registered, registrar, tok, zone_id, zone_status, ns, cur, target, last_error, checked_at)) = r else { return Row::default() };
    let list = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default();
    Row { domain, registered: registered.map(|v| v == 1), registrar, has_token: tok.is_some(), zone_id, zone_status, nameservers: list(ns), current_ns: list(cur), target, last_error, checked_at }
}

async fn ensure_row(env: &SiteEnv) -> Result<(), String> {
    sqlx::query("INSERT INTO golive (id, updated_at) VALUES ('golive', ?) ON CONFLICT(id) DO NOTHING").bind(jugg_core::now_unix()).execute(&env.pool).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn token(env: &SiteEnv) -> Option<String> {
    let enc: Option<Vec<u8>> = sqlx::query_scalar("SELECT cf_token_enc FROM golive WHERE id = 'golive'").fetch_optional(&env.pool).await.ok().flatten().flatten();
    String::from_utf8(jugg_crypto::open(&env.ring, &enc?, TOKEN_AAD).ok()?).ok()
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).user_agent("Juggernaught-OS").build().unwrap_or_default()
}

// ------------------------------------------------------------------ public lookups

/// RDAP: is the domain registered, and with whom? `None` when the registry can't say.
async fn rdap(domain: &str) -> (Option<bool>, Option<String>) {
    let Ok(r) = http().get(format!("https://rdap.org/domain/{domain}")).header("accept", "application/rdap+json").send().await else { return (None, None) };
    match r.status().as_u16() {
        404 => (Some(false), None),
        200 => {
            let v: Value = r.json().await.unwrap_or_default();
            (Some(true), registrar_name(&v))
        }
        _ => (None, None),
    }
}

/// The registrar's display name from an RDAP domain record (its vCard `fn`).
fn registrar_name(v: &Value) -> Option<String> {
    v["entities"].as_array()?.iter().find(|e| e["roles"].as_array().is_some_and(|r| r.iter().any(|x| x == "registrar"))).and_then(|e| {
        e["vcardArray"][1].as_array()?.iter().find(|p| p[0] == "fn").and_then(|p| p[3].as_str()).map(|s| s.chars().take(80).collect())
    })
}

/// The domain's nameservers as public DNS sees them (DNS over HTTPS).
async fn public_ns(domain: &str) -> Vec<String> {
    let Ok(r) = http().get(format!("https://cloudflare-dns.com/dns-query?name={domain}&type=NS")).header("accept", "application/dns-json").send().await else { return vec![] };
    let v: Value = r.json().await.unwrap_or_default();
    let mut ns: Vec<String> = v["Answer"].as_array().map(|a| a.iter().filter(|x| x["type"] == 2).filter_map(|x| x["data"].as_str()).map(|s| s.trim_end_matches('.').to_ascii_lowercase()).collect()).unwrap_or_default();
    ns.sort();
    ns
}

// ------------------------------------------------------------------ Cloudflare API

struct Cf {
    token: String,
}

impl Cf {
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut req = http().request(method, format!("{CF_API}{path}")).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let v: Value = req.send().await.map_err(|e| format!("couldn't reach Cloudflare: {e}"))?.json().await.map_err(|e| format!("Cloudflare sent something unexpected: {e}"))?;
        if v["success"].as_bool() == Some(true) {
            return Ok(v["result"].clone());
        }
        let msgs: Vec<String> = v["errors"].as_array().map(|a| a.iter().map(|e| format!("{} ({})", e["message"].as_str().unwrap_or("error"), e["code"])).collect()).unwrap_or_default();
        Err(if msgs.is_empty() { "Cloudflare said no".into() } else { msgs.join("; ") })
    }

    async fn verify(&self) -> Result<(), String> {
        let r = self.call(reqwest::Method::GET, "/user/tokens/verify", None).await.map_err(|e| {
            tracing::info!(error = %e, "cloudflare token rejected");
            "Cloudflare didn't accept that token. Copy it again from Cloudflare (it's only shown once; if it's gone, create a new one with the link).".to_string()
        })?;
        if r["status"] == "active" { Ok(()) } else { Err("That token isn't active. Create a new one with the link.".into()) }
    }

    async fn account(&self) -> Result<String, String> {
        let r = self.call(reqwest::Method::GET, "/accounts?per_page=5", None).await?;
        r.as_array().and_then(|a| a.first()).and_then(|a| a["id"].as_str()).map(str::to_string).ok_or_else(|| "The token can't see your Cloudflare account. Create it again with the link (it includes Account Settings: Read).".into())
    }

    async fn zone(&self, domain: &str) -> Result<Option<Value>, String> {
        let r = self.call(reqwest::Method::GET, &format!("/zones?name={domain}"), None).await?;
        Ok(r.as_array().and_then(|a| a.first()).cloned())
    }

    async fn add_zone(&self, account: &str, domain: &str) -> Result<Value, String> {
        self.call(reqwest::Method::POST, "/zones", Some(json!({ "name": domain, "account": { "id": account }, "type": "full" }))).await.map_err(|e| format!("Cloudflare couldn't add {domain}: {e}"))
    }
}

// ------------------------------------------------------------------ steps the owner takes

pub async fn set_domain(env: &SiteEnv, input: &str) -> Result<GoLiveView, String> {
    let domain = normalise_domain(input)?;
    ensure_row(env).await?;
    sqlx::query("UPDATE golive SET domain = ?, registered = NULL, registrar = NULL, cf_zone_id = NULL, zone_status = NULL, nameservers = NULL, current_ns = NULL, last_error = NULL, updated_at = ? WHERE id = 'golive'")
        .bind(&domain)
        .bind(jugg_core::now_unix())
        .execute(&env.pool)
        .await
        .map_err(|e| e.to_string())?;
    let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), "owner", "golive.domain_set", &json!({ "domain": domain })).await;
    refresh(env).await;
    Ok(view(env).await)
}

pub async fn save_token(env: &SiteEnv, token: &str) -> Result<GoLiveView, String> {
    let token = token.trim();
    if token.len() < 20 || token.len() > 200 || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("That doesn't look like a Cloudflare API token. Copy the whole token Cloudflare shows after you click Create Token.".into());
    }
    let cf = Cf { token: token.to_string() };
    cf.verify().await?;
    let account = cf.account().await?;
    let enc = jugg_crypto::seal(&env.ring, token.as_bytes(), TOKEN_AAD).map_err(|e| e.to_string())?;
    ensure_row(env).await?;
    sqlx::query("UPDATE golive SET cf_token_enc = ?, cf_account_id = ?, last_error = NULL, updated_at = ? WHERE id = 'golive'")
        .bind(enc)
        .bind(&account)
        .bind(jugg_core::now_unix())
        .execute(&env.pool)
        .await
        .map_err(|e| e.to_string())?;
    let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), "owner", "golive.cloudflare_connected", &json!({})).await;
    refresh(env).await;
    Ok(view(env).await)
}

pub async fn forget_token(env: &SiteEnv) -> Result<GoLiveView, String> {
    sqlx::query("UPDATE golive SET cf_token_enc = NULL, cf_account_id = NULL, updated_at = ? WHERE id = 'golive'").bind(jugg_core::now_unix()).execute(&env.pool).await.map_err(|e| e.to_string())?;
    let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), "owner", "golive.cloudflare_disconnected", &json!({})).await;
    Ok(view(env).await)
}

pub async fn set_target(env: &SiteEnv, target: &str) -> Result<GoLiveView, String> {
    if !matches!(target, "vps" | "home") {
        return Err("Choose a VPS or this computer.".into());
    }
    ensure_row(env).await?;
    sqlx::query("UPDATE golive SET target = ?, updated_at = ? WHERE id = 'golive'").bind(target).bind(jugg_core::now_unix()).execute(&env.pool).await.map_err(|e| e.to_string())?;
    Ok(view(env).await)
}

/// Look again at everything that can change outside: registration, the Cloudflare
/// zone, the nameservers. Adds the domain to Cloudflare when it isn't there yet.
/// Returns whether anything moved forward.
pub async fn refresh(env: &SiteEnv) -> bool {
    let before = load(env).await;
    let Some(domain) = before.domain.clone() else { return false };
    let (registered, registrar) = rdap(&domain).await;
    let current_ns = public_ns(&domain).await;
    let mut zone_id = before.zone_id.clone();
    let mut zone_status = before.zone_status.clone();
    let mut nameservers = before.nameservers.clone();
    let mut error = None;
    if let Some(tok) = token(env).await {
        let cf = Cf { token: tok };
        let zone = match cf.zone(&domain).await {
            Ok(Some(z)) => Some(z),
            // Only add a registered domain: an unregistered one can't be activated.
            Ok(None) if registered == Some(true) => {
                let account: Option<String> = sqlx::query_scalar("SELECT cf_account_id FROM golive WHERE id = 'golive'").fetch_optional(&env.pool).await.ok().flatten().flatten();
                match account {
                    Some(a) => match cf.add_zone(&a, &domain).await {
                        Ok(z) => {
                            let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), "owner", "golive.zone_added", &json!({ "domain": domain })).await;
                            Some(z)
                        }
                        Err(e) => {
                            error = Some(e);
                            None
                        }
                    },
                    None => None,
                }
            }
            Ok(None) => None,
            Err(e) => {
                error = Some(e);
                None
            }
        };
        if let Some(z) = zone {
            zone_id = z["id"].as_str().map(str::to_string);
            zone_status = z["status"].as_str().map(str::to_string);
            nameservers = z["name_servers"].as_array().map(|a| a.iter().filter_map(|n| n.as_str().map(str::to_string)).collect()).unwrap_or_default();
        }
    }
    let _ = sqlx::query(
        "UPDATE golive SET registered = COALESCE(?, registered), registrar = COALESCE(?, registrar), cf_zone_id = ?, zone_status = ?, nameservers = ?, current_ns = ?, last_error = ?, checked_at = ?, updated_at = ? WHERE id = 'golive'",
    )
    .bind(registered.map(i64::from))
    .bind(registrar)
    .bind(&zone_id)
    .bind(&zone_status)
    .bind(serde_json::to_string(&nameservers).unwrap_or_default())
    .bind(serde_json::to_string(&current_ns).unwrap_or_default())
    .bind(&error)
    .bind(jugg_core::now_unix())
    .bind(jugg_core::now_unix())
    .execute(&env.pool)
    .await;
    let after = load(env).await;
    (before.registered != Some(true) && after.registered == Some(true)) || (before.zone_status.as_deref() != Some("active") && after.zone_status.as_deref() == Some("active"))
}

// ------------------------------------------------------------------ what the owner sees

fn step(key: &str, title: &str, status: &str, detail: String) -> GoLiveStep {
    GoLiveStep { key: key.into(), title: title.into(), status: status.into(), detail }
}

fn steps(r: &Row) -> Vec<GoLiveStep> {
    let mut out = Vec::new();
    let Some(domain) = &r.domain else {
        out.push(step("domain", "Your domain", "active", "Type the domain you have, or the one you'd like to buy.".into()));
        out.push(step("cloudflare", "Connect Cloudflare", "todo", String::new()));
        out.push(step("dns", "Domain on Cloudflare", "todo", String::new()));
        out.push(step("where", "Where it runs", "todo", String::new()));
        return out;
    };
    match r.registered {
        Some(false) => out.push(step("domain", "Your domain", "needs_you", format!("{domain} is available. Buy it on Cloudflare (sold at cost, about $10 a year for .com); I'll notice when it's yours."))),
        Some(true) => out.push(step("domain", "Your domain", "done", match &r.registrar {
            Some(reg) => format!("{domain} · registered with {reg}"),
            None => domain.clone(),
        })),
        None => out.push(step("domain", "Your domain", "done", format!("{domain} · couldn't confirm who it's registered with; carrying on"))),
    }
    let bought = r.registered != Some(false);
    if !r.has_token {
        out.push(step("cloudflare", "Connect Cloudflare", if bought { "needs_you" } else { "todo" }, "Free Cloudflare account and one API token, so I can set up your domain's DNS.".into()));
    } else {
        out.push(step("cloudflare", "Connect Cloudflare", "done", "Connected. The token is stored encrypted on this box.".into()));
    }
    let dns = match (r.has_token, r.zone_status.as_deref()) {
        (false, _) => step("dns", "Domain on Cloudflare", "todo", String::new()),
        (true, Some("active")) => step("dns", "Domain on Cloudflare", "done", format!("{domain} is active on Cloudflare.")),
        (true, Some(_)) if !r.nameservers.is_empty() => {
            let moved = !r.current_ns.is_empty() && r.current_ns.iter().all(|n| r.nameservers.iter().any(|c| c.eq_ignore_ascii_case(n)));
            if moved {
                step("dns", "Domain on Cloudflare", "waiting", "Your nameservers point at Cloudflare. Cloudflare usually confirms within an hour; I'm checking.".into())
            } else {
                step("dns", "Domain on Cloudflare", "needs_you", format!("At {}, replace your nameservers with Cloudflare's two below. It can take a few hours to switch; I'll keep checking.", r.registrar.clone().unwrap_or_else(|| "your registrar".into())))
            }
        }
        (true, _) if !bought => step("dns", "Domain on Cloudflare", "todo", "After you buy the domain.".into()),
        (true, _) => step("dns", "Domain on Cloudflare", "waiting", "Adding your domain to Cloudflare…".into()),
    };
    let dns_done = dns.status == "done";
    out.push(dns);
    out.push(step("where", "Where it runs", if dns_done { "active" } else { "todo" }, match r.target.as_deref() {
        Some("vps") => "A VPS. Setting it up from here arrives in the next update.".into(),
        Some("home") => "This computer, through a Cloudflare Tunnel. Arrives in the next update.".into(),
        _ => "A VPS (rented server, from about €5 a month) or this computer as a home server.".into(),
    }));
    out
}

pub async fn view(env: &SiteEnv) -> GoLiveView {
    let r = load(env).await;
    let steps = steps(&r);
    let next = steps.iter().find(|s| s.status == "needs_you").map(|s| s.title.clone());
    GoLiveView {
        domain: r.domain.clone(),
        registered: r.registered,
        registrar: r.registrar.clone(),
        cloudflare_connected: r.has_token,
        zone_status: r.zone_status.clone(),
        nameservers: r.nameservers.clone(),
        current_ns: r.current_ns.clone(),
        target: r.target.clone(),
        error: r.last_error.clone(),
        checked_at: r.checked_at,
        needs_you: next,
        steps,
        token_url: token_url(),
        sign_up_url: SIGN_UP_URL.into(),
        buy_url: BUY_URL.into(),
    }
}

/// One paragraph for the agent (and for notification emails).
pub fn summary(v: &GoLiveView) -> String {
    let mut s = String::new();
    for st in &v.steps {
        let mark = match st.status.as_str() {
            "done" => "done",
            "needs_you" => "NEEDS THE OWNER",
            "waiting" => "waiting (checked automatically)",
            "active" => "current step",
            _ => "later",
        };
        s.push_str(&format!("- {} [{mark}] {}\n", st.title, st.detail));
    }
    if !v.nameservers.is_empty() && v.zone_status.as_deref() != Some("active") {
        s.push_str(&format!("Cloudflare nameservers to set at the registrar: {}\n", v.nameservers.join(", ")));
    }
    if let Some(e) = &v.error {
        s.push_str(&format!("Last problem: {e}\n"));
    }
    s.push_str("The owner does these on the Go live page (sidebar → Go live). Never ask for the Cloudflare token in chat: it's pasted on that page only.");
    s
}

/// Every few minutes, while something outside is pending, look again; tell the owner
/// when it moves forward.
pub async fn watch(st: crate::server::AppState) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
        let r = load(&st.site).await;
        let pending = r.domain.is_some() && (r.registered == Some(false) || (r.has_token && r.zone_status.as_deref() != Some("active")));
        if !pending {
            continue;
        }
        if refresh(&st.site).await {
            let v = view(&st.site).await;
            let line = match v.steps.iter().find(|s| s.status != "done") {
                Some(s) => format!("Next: {} — {}", s.title, s.detail),
                None => "Everything's done.".into(),
            };
            notify_owner(&st, "Go live moved forward", &format!("{}\n\n{line}\n\nOpen Go live in Juggernaught to continue.", v.steps.iter().filter(|s| s.status == "done").map(|s| format!("✓ {} — {}", s.title, s.detail)).collect::<Vec<_>>().join("\n"))).await;
        }
    }
}

async fn notify_owner(st: &crate::server::AppState, subject: &str, text: &str) {
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), "system", "golive.progress", &json!({ "note": subject })).await;
    let Some(mailer) = &st.mailer else { return };
    let row: Option<(String, Vec<u8>)> = sqlx::query_as("SELECT id, email_enc FROM users WHERE role = 'owner' AND disabled_at IS NULL ORDER BY created_at LIMIT 1").fetch_optional(&st.pool).await.ok().flatten();
    let Some((id, enc)) = row else { return };
    let Some(email) = jugg_crypto::open(&st.ring, &enc, format!("users:email:{id}").as_bytes()).ok().and_then(|b| String::from_utf8(b).ok()) else { return };
    let html = format!("<p>{}</p>", text.replace('&', "&amp;").replace('<', "&lt;").replace('\n', "<br>"));
    let _ = mailer.send(&email, subject, text.to_string(), html).await;
}

// ------------------------------------------------------------------ agent tools

pub struct GoLiveStatus(pub std::sync::Arc<SiteEnv>);
pub struct GoLiveDomain(pub std::sync::Arc<SiteEnv>);

fn spec(name: &str, d: &str, schema: Value, risk: jugg_core::Risk) -> jugg_tools::ToolSpec {
    jugg_tools::ToolSpec { name: name.into(), description: d.into(), input_schema: schema, risk, long_running: false }
}

#[async_trait::async_trait]
impl jugg_tools::Tool for GoLiveStatus {
    fn spec(&self) -> jugg_tools::ToolSpec {
        spec("go_live_status", "Where putting the workspace and website online (Go live) stands: domain, Cloudflare, DNS, hosting. Checks again first. Use it when the owner asks about going live, their domain or hosting, then explain the next step in plain words and send them to the Go live page (sidebar) for anything they must do themselves (accounts, payment, the Cloudflare token).",
            json!({"type":"object","properties":{}}), jugg_core::Risk::Read)
    }
    async fn run(&self, _args: Value, _ctx: &jugg_tools::ToolContext) -> Result<jugg_tools::ToolOutput, jugg_tools::ToolError> {
        refresh(&self.0).await;
        let v = view(&self.0).await;
        Ok(jugg_tools::ToolOutput { summary: summary(&v), files: vec![], data: serde_json::to_value(&v.steps).unwrap_or_default() })
    }
}

#[async_trait::async_trait]
impl jugg_tools::Tool for GoLiveDomain {
    fn spec(&self) -> jugg_tools::ToolSpec {
        spec("go_live_set_domain", "Set the domain the owner wants to go live on (one they own or want to buy), then check whether it's registered and where. Only with a domain the owner gave you.",
            json!({"type":"object","required":["domain"],"properties":{"domain":{"type":"string","description":"e.g. yourbusiness.com"}}}), jugg_core::Risk::Modify)
    }
    async fn run(&self, args: Value, _ctx: &jugg_tools::ToolContext) -> Result<jugg_tools::ToolOutput, jugg_tools::ToolError> {
        let v = set_domain(&self.0, args["domain"].as_str().unwrap_or_default()).await.map_err(jugg_tools::ToolError::Args)?;
        Ok(jugg_tools::ToolOutput { summary: summary(&v), files: vec![], data: serde_json::to_value(&v.steps).unwrap_or_default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_normalised_and_checked() {
        assert_eq!(normalise_domain(" https://www.Northwind-Climbing.co.uk/about ").unwrap(), "northwind-climbing.co.uk");
        assert_eq!(normalise_domain("example.com.").unwrap(), "example.com");
        for bad in ["", "localhost", "exa mple.com", "-bad.com", "1.2.3.4", "a/../b.com", "ex_ample.com", "example.com:8080"] {
            assert!(normalise_domain(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn token_link_preselects_permissions() {
        let u = token_url();
        assert!(u.starts_with("https://dash.cloudflare.com/profile/api-tokens?permissionGroupKeys="));
        let q = url::form_urlencoded::parse(u.split_once('?').unwrap().1.as_bytes()).collect::<std::collections::HashMap<_, _>>();
        let perms: Value = serde_json::from_str(&q["permissionGroupKeys"]).unwrap();
        let keys: Vec<&str> = perms.as_array().unwrap().iter().map(|p| p["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["zone", "dns", "account_settings"]);
        assert_eq!(q["accountId"], "*");
        assert_eq!(q["zoneId"], "all");
    }

    #[test]
    fn registrar_from_rdap() {
        let v = json!({"entities":[{"roles":["registrar"],"vcardArray":["vcard",[["version",{},"text","4.0"],["fn",{},"text","Namecheap, Inc."]]]}]});
        assert_eq!(registrar_name(&v).as_deref(), Some("Namecheap, Inc."));
        assert_eq!(registrar_name(&json!({})), None);
    }

    #[test]
    fn steps_follow_the_owner() {
        let base = Row { domain: Some("example.com".into()), registered: Some(true), ..Default::default() };
        let s = steps(&base);
        assert_eq!(s[1].status, "needs_you", "connect Cloudflare next");
        let buying = Row { registered: Some(false), ..base.clone() };
        assert_eq!(steps(&buying)[0].status, "needs_you");
        let pending = Row { has_token: true, zone_status: Some("pending".into()), nameservers: vec!["ada.ns.cloudflare.com".into(), "bob.ns.cloudflare.com".into()], current_ns: vec!["ns1.registrar.com".into()], ..base.clone() };
        assert_eq!(steps(&pending)[2].status, "needs_you", "change nameservers");
        let moved = Row { current_ns: vec!["ada.ns.cloudflare.com".into(), "bob.ns.cloudflare.com".into()], ..pending.clone() };
        assert_eq!(steps(&moved)[2].status, "waiting");
        let active = Row { zone_status: Some("active".into()), ..moved };
        let s = steps(&active);
        assert_eq!((s[2].status.as_str(), s[3].status.as_str()), ("done", "active"));
    }
}
