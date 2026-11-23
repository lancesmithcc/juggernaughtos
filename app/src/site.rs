//! Website builder: pages in SQLite, immutable releases on disk, atomic switch.
//!
//! ```text
//! site_save_page ─▶ site_pages (draft) ─▶ /_preview/… (owner only)
//! site_publish (owner approves) ─▶ build: validate + media + fonts
//!     ─▶ data/site/releases/<deploy>/  ─▶ symlink data/site/current  (instant, atomic)
//! site_rollback ─▶ point `current` at the previous release
//! ```

use crate::server::AppState;
use async_trait::async_trait;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use jugg_brand::BrandStandards;
use jugg_core::{Config, Risk};
use jugg_crypto::{Keyring, Signer};
use jugg_db::sqlx::{self, SqlitePool};
use jugg_tools::{Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use jugg_web::{Block, FontFace, Image, Override, Page, SiteContext};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// What the site tools need (a subset of AppState, so tools can live inside it).
#[derive(Clone)]
pub struct SiteEnv {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub brand: Arc<RwLock<BrandStandards>>,
    pub signer: Arc<Signer>,
    pub ring: Arc<Keyring>,
}

const KEEP_RELEASES: usize = 5;

impl SiteEnv {
    fn site_root(&self) -> PathBuf {
        self.cfg.data_dir.join("site")
    }

    pub fn public_url(&self) -> String {
        match (&self.cfg.domain, self.cfg.dev) {
            (Some(d), false) => format!("https://{d}"),
            _ => format!("http://site.localhost:{}", self.cfg.listen_http.rsplit(':').next().unwrap_or("3700")),
        }
    }

    pub async fn pages(&self) -> Vec<(Page, String, i64)> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as("SELECT blocks_json, status, updated_at FROM site_pages WHERE status != 'archived' ORDER BY slug").fetch_all(&self.pool).await.unwrap_or_default();
        rows.into_iter().filter_map(|(j, s, at)| serde_json::from_str::<Page>(&j).ok().map(|p| (p, s, at))).collect()
    }

    pub async fn save_page(&self, page: Page) -> Result<jugg_web::Report, String> {
        if !jugg_web::valid_slug(&page.slug) {
            return Err(format!("“{}” isn't a valid page address. Use / for the homepage or /about, /services/web …", page.slug));
        }
        let overrides = self.overrides().await;
        let mut ov = overrides.get(&page.slug).cloned().unwrap_or_default();
        ov.extend(overrides.get("*").cloned().unwrap_or_default());
        let report = jugg_web::validate_page(&page, &ov);
        let json = serde_json::to_string(&page).map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO site_pages (id, slug, title, blocks_json, status, updated_at) VALUES (?,?,?,?,'draft',?)
             ON CONFLICT(slug) DO UPDATE SET title = excluded.title, blocks_json = excluded.blocks_json, status = 'draft', updated_at = excluded.updated_at",
        )
        .bind(jugg_core::new_id())
        .bind(&page.slug)
        .bind(&page.title)
        .bind(json)
        .bind(jugg_core::now_unix())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        let _ = jugg_db::audit::append(&self.pool, Some(&self.signer), "agent", "site.page_saved", &json!({ "slug": page.slug })).await;
        Ok(report)
    }

    pub async fn overrides(&self) -> HashMap<String, Vec<Override>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as("SELECT rule, target, reason FROM standards_overrides WHERE revoked_at IS NULL AND rule != 'brand.gate'").fetch_all(&self.pool).await.unwrap_or_default();
        let mut m: HashMap<String, Vec<Override>> = HashMap::new();
        for (rule, target, reason) in rows {
            m.entry(target).or_default().push(Override { rule, reason });
        }
        m
    }

    /// Fonts for the brand: bundled Plus Jakarta Sans, others from the Google Fonts repo (cached).
    pub(crate) async fn fonts(&self, brand: &BrandStandards) -> (Vec<FontFace>, Vec<(String, Vec<u8>)>) {
        let mut faces = Vec::new();
        let mut files = Vec::new();
        for family in brand.font_families() {
            let found = if family == "Plus Jakarta Sans" {
                bundled_jakarta().map(|b| ("PlusJakartaSans-Variable.ttf".to_string(), b, "200 800".to_string()))
            } else {
                let f = google_font(&self.cfg.data_dir.join("fonts"), family).await;
                if f.is_some() {
                    jugg_print::load_fonts(&self.cfg.data_dir.join("fonts"));
                }
                f
            };
            if let Some((name, bytes, weight)) = found {
                faces.push(FontFace { family: family.to_string(), url: format!("/fonts/{name}"), weight, format: "truetype".into() });
                files.push((format!("fonts/{name}"), bytes));
            } else {
                tracing::warn!(family, "font not available; the site falls back to system fonts");
            }
        }
        (faces, files)
    }

    /// Resolve `asset:<id>` images: approved assets only, resized and re-encoded.
    async fn media(&self, pages: &mut [Page]) -> (Vec<(String, Vec<u8>)>, HashMap<String, Vec<String>>) {
        let mut files = Vec::new();
        let mut problems: HashMap<String, Vec<String>> = HashMap::new();
        let mut cache: HashMap<String, Image> = HashMap::new();
        for page in pages.iter_mut() {
            let slug = page.slug.clone();
            for img in images_mut(page) {
                let Some(id) = img.src.strip_prefix("asset:").map(str::to_string) else { continue };
                if let Some(done) = cache.get(&id) {
                    img.src = done.src.clone();
                    img.width = done.width;
                    img.height = done.height;
                    img.bytes = done.bytes;
                    continue;
                }
                let row: Option<(String, String)> = sqlx::query_as("SELECT storage_path, status FROM assets WHERE id = ?").bind(&id).fetch_optional(&self.pool).await.ok().flatten();
                let Some((stored, status)) = row else {
                    problems.entry(slug.clone()).or_default().push(format!("image {id} doesn't exist"));
                    continue;
                };
                if status != "final" {
                    problems.entry(slug.clone()).or_default().push(format!("image {id} isn't approved yet ({status})"));
                    continue;
                }
                let Ok(bytes) = std::fs::read(self.cfg.assets_dir().join(&stored)) else { continue };
                match tokio::task::spawn_blocking(move || optimise(&stored, &bytes)).await {
                    Ok(Some((name, out, w, h))) => {
                        img.src = format!("/media/{name}");
                        img.width = w;
                        img.height = h;
                        img.bytes = Some(out.len() as u64);
                        cache.insert(id, img.clone());
                        files.push((format!("media/{name}"), out));
                    }
                    _ => problems.entry(slug.clone()).or_default().push(format!("image {id} couldn't be processed")),
                }
            }
        }
        (files, problems)
    }

    pub async fn build(&self) -> Result<(jugg_web::Built, Vec<(String, Vec<u8>)>), String> {
        let brand = self.brand.read().await.clone();
        let mut pages: Vec<Page> = self.pages().await.into_iter().map(|(p, _, _)| p).collect();
        if pages.is_empty() {
            return Err("There are no pages yet. Ask me to build your homepage first.".into());
        }
        let (mut extra, problems) = self.media(&mut pages).await;
        let (faces, font_files) = self.fonts(&brand).await;
        extra.extend(font_files);
        let overrides = self.overrides().await;
        let base = self.cfg.domain.as_ref().filter(|_| !self.cfg.dev).map(|d| format!("https://{d}"));
        let mut built = jugg_web::build_site(&pages, &brand, &faces, &overrides, base.as_deref()).map_err(|e| e.to_string())?;
        // Adopted logo: header image + favicon from the icon.
        if let Some(svg) = crate::studio::logo_file(self, &brand.logo.primary_asset).await {
            let attr = |n: &str| svg[..svg.find('>').unwrap_or(0)].split(&format!(" {n}=\"")).nth(1).and_then(|v| v.split('"').next()).and_then(|v| v.parse::<f32>().ok());
            let (w, h) = (attr("width").unwrap_or(300.0), attr("height").unwrap_or(80.0));
            let name = format!("media/logo-{}.svg", &jugg_crypto::sha256_hex(svg.as_bytes())[..10]);
            let img = format!("<a class=\"brand\" href=\"/\"><img src=\"/{name}\" alt=\"{alt}\" width=\"{iw}\" height=\"36\"></a>", alt = brand.name.replace('"', "&quot;").replace('<', "&lt;"), iw = (w / h * 36.0).round());
            let plain = format!("<a class=\"brand\" href=\"/\">{}</a>", brand.name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;"));
            for (path, bytes) in built.files.iter_mut() {
                if path.ends_with(".html") {
                    *bytes = String::from_utf8_lossy(bytes).replacen(&plain, &img, 1).into_bytes();
                }
            }
            extra.push((name, svg.into_bytes()));
        }
        if let Some(icon) = crate::studio::logo_file(self, &brand.logo.mark_asset).await {
            if let Some(f) = built.files.iter_mut().find(|(p, _)| p == "favicon.svg") {
                f.1 = icon.into_bytes();
            }
        }
        for (slug, report) in built.reports.iter_mut() {
            let mut ov = overrides.get(slug.as_str()).cloned().unwrap_or_default();
            ov.extend(overrides.get("*").cloned().unwrap_or_default());
            for p in problems.get(slug.as_str()).into_iter().flatten() {
                report.violations.push(jugg_web::Violation { rule: "site.unapproved_media".into(), level: jugg_web::rule_level("site.unapproved_media"), message: p.clone(), overridden: ov.iter().any(|o| o.rule == "site.unapproved_media") });
            }
        }
        Ok((built, extra))
    }

    /// Build, validate and switch the live site. Only runs after owner approval.
    pub async fn publish(&self, by: &str) -> Result<String, String> {
        self.publish_with(by, false).await
    }

    /// `anyway`: the owner chose to publish despite standards that allow it; each
    /// broken rule is recorded as their override. Security rules still block.
    pub async fn publish_with(&self, by: &str, anyway: bool) -> Result<String, String> {
        let (mut built, mut extra) = self.build().await?;
        if !built.publishable() && anyway {
            let mut recorded = 0;
            for (slug, r) in &built.reports {
                for v in r.violations.iter().filter(|v| v.level == jugg_web::Level::Error && !v.overridden && rule_info(&v.rule).0) {
                    sqlx::query("INSERT INTO standards_overrides (id, rule, target, reason, user_id, created_at) VALUES (?,?,?,?,?,?)")
                        .bind(jugg_core::new_id())
                        .bind(&v.rule)
                        .bind(slug)
                        .bind("Owner chose to publish anyway")
                        .bind(by)
                        .bind(jugg_core::now_unix())
                        .execute(&self.pool)
                        .await
                        .map_err(|e| e.to_string())?;
                    recorded += 1;
                }
            }
            if recorded > 0 {
                let _ = jugg_db::audit::append(&self.pool, Some(&self.signer), by, "site.published_anyway", &json!({ "overrides": recorded })).await;
                (built, extra) = self.build().await?;
            }
        }
        if !built.publishable() {
            let mut lines = Vec::new();
            let mut all_overridable = true;
            for (slug, r) in &built.reports {
                for v in r.violations.iter().filter(|v| v.level == jugg_web::Level::Error && !v.overridden) {
                    let (overridable, why) = rule_info(&v.rule);
                    all_overridable &= overridable;
                    let page = if slug == "/" { "Homepage".to_string() } else { slug.clone() };
                    lines.push(format!("• {page}: {}. {why}", v.message));
                }
            }
            let next = if all_overridable {
                "Ask me in chat to fix it, or press Publish anyway to go live as it is (I'll note that you chose to)."
            } else {
                "Ask me in chat to fix it; security standards can't be skipped."
            };
            return Err(format!("Not published yet. {} to sort out:\n{}\n\n{next}", if lines.len() == 1 { "One thing".to_string() } else { format!("{} things", lines.len()) }, lines.join("\n")));
        }
        let id = jugg_core::new_id();
        let releases = self.site_root().join("releases");
        let dir = releases.join(&id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut hasher = Vec::new();
        for (path, bytes) in built.files.iter().chain(extra.iter()) {
            if path.contains("..") || path.starts_with('/') {
                return Err(format!("refusing unsafe path {path}"));
            }
            let full = dir.join(path);
            if let Some(p) = full.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            std::fs::write(&full, bytes).map_err(|e| e.to_string())?;
            hasher.extend_from_slice(path.as_bytes());
            hasher.extend_from_slice(jugg_crypto::sha256_hex(bytes).as_bytes());
        }
        swap_current(&self.site_root(), &id).map_err(|e| e.to_string())?;
        let report = json!({ "pages": built.reports.iter().map(|(s, r)| json!({ "slug": s, "violations": r.violations })).collect::<Vec<_>>() });
        sqlx::query("UPDATE site_deploys SET status = 'superseded' WHERE status = 'live'").execute(&self.pool).await.map_err(|e| e.to_string())?;
        sqlx::query("INSERT INTO site_deploys (id, status, report_json, content_sha256, created_by, created_at, live_at) VALUES (?,'live',?,?,?,?,?)")
            .bind(&id)
            .bind(report.to_string())
            .bind(jugg_crypto::sha256_hex(&hasher))
            .bind(Some(by).filter(|b| b.len() == 36))
            .bind(jugg_core::now_unix())
            .bind(jugg_core::now_unix())
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("UPDATE site_pages SET status = 'published' WHERE status = 'draft'").execute(&self.pool).await.map_err(|e| e.to_string())?;
        prune_releases(&releases, &id);
        let _ = jugg_db::audit::append(&self.pool, Some(&self.signer), by, "site.published", &json!({ "deploy": id, "pages": built.reports.len() })).await;
        let warnings: usize = built.reports.iter().map(|(_, r)| r.warnings().count()).sum();
        Ok(format!("The website is live at {} ({} page(s){}).", self.public_url(), built.reports.len(), if warnings > 0 { format!(", {warnings} note(s) to look at") } else { String::new() }))
    }

    pub async fn rollback(&self, by: &str) -> Result<String, String> {
        let prev: Option<String> = sqlx::query_scalar("SELECT id FROM site_deploys WHERE status = 'superseded' ORDER BY live_at DESC LIMIT 1").fetch_optional(&self.pool).await.map_err(|e| e.to_string())?;
        let Some(prev) = prev else { return Err("There's no earlier version to go back to.".into()) };
        if !self.site_root().join("releases").join(&prev).exists() {
            return Err("The earlier version was cleaned up and can't be restored.".into());
        }
        swap_current(&self.site_root(), &prev).map_err(|e| e.to_string())?;
        sqlx::query("UPDATE site_deploys SET status = 'rolled_back' WHERE status = 'live'").execute(&self.pool).await.map_err(|e| e.to_string())?;
        sqlx::query("UPDATE site_deploys SET status = 'live', live_at = ? WHERE id = ?").bind(jugg_core::now_unix()).bind(&prev).execute(&self.pool).await.map_err(|e| e.to_string())?;
        let _ = jugg_db::audit::append(&self.pool, Some(&self.signer), by, "site.rolled_back", &json!({ "to": prev })).await;
        Ok(format!("Rolled back. {} is showing the previous version again.", self.public_url()))
    }
}

fn images_mut(page: &mut Page) -> Vec<&mut Image> {
    let mut out = Vec::new();
    for b in page.blocks.iter_mut() {
        match b {
            Block::Hero { image: Some(i), .. } => out.push(i),
            Block::Gallery { images, .. } => out.extend(images.iter_mut()),
            Block::Cards { cards, .. } => out.extend(cards.iter_mut().filter_map(|c| c.image.as_mut())),
            _ => {}
        }
    }
    out
}

/// ≤1600 px wide; photos → JPEG q82, graphics with transparency → PNG, SVG as is.
fn optimise(stored: &str, bytes: &[u8]) -> Option<(String, Vec<u8>, u32, u32)> {
    let stem: String = stored.chars().take(16).collect();
    if stored.ends_with(".svg") {
        let s = String::from_utf8_lossy(bytes);
        let num = |attr: &str| -> Option<u32> {
            let i = s.find(&format!("{attr}=\""))? + attr.len() + 2;
            s[i..].split(|c: char| !c.is_ascii_digit() && c != '.').next()?.parse::<f32>().ok().map(|f| f as u32)
        };
        let (w, h) = (num("width").unwrap_or(800), num("height").unwrap_or(600));
        return Some((format!("{stem}.svg"), bytes.to_vec(), w.max(1), h.max(1)));
    }
    let original = image::load_from_memory(bytes).ok()?;
    let has_alpha = original.color().has_alpha() && original.to_rgba8().pixels().any(|p| p[3] < 255);
    // Step down quality, then size, until the image fits the page budget, so a
    // publish never stops on a heavy photo. The smallest try is kept regardless.
    let mut best: Option<(String, Vec<u8>, u32, u32)> = None;
    'sizes: for max_w in [1600u32, 1400, 1200, 1000, 800] {
        let img = if original.width() > max_w { original.resize(max_w, u32::MAX, image::imageops::FilterType::Lanczos3) } else { original.clone() };
        let (w, h) = (img.width(), img.height());
        let tries: &[u8] = if has_alpha { &[0] } else { &[82, 74, 66] };
        for &q in tries {
            let mut out = std::io::Cursor::new(Vec::new());
            let name = if has_alpha {
                img.write_to(&mut out, image::ImageFormat::Png).ok()?;
                format!("{stem}-{w}.png")
            } else {
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q).encode_image(&img.to_rgb8()).ok()?;
                format!("{stem}-{w}.jpg")
            };
            let out = out.into_inner();
            let fits = (out.len() as u64) <= IMAGE_TARGET;
            best = Some((name, out, w, h));
            if fits {
                break 'sizes;
            }
        }
        if original.width() <= max_w && max_w < 1000 {
            break;
        }
    }
    best
}

