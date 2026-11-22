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
        _ if host == "site.localhost" => Target::Public(path),
        _ => match path.strip_prefix("/_site") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => Target::Public(if rest.is_empty() { "/".into() } else { rest.into() }),
            _ => Target::Admin,
        },
    }
}

pub async fn host_router(State(st): State<AppState>, axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>, req: Request, next: Next) -> Response {
    match classify(&st, &req) {
        Target::Admin => next.run(req).await,
        Target::WwwRedirect(to) => Redirect::permanent(&to).into_response(),
        Target::Public(path) => public(&st, &path, &crate::server::client_ip(&addr), req).await,
    }
}

async fn public(st: &AppState, path: &str, ip: &str, req: Request) -> Response {
    if *req.method() == axum::http::Method::POST {
        if let Some(name) = path.strip_prefix("/forms/").filter(|n| !n.is_empty() && n.len() < 60 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
            return harden(crate::site::handle_form(st, name, ip, req).await);
        }
        return harden(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    if !matches!(*req.method(), axum::http::Method::GET | axum::http::Method::HEAD) {
        return harden(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let site = st.cfg.site_dir();
    if path == "/forms.json" {
        return harden(StatusCode::NOT_FOUND.into_response());
    }
    let res = if site.join("index.html").exists() {
        let mut sub = Request::new(Body::empty());
        *sub.uri_mut() = path.parse().unwrap_or_else(|_| "/".parse().expect("static uri"));
        let not_found = tower_http::services::ServeFile::new(site.join("404.html"));
        match tower_http::services::ServeDir::new(&site).append_index_html_on_directories(true).not_found_service(not_found).oneshot(sub).await {
            Ok(r) => {
                let mut r = r.map(Body::new).into_response();
                // Hashed CSS, media and fonts never change under the same name.
                let immutable = path.starts_with("/media/") || path.starts_with("/fonts/") || (path.starts_with("/site.") && path.ends_with(".css"));
                let cc = if immutable { "public, max-age=31536000, immutable" } else { "public, max-age=0, must-revalidate" };
                r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
                r
            }
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
                nav: None,
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
