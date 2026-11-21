//! Server functions: the only door between the UI and the platform.
//! Every function re-checks the session on the server (dev mode excepted).

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Overview {
    pub business: String,
    pub setup_complete: bool,
    pub provider: String,
    pub provider_ready: bool,
    pub suggestions: Vec<String>,
    pub greeting_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolActivity {
    pub name: String,
    pub summary: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reply {
    pub conversation_id: String,
    pub text: String,
    pub tools: Vec<ToolActivity>,
    pub approvals: Vec<String>,
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
}

#[cfg(feature = "ssr")]
fn state() -> Result<crate::server::AppState, ServerFnError> {
    use_context::<crate::server::AppState>().ok_or_else(|| ServerFnError::new("app state missing"))
}

#[cfg(feature = "ssr")]
async fn require_owner(st: &crate::server::AppState) -> Result<(), ServerFnError> {
    if st.cfg.dev {
        return Ok(());
    }
    let parts: http::request::Parts = leptos_axum::extract().await?;
    let token = parts
        .headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|c| c.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(&format!("{}=", jugg_auth::session::COOKIE)).map(str::to_string))
        .ok_or_else(|| ServerFnError::new("sign in required"))?;
    jugg_auth::session::validate(&st.pool, &token).await.map_err(|_| ServerFnError::new("sign in required"))?;
    Ok(())
}

#[server]
pub async fn get_overview() -> Result<Overview, ServerFnError> {
    let st = state()?;
    let owners: i64 = jugg_db::sqlx::query_scalar("SELECT count(*) FROM users WHERE role='owner'").fetch_one(&st.pool).await?;
    let brand = st.brand.read().await.clone();
    let setup_complete = owners > 0;
    let provider_ready = st.provider.health().await.is_ok();
    Ok(Overview {
        business: brand.name.clone(),
        setup_complete,
        provider: st.provider.label(),
        provider_ready,
        suggestions: crate::server::suggestions(&st, setup_complete, provider_ready).await,
        greeting_name: None,
    })
}

#[server]
pub async fn send_message(conversation_id: Option<String>, text: String) -> Result<Reply, ServerFnError> {
    let st = state()?;
    require_owner(&st).await?;
    let text = text.trim().to_string();
    if text.is_empty() || text.len() > 20_000 {
        return Err(ServerFnError::new("message must be 1–20000 characters"));
    }
    crate::server::chat(&st, conversation_id, text).await.map_err(ServerFnError::new)
}

#[server]
pub async fn complete_setup(input: WizardInput) -> Result<(), ServerFnError> {
    let st = state()?;
    crate::server::complete_setup(&st, input).await.map_err(ServerFnError::new)
}

#[server]
pub async fn request_magic_link(email: String) -> Result<(), ServerFnError> {
    let st = state()?;
    crate::server::request_magic_link(&st, &email).await;
    // Same answer whether or not the address exists.
    Ok(())
}

#[server]
pub async fn get_brand() -> Result<BrandView, ServerFnError> {
    let st = state()?;
    require_owner(&st).await?;
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
    let st = state()?;
    require_owner(&st).await?;
    Ok(crate::server::security_view(&st))
}

#[server]
pub async fn get_standards() -> Result<StandardsView, ServerFnError> {
    Ok(StandardsView {
        rules: jugg_web::RULES
            .iter()
            .map(|r| (r.id.to_string(), format!("{:?}", r.level).to_lowercase(), r.chat_overridable, r.why.to_string()))
            .collect(),
    })
}

#[server]
pub async fn get_tools() -> Result<Vec<ToolView>, ServerFnError> {
    let st = state()?;
    Ok(st
        .registry
        .specs()
        .into_iter()
        .map(|s| ToolView { name: s.name, description: s.description, risk: format!("{:?}", s.risk).to_lowercase() })
        .collect())
}