/// Whether the owner may override a web standard, and why it exists.
fn rule_info(id: &str) -> (bool, &'static str) {
    jugg_web::RULES.iter().find(|r| r.id == id).map(|r| (r.chat_overridable, r.why)).unwrap_or((false, ""))
}

/// Comfortably under the web standards' 300 KB per-image limit.
const IMAGE_TARGET: u64 = 280 * 1024;

#[cfg(test)]
mod optimise_tests {
    #[test]
    fn heavy_photos_fit_the_budget() {
        // Noise is the worst case for JPEG: it must still come in under budget.
        let mut seed = 1u32;
        let img = image::RgbImage::from_fn(2400, 1600, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8])
        });
        let mut raw = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut raw, image::ImageFormat::Png).unwrap();
        let (name, out, w, _) = super::optimise("abcdef0123456789.png", &raw.into_inner()).unwrap();
        assert!(name.ends_with(".jpg"), "{name}");
        assert!(w <= 1600);
        assert!(out.len() as u64 <= super::IMAGE_TARGET, "{} KB", out.len() / 1024);
    }
}

fn bundled_jakarta() -> Option<Vec<u8>> {
    let root = std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".into());
    [PathBuf::from(&root).join("fonts/PlusJakartaSans-Variable.ttf"), PathBuf::from("public/fonts/PlusJakartaSans-Variable.ttf")].iter().find_map(|p| std::fs::read(p).ok())
}

