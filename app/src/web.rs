//! Host routing and the public website.
//!
//! * `admin.<domain>` → the admin app (everything else in the router).
//! * `<domain>` → the published static site from `data_dir/site/current`, or a
//!   brand-rendered "coming soon" page until the first deploy.
//! * `www.<domain>` → permanent redirect to the apex.
//!
//! In development (no domain) the public site is previewed under `/_site/`.

use crate::server::AppState;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use tower::ServiceExt;

enum Target {
    Admin,
    Public(String),
    WwwRedirect(String),
}

fn classify(st: &AppState, req: &Request) -> Target {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").split(':').next().unwrap_or("").to_ascii_lowercase();
    let path = req.uri().path().to_string();
    match &st.cfg.domain {
        Some(d) if !st.cfg.dev => {
            if host == *d {
                Target::Public(path)
            } else if host == format!("www.{d}") {
                Target::WwwRedirect(format!("https://{d}{}", req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/")))
            } else {
                Target::Admin
            }
        }
        _ => match path.strip_prefix("/_site") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => Target::Public(if rest.is_empty() { "/".into() } else { rest.into() }),
            _ => Target::Admin,
        },
    }
}

pub async fn host_router(State(st): State<AppState>, req: Request, next: Next) -> Response {
    match classify(&st, &req) {
        Target::Admin => next.run(req).await,
        Target::WwwRedirect(to) => Redirect::permanent(&to).into_response(),
        Target::Public(path) => public(&st, &path, req).await,
    }
}

async fn public(st: &AppState, path: &str, req: Request) -> Response {
    if !matches!(*req.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        // Forms land in phase 4; nothing else is writable on the public site.
        return harden(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let site = st.cfg.site_dir();
    let res = if site.join("index.html").exists() {
        let mut sub = Request::new(Body::empty());
        *sub.uri_mut() = path.parse().unwrap_or_else(|_| "/".parse().expect("static uri"));
        match tower_http::services::ServeDir::new(&site).append_index_html_on_directories(true).oneshot(sub).await {
            Ok(r) => r.map(Body::new).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    } else {
        placeholder(st, path).await
    };
    harden(res)
}

/// Until the first deploy: a calm, on-brand holding page.
async fn placeholder(st: &AppState, path: &str) -> Response {
    let brand = st.brand.read().await.clone();
    match path {
        "/site.css" => ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], jugg_web::site_css(&brand)).into_response(),
        "/" => {
            let css = if st.cfg.dev && st.cfg.domain.is_none() { "/_site/site.css" } else { "/site.css" };
            let page = jugg_web::Page {
                slug: "/".into(),
                title: format!("{} — coming soon", brand.name),
                description: brand.tagline.clone().unwrap_or_else(|| format!("{} is getting ready.", brand.name)),
                lang: None,
                blocks: vec![jugg_web::Block::Hero {
                    eyebrow: Some("Coming soon".into()),
                    heading: brand.name.clone(),
                    body: brand.tagline.clone(),
                    cta: None,
                    secondary: None,
                    image: None,
                    tone: jugg_web::Tone::Dark,
                }],
            };
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], jugg_web::render_page(&page, &brand, css)).into_response()
        }
        "/robots.txt" => ([(header::CONTENT_TYPE, "text/plain")], format!("User-agent: *\nAllow: /\n{}", jugg_steward::honeytokens::ROBOTS_BAIT)).into_response(),
        _ => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// Public responses carry the strict public CSP (the security layer leaves it alone).
fn harden(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(jugg_web::Csp::PUBLIC));
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    res
}

/// :80 in production: everything moves to HTTPS.
pub async fn redirect_to_https(req: Request) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").split(':').next().unwrap_or("").to_string();
    if host.is_empty() || host.len() > 253 || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let pq = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
    Redirect::permanent(&format!("https://{host}{pq}")).into_response()
}