/// Fetch a family from the google/fonts repo once; cached with its licence.
async fn google_font(cache: &FsPath, family: &str) -> Option<(String, Vec<u8>, String)> {
    let dir_name: String = family.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if dir_name.is_empty() {
        return None;
    }
    let local = cache.join(&dir_name);
    if let Some(found) = std::fs::read_dir(&local).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "ttf"))) {
        let name = found.file_name()?.to_string_lossy().to_string();
        let weight = if name.contains("wght") { "100 900" } else { "400" };
        return Some((public_font_name(&name), std::fs::read(&found).ok()?, weight.into()));
    }
    let http = reqwest::Client::builder().user_agent("juggernaught-os").timeout(std::time::Duration::from_secs(30)).build().ok()?;
    for license in ["ofl", "apache", "ufl"] {
        let list: Value = match http.get(format!("https://api.github.com/repos/google/fonts/contents/{license}/{dir_name}")).send().await {
            Ok(r) if r.status().is_success() => r.json().await.ok()?,
            _ => continue,
        };
        let files = list.as_array()?;
        let pick = files
            .iter()
            .filter(|f| f["name"].as_str().is_some_and(|n| n.ends_with(".ttf") && !n.to_lowercase().contains("italic")))
            .min_by_key(|f| f["name"].as_str().map(|n| n.len()).unwrap_or(usize::MAX))?;
        let name = pick["name"].as_str()?.to_string();
        let url = pick["download_url"].as_str()?;
        if !url.starts_with("https://raw.githubusercontent.com/google/fonts/") {
            return None;
        }
        let bytes = http.get(url).send().await.ok()?.bytes().await.ok()?.to_vec();
        std::fs::create_dir_all(&local).ok()?;
        std::fs::write(local.join(&name), &bytes).ok()?;
        if let Some(lic) = files.iter().find(|f| f["name"].as_str().is_some_and(|n| n == "OFL.txt" || n == "LICENSE.txt")).and_then(|f| f["download_url"].as_str()) {
            if let Ok(r) = http.get(lic).send().await {
                if let Ok(t) = r.bytes().await {
                    let _ = std::fs::write(local.join("LICENSE.txt"), t);
                }
            }
        }
        let weight = if name.contains("wght") { "100 900" } else { "400" };
        return Some((public_font_name(&name), bytes, weight.into()));
    }
    None
}

fn public_font_name(name: &str) -> String {
    name.replace(['[', ']', ','], "-").replace("--", "-").replace("-.ttf", ".ttf")
}

fn swap_current(root: &FsPath, release: &str) -> std::io::Result<()> {
    let tmp = root.join(".current.tmp");
    let _ = std::fs::remove_file(&tmp);
    #[cfg(unix)]
    std::os::unix::fs::symlink(PathBuf::from("releases").join(release), &tmp)?;
    std::fs::rename(&tmp, root.join("current"))
}

fn prune_releases(releases: &FsPath, keep_id: &str) {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(releases).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    dirs.sort();
    let excess = dirs.len().saturating_sub(KEEP_RELEASES);
    for d in dirs.into_iter().take(excess) {
        if d.file_name().is_some_and(|n| n != keep_id) {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

// ------------------------------------------------------------------ agent tools

const PAGE_HELP: &str = "A page is JSON: {\"slug\":\"/\" or \"/about\", \"title\":\"10–60 chars\", \"description\":\"50–160 chars\", \"nav\":\"Menu label or null\", \
\"blocks\":[…]}. Blocks (each has \"type\" and \"tone\": light|dark|accent; alternate tones): \
hero {eyebrow?, heading, body?, cta?:{label,href}, secondary?:{label,href}, image?:{src,alt,width,height}} — exactly one per page; \
section {eyebrow?, heading, body} — body paragraphs separated by blank lines; \
cards {eyebrow?, heading, cards:[{heading, body, link?, image?}]}; \
gallery {heading?, images:[…]}; cta {heading, body?, cta:{label,href}}; \
contact {heading, form:{name, submit_label, fields:[{name,label,kind:text|email|tel|textarea,required}]}}. \
Images use src \"asset:<asset id>\" of an APPROVED asset (the platform resizes them). Links: https:, mailto:, tel:, /page or #anchor. \
No raw HTML, CSS or JavaScript.";

pub struct SitePages(pub Arc<SiteEnv>);
pub struct SiteSavePage(pub Arc<SiteEnv>);
pub struct SitePublish(pub Arc<SiteEnv>);
pub struct SiteRollback(pub Arc<SiteEnv>);
pub struct SiteOverride(pub Arc<SiteEnv>);

fn spec(name: &str, description: String, input_schema: Value, risk: Risk) -> ToolSpec {
    ToolSpec { name: name.into(), description, input_schema, risk, long_running: false }
}

#[async_trait]
impl Tool for SitePages {
    fn spec(&self) -> ToolSpec {
        spec("site_pages", "List the website's pages, or get one page's full JSON (pass slug).".into(), json!({"type":"object","properties":{"slug":{"type":"string"}}}), Risk::Read)
    }
    async fn run(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let pages = self.0.pages().await;
        if let Some(slug) = args["slug"].as_str() {
            let p = pages.into_iter().find(|(p, _, _)| p.slug == slug).ok_or_else(|| ToolError::Args(format!("no page {slug}")))?;
            return Ok(ToolOutput { summary: format!("{} ({})", p.0.title, p.1), files: vec![], data: serde_json::to_value(&p.0).unwrap_or_default() });
        }
        let list: Vec<Value> = pages.iter().map(|(p, s, _)| json!({ "slug": p.slug, "title": p.title, "status": s, "nav": p.nav })).collect();
        let live: Option<String> = sqlx::query_scalar("SELECT id FROM site_deploys WHERE status = 'live'").fetch_optional(&self.0.pool).await.ok().flatten();
        Ok(ToolOutput { summary: format!("{} page(s); site {}", list.len(), if live.is_some() { format!("live at {}", self.0.public_url()) } else { "not published yet".into() }), files: vec![], data: json!(list) })
    }
}

#[async_trait]
impl Tool for SiteSavePage {
    fn spec(&self) -> ToolSpec {
        spec("site_save_page", format!("Create or replace a website page (saved as a draft; the owner previews it, then publishing makes it live). {PAGE_HELP}"),
            json!({"type":"object","required":["page"],"properties":{"page":{"type":"object","description":"the page JSON described above"}}}), Risk::Modify)
    }
    async fn run(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let page: Page = serde_json::from_value(args["page"].clone()).map_err(|e| ToolError::Args(format!("page JSON doesn't match the block schema: {e}")))?;
        let report = self.0.save_page(page.clone()).await.map_err(ToolError::Args)?;
        let issues: Vec<String> = report.violations.iter().filter(|v| !v.overridden).map(|v| format!("[{}] {} — {}", format!("{:?}", v.level).to_lowercase(), v.message, v.rule)).collect();
        let preview = format!("/_preview{}", if page.slug == "/" { "/" } else { &page.slug });
        Ok(ToolOutput {
            summary: if issues.is_empty() { format!("Saved {} as a draft. It passes every web standard. Owner preview: {preview}", page.slug) } else { format!("Saved {} as a draft with {} issue(s) to fix before publishing:\n{}\nPreview: {preview}", page.slug, issues.len(), issues.join("\n")) },
            files: vec![],
            data: json!({ "preview": preview, "publishable": report.publishable() }),
        })
    }
}

#[async_trait]
impl Tool for SiteOverride {
    fn spec(&self) -> ToolSpec {
        let rules: Vec<&str> = jugg_web::RULES.iter().filter(|r| r.chat_overridable).map(|r| r.id).collect();
        spec("site_override", format!("Record the owner's decision to break a web standard for a page (only after they confirmed in chat, having heard why the rule exists). Overridable rules: {}. Security rules cannot be overridden here.", rules.join(", ")),
            json!({"type":"object","required":["rule","slug","reason"],"properties":{"rule":{"type":"string"},"slug":{"type":"string","description":"page slug or * for all pages"},"reason":{"type":"string","description":"the owner's words"}}}), Risk::Modify)
    }
    async fn run(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let rule = args["rule"].as_str().unwrap_or_default();
        let r = jugg_web::RULES.iter().find(|r| r.id == rule).ok_or_else(|| ToolError::Args(format!("unknown rule {rule}")))?;
        if !r.chat_overridable {
            return Err(ToolError::Args(format!("{rule} protects visitors' security and can only be changed in Settings → Advanced.")));
        }
        let owner: Option<String> = sqlx::query_scalar("SELECT id FROM users WHERE role = 'owner' LIMIT 1").fetch_optional(&self.0.pool).await.ok().flatten();
        sqlx::query("INSERT INTO standards_overrides (id, rule, target, reason, user_id, created_at) VALUES (?,?,?,?,?,?)")
            .bind(jugg_core::new_id())
            .bind(rule)
            .bind(args["slug"].as_str().unwrap_or("*"))
            .bind(args["reason"].as_str().unwrap_or("owner request"))
            .bind(owner.ok_or_else(|| ToolError::Failed("no owner".into()))?)
            .bind(jugg_core::now_unix())
            .execute(&self.0.pool)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let _ = jugg_db::audit::append(&self.0.pool, Some(&self.0.signer), "owner", "standards.override", &args).await;
        Ok(ToolOutput { summary: format!("Override recorded for {rule}. It's logged and visible in Website settings. ({})", r.why), files: vec![], data: Value::Null })
    }
}

#[async_trait]
impl Tool for SitePublish {
    fn spec(&self) -> ToolSpec {
        spec("site_publish", "Publish the website: builds every page, checks all web standards, and switches the live site in one step (the previous version is kept for rollback). Needs the owner's approval.".into(), json!({"type":"object","properties":{}}), Risk::Publish)
    }
    fn approval_summary(&self, _args: &Value) -> String {
        format!("Publish the website to {}", self.0.public_url())
    }
    async fn run(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let msg = self.0.publish("owner").await.map_err(ToolError::Failed)?;
        Ok(ToolOutput { summary: msg, files: vec![], data: Value::Null })
    }
}

#[async_trait]
impl Tool for SiteRollback {
    fn spec(&self) -> ToolSpec {
        spec("site_rollback", "Put the previous version of the website back live. Needs the owner's approval.".into(), json!({"type":"object","properties":{}}), Risk::Publish)
    }
    fn approval_summary(&self, _args: &Value) -> String {
        "Roll the website back to its previous version".into()
    }
    async fn run(&self, _args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let msg = self.0.rollback("owner").await.map_err(ToolError::Failed)?;
        Ok(ToolOutput { summary: msg, files: vec![], data: Value::Null })
    }
}

// ------------------------------------------------------------------ approvals

pub struct DbApprovals {
    pub pool: SqlitePool,
}

#[async_trait]
impl jugg_tools::ApprovalSink for DbApprovals {
    async fn request(&self, tool: &str, args: Value, summary: String, ctx: &ToolContext) -> Result<String, ToolError> {
        let id = jugg_core::new_id();
        let conversation = ctx.job_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        sqlx::query("INSERT INTO approvals (id, action, risk, summary, payload_json, status, requested_at) VALUES (?,?,'publish',?,?,'pending',?)")
            .bind(&id)
            .bind(tool)
            .bind(&summary)
            .bind(json!({ "tool": tool, "args": args, "conversation": conversation }).to_string())
            .bind(jugg_core::now_unix())
            .execute(&self.pool)
            .await
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        Ok(id)
    }
}

pub async fn approvals_since(st: &AppState, conversation: &str, since: i64) -> Vec<crate::api::ApprovalView> {
    let rows: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT id, summary, status, payload_json FROM approvals WHERE status = 'pending' AND requested_at >= ? ORDER BY requested_at").bind(since).fetch_all(&st.pool).await.unwrap_or_default();
    rows.into_iter()
        .filter(|(_, _, _, p)| serde_json::from_str::<Value>(p).ok().is_some_and(|v| v["conversation"] == conversation))
        .map(|(id, summary, status, _)| crate::api::ApprovalView { id, summary, status, result: None })
        .collect()
}

/// The owner clicked Approve / Decline on a pending action.
pub async fn decide_approval(st: &AppState, user: &str, id: &str, approve: bool) -> Result<crate::api::ApprovalView, String> {
    let row: Option<(String, String)> = sqlx::query_as("SELECT summary, payload_json FROM approvals WHERE id = ? AND status = 'pending'").bind(id).fetch_optional(&st.pool).await.map_err(|e| e.to_string())?;
    let (summary, payload) = row.ok_or("This request was already decided or has expired.")?;
    let payload: Value = serde_json::from_str(&payload).unwrap_or_default();
    let now = jugg_core::now_unix();
    if !approve {
        sqlx::query("UPDATE approvals SET status = 'rejected', decided_at = ?, decided_by = ? WHERE id = ?").bind(now).bind(user).bind(id).execute(&st.pool).await.map_err(|e| e.to_string())?;
        return Ok(crate::api::ApprovalView { id: id.into(), summary, status: "rejected".into(), result: Some("Declined. Nothing was changed.".into()) });
    }
    // Claim it first so a double click can't run the action twice.
    let claimed = sqlx::query("UPDATE approvals SET status = 'approved', decided_at = ?, decided_by = ? WHERE id = ? AND status = 'pending'").bind(now).bind(user).bind(id).execute(&st.pool).await.map_err(|e| e.to_string())?.rows_affected();
    if claimed != 1 {
        return Err("This request was already decided.".into());
    }
    let conversation = payload["conversation"].as_str().unwrap_or("approvals").to_string();
    if conversation.contains('/') || conversation.contains("..") {
        return Err("bad approval payload".into());
    }
    let ctx = ToolContext { job_dir: st.cfg.data_dir.join("jobs").join(&conversation), brand: Arc::new(st.brand.read().await.clone()), approved: true };
    let tool = payload["tool"].as_str().unwrap_or_default();
    let result = st.registry.direct().call(tool, payload["args"].clone(), &ctx).await;
    let _ = jugg_db::audit::append(&st.pool, Some(&st.signer), user, "approval.granted", &json!({ "id": id, "tool": tool, "ok": result.is_ok() })).await;
    let text = match result {
        Ok(out) => out.summary,
        Err(e) => format!("It didn't go through: {e}"),
    };
    Ok(crate::api::ApprovalView { id: id.into(), summary, status: "approved".into(), result: Some(text) })
}

// ------------------------------------------------------------------ previews

/// GET /_preview/{*rest} — drafts as visitors will see them (owner only).
pub async fn preview(State(st): State<AppState>, Path(rest): Path<String>, req: Request) -> Response {
    if crate::server::session_user(&st, req.headers()).await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let env = st.site.clone();
    let brand = st.brand.read().await.clone();
    let rest = rest.trim_matches('/');
    if rest == "site.css" {
        let (faces, _) = env.fonts(&brand).await;
        let faces: Vec<FontFace> = faces.into_iter().map(|f| FontFace { url: format!("/_preview{}", f.url), ..f }).collect();
        return ([(header::CONTENT_TYPE, "text/css; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], jugg_web::site_css_with_fonts_pub(&brand, &faces)).into_response();
    }
    if let Some(font) = rest.strip_prefix("fonts/") {
        let (_, files) = env.fonts(&brand).await;
        return match files.into_iter().find(|(p, _)| p == &format!("fonts/{font}")) {
            Some((_, b)) => ([(header::CONTENT_TYPE, "font/ttf")], b).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if let Some(id) = rest.strip_prefix("media/asset-") {
        let row: Option<(String, String)> = sqlx::query_as("SELECT storage_path, mime FROM assets WHERE id = ?").bind(id).fetch_optional(&st.pool).await.ok().flatten();
        return match row.and_then(|(p, m)| std::fs::read(st.cfg.assets_dir().join(p)).ok().map(|b| (m, b))) {
            Some((m, b)) => ([(header::CONTENT_TYPE, m)], b).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    let slug = if rest.is_empty() { "/".to_string() } else { format!("/{rest}") };
    let pages = env.pages().await;
    let nav: Vec<(String, String)> = pages.iter().filter_map(|(p, _, _)| p.nav.clone().map(|l| (l, p.slug.clone()))).collect();
    let Some((mut page, _, _)) = pages.into_iter().find(|(p, _, _)| p.slug == slug) else {
        return (StatusCode::NOT_FOUND, "No such page yet.").into_response();
    };
    for img in images_mut(&mut page) {
        if let Some(id) = img.src.strip_prefix("asset:") {
            img.src = format!("/media/asset-{id}");
        }
    }
    let ctx = SiteContext { nav, css_href: "/site.css".into(), prefix: "/_preview".into(), noindex: true, ..Default::default() };
    let mut res = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], jugg_web::render_page_with(&page, &brand, &ctx)).into_response();
    res.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(jugg_web::Csp::PUBLIC));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

// ------------------------------------------------------------------ public forms

/// POST /forms/{name} on the public site.
pub async fn handle_form(st: &AppState, name: &str, ip: &str, req: Request) -> Response {
    let now = jugg_core::now_unix();
    let key = format!("form:{ip}");
    if st.limiter.lock().map(|mut l| l.blocked(&key, now)).unwrap_or(true) {
        return (StatusCode::TOO_MANY_REQUESTS, "Too many messages from this address. Please try again later.").into_response();
    }
    let forms: Vec<jugg_web::Form> = std::fs::read(st.cfg.site_dir().join("forms.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let Some(form) = forms.into_iter().find(|f| f.name == name) else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(body) = axum::body::to_bytes(req.into_body(), 16 * 1024).await else { return StatusCode::PAYLOAD_TOO_LARGE.into_response() };
    let fields: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    // Count every submission toward the IP's budget (5 per 15 minutes).
    if let Ok(mut l) = st.limiter.lock() {
        for _ in 0..2 {
            l.fail(&key, now);
        }
    }
    if fields.get("_hp").is_some_and(|v| !v.is_empty()) {
        st.steward.send(jugg_steward::Event::new("http", "form_honeypot", jugg_steward::Severity::Low, 0.4, ip, json!({ "form": name })));
        return Redirect::to("/thanks/").into_response();
    }
    let mut data = serde_json::Map::new();
    for f in &form.fields {
        let v = fields.get(&f.name).map(|v| v.trim().chars().take(5000).collect::<String>()).unwrap_or_default();
        if f.required && v.is_empty() {
            return (StatusCode::BAD_REQUEST, format!("Please fill in “{}”.", f.label)).into_response();
        }
        data.insert(f.name.clone(), Value::String(v));
    }
    let id = jugg_core::new_id();
    let Ok(enc) = jugg_crypto::seal(&st.ring, Value::Object(data).to_string().as_bytes(), format!("form_submissions:data:{id}").as_bytes()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let _ = sqlx::query("INSERT INTO form_submissions (id, form, data_enc, created_at) VALUES (?,?,?,?)").bind(&id).bind(name).bind(enc).bind(now).execute(&st.pool).await;
    Redirect::to("/thanks/").into_response()
}

pub async fn submissions(st: &AppState) -> Vec<crate::api::SubmissionView> {
    let rows: Vec<(String, String, Vec<u8>, i64)> = sqlx::query_as("SELECT id, form, data_enc, created_at FROM form_submissions ORDER BY created_at DESC LIMIT 50").fetch_all(&st.pool).await.unwrap_or_default();
    rows.into_iter()
        .filter_map(|(id, form, enc, at)| {
            let plain = jugg_crypto::open(&st.ring, &enc, format!("form_submissions:data:{id}").as_bytes()).ok()?;
            let v: Value = serde_json::from_slice(&plain).ok()?;
            let fields = v.as_object()?.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect();
            Some(crate::api::SubmissionView { id, form, fields, created_at: at })
        })
        .collect()
}

/// First homepage, drafted from the setup wizard so the site is never empty.
pub fn starter_homepage(brand: &BrandStandards, what_you_do: &str, audience: &str) -> Page {
    use jugg_web::{Field, FieldKind, Form, Link, Tone};
    let about = if what_you_do.trim().is_empty() { format!("{} is getting ready. Here's what we do and how to reach us.", brand.name) } else { what_you_do.trim().to_string() };
    let mut description: String = about.chars().take(150).collect();
    if description.chars().count() < 50 {
        description = format!("{description} Get in touch with {} today.", brand.name);
    }
    Page {
        slug: "/".into(),
        title: {
            // ≤ 60 chars, cut at a word boundary (never mid-word).
            let full = format!("{} — {}", brand.name, brand.tagline.clone().unwrap_or_else(|| "welcome".into()));
            let mut t = String::new();
            for w in full.split_whitespace() {
                if t.chars().count() + w.chars().count() + 1 > 60 {
                    break;
                }
                if !t.is_empty() {
                    t.push(' ');
                }
                t.push_str(w);
            }
            t.trim_end_matches(['—', ',', ' ']).trim().to_string()
        },
        description: description.chars().take(160).collect(),
        lang: None,
        nav: None,
        blocks: vec![
            Block::Hero { eyebrow: Some(brand.name.clone()), heading: brand.tagline.clone().unwrap_or_else(|| format!("Welcome to {}", brand.name)), body: (!audience.trim().is_empty()).then(|| format!("For {}.", audience.trim())), cta: Some(Link { label: "Get in touch".into(), href: "#contact".into() }), secondary: None, image: None, tone: Tone::Dark },
            Block::Section { eyebrow: Some("About".into()), heading: format!("What {} does", brand.name), body: about, tone: Tone::Light },
            Block::Contact {
                heading: "Get in touch".into(),
                form: Form {
                    name: "contact".into(),
                    submit_label: "Send message".into(),
                    fields: vec![
                        Field { name: "name".into(), label: "Your name".into(), kind: FieldKind::Text, required: true },
                        Field { name: "email".into(), label: "Email".into(), kind: FieldKind::Email, required: true },
                        Field { name: "message".into(), label: "Message".into(), kind: FieldKind::Textarea, required: true },
                    ],
                },
                tone: Tone::Accent,
            },
        ],
    }
}
