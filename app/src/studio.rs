//! Brand studio: logo concepts → adopt → variants + brand book; social posts.

use crate::api::AssetCard;
use crate::site::SiteEnv;
use async_trait::async_trait;
use jugg_brand::BrandStandards;
use jugg_core::{AssetKind, Risk};
use jugg_db::sqlx;
use jugg_print::logo::{Concept, Variant};
use jugg_tools::{job_path, Tool, ToolContext, ToolError, ToolOutput, ToolSpec};
use serde_json::{json, Value};
use std::sync::Arc;

fn concept_key(c: Concept) -> String {
    serde_json::to_value(c).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

pub fn parse_concept(s: &str) -> Option<Concept> {
    serde_json::from_value(json!(s.trim().to_lowercase())).ok()
}

/// Store bytes as an asset directly (used for things the owner already chose).
pub async fn store(env: &SiteEnv, bytes: &[u8], ext: &str, kind: AssetKind, title: &str, preview_png: Option<&[u8]>, status: &str) -> Result<String, String> {
    let sha = jugg_crypto::sha256_hex(bytes);
    if let Some(id) = sqlx::query_scalar::<_, String>("SELECT id FROM assets WHERE sha256 = ?").bind(&sha).fetch_optional(&env.pool).await.map_err(|e| e.to_string())? {
        let self_preview = matches!(ext, "png" | "jpg" | "jpeg" | "webp" | "svg").then(|| format!("{sha}.{ext}"));
        sqlx::query("UPDATE assets SET status = ?, preview_path = COALESCE(preview_path, ?) WHERE id = ?").bind(status).bind(self_preview).bind(&id).execute(&env.pool).await.map_err(|e| e.to_string())?;
        return Ok(id);
    }
    let store = env.cfg.assets_dir();
    std::fs::create_dir_all(&store).map_err(|e| e.to_string())?;
    let stored = format!("{sha}.{ext}");
    std::fs::write(store.join(&stored), bytes).map_err(|e| e.to_string())?;
    let preview = match preview_png {
        Some(p) => {
            let name = format!("{sha}.preview.png");
            std::fs::write(store.join(&name), p).map_err(|e| e.to_string())?;
            Some(name)
        }
        // Images are their own preview.
        None if matches!(ext, "png" | "jpg" | "jpeg" | "webp" | "svg") => Some(stored.clone()),
        None => None,
    };
    let brand = env.brand.read().await.clone();
    let strict = if matches!(kind, AssetKind::Logo | AssetKind::LogoVector) { jugg_brand::gate::Strictness::Exact } else { jugg_brand::gate::Strictness::Loose };
    let report = jugg_brand::gate::check_file(&brand, &store.join(&stored), strict);
    let id = jugg_core::new_id();
    let kind_s = serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    sqlx::query(
        "INSERT INTO assets (id, kind, title, status, storage_path, mime, bytes, sha256, encrypted, brand_version, gate_report_json, preview_path, created_at, decided_at)
         VALUES (?,?,?,?,?,?,?,?,0,1,?,?,?,?)",
    )
    .bind(&id)
    .bind(kind_s)
    .bind(title)
    .bind(status)
    .bind(&stored)
    .bind(crate::assets::mime_for(ext))
    .bind(bytes.len() as i64)
    .bind(&sha)
    .bind(serde_json::to_string(&report).unwrap_or_default())
    .bind(preview)
    .bind(jugg_core::now_unix())
    .bind((status == "final").then(jugg_core::now_unix))
    .execute(&env.pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(id)
}

pub fn concepts_preview(brand: &BrandStandards) -> Result<Vec<(String, String, String)>, String> {
    use base64::Engine;
    let all = jugg_print::logo::concepts(brand).map_err(|e| e.to_string())?;
    Ok(all.into_iter().map(|l| (concept_key(l.concept), l.concept.label().to_string(), format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(l.svg.as_bytes())))).collect())
}

/// Adopt a concept: final logo files, brand updated, brand book + launch post made.
pub async fn adopt(env: &SiteEnv, concept: Concept) -> Result<(String, Vec<String>), String> {
    let brand = env.brand.read().await.clone();
    let b2 = brand.clone();
    let logos = tokio::task::spawn_blocking(move || jugg_print::logo::variants(&b2, concept)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    let mut by_variant = std::collections::HashMap::new();
    for l in &logos {
        let title = format!("{} logo · {}", brand.name, l.variant.label());
        let kind = if l.variant == Variant::Icon { AssetKind::Logo } else { AssetKind::LogoVector };
        let id = store(env, l.svg.as_bytes(), "svg", kind, &title, Some(&l.png), "final").await?;
        by_variant.insert(l.variant, id.clone());
        ids.push(id);
    }
    let mut updated = brand.clone();
    updated.logo.primary_asset = by_variant.get(&Variant::Primary).cloned();
    updated.logo.reversed_asset = by_variant.get(&Variant::Reversed).cloned();
    updated.logo.mono_asset = by_variant.get(&Variant::Mono).cloned();
    updated.logo.mark_asset = by_variant.get(&Variant::Icon).cloned();
    updated.logo.concept = Some(concept_key(concept));
    save_brand(env, &updated).await?;

    // Brand book and a first social post, so the kit is usable on day one.
    let (b3, l3) = (updated.clone(), logos.clone());
    let (book, cover) = tokio::task::spawn_blocking(move || jugg_print::brand_book(&b3, concept, &l3)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
    ids.push(store(env, &book, "pdf", AssetKind::Pdf, &format!("{} brand guidelines", updated.name), Some(&cover), "final").await?);
    let icon = logos.iter().find(|l| l.variant == Variant::Icon).map(|l| l.svg.clone());
    let b4 = updated.clone();
    let headline = updated.tagline.clone().unwrap_or_else(|| format!("Welcome to {}", updated.name));
    let post = tokio::task::spawn_blocking(move || jugg_print::social_post(&b4, &jugg_print::SocialInput { headline, body: None, cta: Some("Say hello".into()), tone: None, photo: None, icon_svg: icon }, jugg_print::SocialSize::Square))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    ids.push(store(env, &post.1, "png", AssetKind::SocialPost, "Launch post · Square", None, "review").await?);
    let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), "owner", "brand.logo_adopted", &json!({ "concept": concept_key(concept), "assets": ids })).await;
    Ok((format!("Logo adopted ({}). Saved four logo files, the brand guidelines PDF and a launch post.", concept.label()), ids))
}

/// Every file a finished logo ships as.
struct LogoPack {
    svg: String,
    pdf: Vec<u8>,
    eps: Vec<u8>,
    png_high: Vec<u8>,
    png_screen: Vec<u8>,
    webp_high: Vec<u8>,
    webp_screen: Vec<u8>,
}

/// Longest side of the high-resolution and screen raster files.
pub const LOGO_HIGH_PX: u32 = 4000;
pub const LOGO_SCREEN_PX: u32 = 1000;

fn webp(png: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(png).map_err(|e| e.to_string())?.into_rgba8();
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .encode(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Raster (or SVG) logo → traced, cropped vector plus print and screen files. Blocking.
fn build_logo_pack(bytes: &[u8], is_svg: bool, title: &str) -> Result<LogoPack, String> {
    let svg = if is_svg {
        String::from_utf8(bytes.to_vec()).map_err(|_| "That SVG isn't valid text.".to_string())?
    } else {
        let mut settings = vectorverter_core::Preset::Logo.settings();
        // ChatGPT draws logos on a plain background: drop it, letter counters included,
        // so every file is transparent.
        settings.background = vectorverter_core::Background::Knockout;
        let traced = vectorverter_core::trace_bytes(bytes, &settings).map_err(|e| format!("Vectorverter couldn't trace it: {e}"))?;
        jugg_print::trim_to_artwork(&traced.svg, 0.04).map_err(|e| e.to_string())?
    };
    let png_high = jugg_print::png_fit(&svg, LOGO_HIGH_PX).map_err(|e| e.to_string())?;
    let png_screen = jugg_print::png_fit(&svg, LOGO_SCREEN_PX).map_err(|e| e.to_string())?;
    Ok(LogoPack {
        pdf: jugg_print::logo_pdf(&svg, title).map_err(|e| e.to_string())?,
        eps: jugg_print::eps_from_svg(&svg, title).map_err(|e| e.to_string())?,
        webp_high: webp(&png_high)?,
        webp_screen: webp(&png_screen)?,
        png_high,
        png_screen,
        svg,
    })
}

/// The owner approved a logo image: vectorize it with Vectorverter 5000, make the
/// SVG, PDF, EPS, PNG and WebP (high-res and screen) files, and make it the brand's
/// logo (website header, favicon, sidebar). Returns a summary and the new asset ids.
pub async fn finish_logo(env: &SiteEnv, actor: &str, asset_id: &str) -> Result<(String, Vec<String>), String> {
    let row: Option<(String, String)> = sqlx::query_as("SELECT kind, storage_path FROM assets WHERE id = ?").bind(asset_id).fetch_optional(&env.pool).await.map_err(|e| e.to_string())?;
    let (kind, stored) = row.ok_or("That logo is no longer in Assets.")?;
    if kind != "logo" && kind != "logo_vector" {
        return Err("Only logos get the logo file set.".into());
    }
    if stored.contains('/') || stored.contains("..") {
        return Err("That file can't be read.".into());
    }
    let ext = stored.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "svg") {
        return Err("Only PNG, JPEG, WebP or SVG logos can be turned into the file set.".into());
    }
    let bytes = std::fs::read(env.cfg.assets_dir().join(&stored)).map_err(|e| format!("couldn't read the logo: {e}"))?;
    let brand = env.brand.read().await.clone();
    let name = brand.name.clone();
    let title = format!("{name} logo");
    let t2 = title.clone();
    let pack = tokio::task::spawn_blocking(move || build_logo_pack(&bytes, ext == "svg", &t2)).await.map_err(|e| e.to_string())??;

    let files: [(&[u8], &str, AssetKind, String, Option<&[u8]>); 7] = [
        (pack.svg.as_bytes(), "svg", AssetKind::LogoVector, format!("{title} · Vector (SVG)"), Some(&pack.png_screen)),
        (&pack.pdf, "pdf", AssetKind::LogoVector, format!("{title} · Print (PDF)"), Some(&pack.png_screen)),
        (&pack.eps, "eps", AssetKind::LogoVector, format!("{title} · Print (EPS)"), Some(&pack.png_screen)),
        (&pack.png_high, "png", AssetKind::Logo, format!("{title} · PNG high-res ({LOGO_HIGH_PX} px)"), Some(&pack.png_screen)),
        (&pack.png_screen, "png", AssetKind::Logo, format!("{title} · PNG screen ({LOGO_SCREEN_PX} px)"), None),
        (&pack.webp_high, "webp", AssetKind::Logo, format!("{title} · WebP high-res ({LOGO_HIGH_PX} px)"), Some(&pack.png_screen)),
        (&pack.webp_screen, "webp", AssetKind::Logo, format!("{title} · WebP screen ({LOGO_SCREEN_PX} px)"), None),
    ];
    let mut ids = Vec::new();
    for (bytes, ext, kind, title, preview) in files {
        ids.push(store(env, bytes, ext, kind, &title, preview, "final").await?);
    }
    let mut updated = brand.clone();
    updated.logo.primary_asset = ids.first().cloned();
    updated.logo.mark_asset = ids.first().cloned();
    // The old concept's reversed and one-colour versions no longer match.
    updated.logo.reversed_asset = None;
    updated.logo.mono_asset = None;
    updated.logo.concept = None;
    save_brand(env, &updated).await?;
    let _ = jugg_db::audit::append(&env.pool, Some(&env.signer), actor, "brand.logo_finished", &json!({ "from": asset_id, "assets": ids })).await;
    Ok((
        format!("Your logo is vectorized and saved as SVG, PDF and EPS (print), plus PNG and WebP at {LOGO_HIGH_PX} px and {LOGO_SCREEN_PX} px. It's now your brand logo: the website header and favicon use it from the next publish."),
        ids,
    ))
}

pub async fn save_brand(env: &SiteEnv, b: &BrandStandards) -> Result<(), String> {
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) + 1 FROM brands").fetch_one(&env.pool).await.map_err(|e| e.to_string())?;
    let mut tx = env.pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("UPDATE brands SET is_active = 0").execute(&mut *tx).await.map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO brands (id, version, standards_json, is_active, created_at) VALUES ('brand', ?, ?, 1, ?)")
        .bind(version)
        .bind(serde_json::to_string(b).map_err(|e| e.to_string())?)
        .bind(jugg_core::now_unix())
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    *env.brand.write().await = b.clone();
    Ok(())
}

pub async fn logo_file(env: &SiteEnv, asset: &Option<String>) -> Option<String> {
    let id = asset.as_ref()?;
    let stored: String = sqlx::query_scalar("SELECT storage_path FROM assets WHERE id = ?").bind(id).fetch_optional(&env.pool).await.ok()??;
    std::fs::read_to_string(env.cfg.assets_dir().join(stored)).ok()
}

pub async fn cards(st: &crate::server::AppState, ids: &[String]) -> Vec<AssetCard> {
    let mut out = Vec::new();
    for id in ids {
        if let Some(c) = crate::assets::card(st, id).await {
            out.push(c);
        }
    }
    out
}

// ------------------------------------------------------------------ agent tools

pub struct LogoConcepts(pub Arc<SiteEnv>);
pub struct AdoptLogo(pub Arc<SiteEnv>);
pub struct SocialPost(pub Arc<SiteEnv>);
pub struct BrandBook(pub Arc<SiteEnv>);

fn spec(name: &str, d: &str, schema: Value, risk: Risk) -> ToolSpec {
    ToolSpec { name: name.into(), description: d.into(), input_schema: schema, risk, long_running: false }
}

#[async_trait]
impl Tool for LogoConcepts {
    fn spec(&self) -> ToolSpec {
        spec("logo_concepts", "Draw four logo concepts (monogram, wordmark, badge, initials) as clean vector files in the brand's colours and fonts. Show them to the owner and ask which one to adopt.", json!({"type":"object","properties":{}}), Risk::Create)
    }
    async fn run(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let b = self.0.brand.read().await.clone();
        let all = tokio::task::spawn_blocking(move || jugg_print::logo::concepts(&b)).await.map_err(|e| ToolError::Failed(e.to_string()))?.map_err(|e| ToolError::Failed(e.to_string()))?;
        let mut files = Vec::new();
        for l in all {
            let base = format!("logo-concept-{}", concept_key(l.concept));
            std::fs::write(job_path(ctx, &format!("{base}.svg"))?, &l.svg).map_err(|e| ToolError::Failed(e.to_string()))?;
            std::fs::write(job_path(ctx, &format!("{base}.png"))?, &l.png).map_err(|e| ToolError::Failed(e.to_string()))?;
            jugg_tools::write_meta(&job_path(ctx, &format!("{base}.svg"))?, AssetKind::Logo, &format!("Logo concept · {}", l.concept.label()), Some(&format!("{base}.png")))?;
            files.push(format!("{base}.svg").into());
        }
        Ok(ToolOutput { summary: "Four logo concepts are ready: monogram, wordmark, badge and initials. Ask the owner which one they like; adopt it with adopt_logo.".into(), files, data: Value::Null })
    }
}

#[async_trait]
impl Tool for AdoptLogo {
    fn spec(&self) -> ToolSpec {
        spec("adopt_logo", "Make one of the concepts the official logo (after the owner chose it). Creates primary, reversed, one-colour and icon versions, updates the brand, and produces the brand guidelines PDF.",
            json!({"type":"object","required":["concept"],"properties":{"concept":{"type":"string","enum":["monogram","wordmark","badge","initials"]}}}), Risk::Modify)
    }
    async fn run(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let c = parse_concept(args["concept"].as_str().unwrap_or_default()).ok_or_else(|| ToolError::Args("concept must be monogram, wordmark, badge or initials".into()))?;
        let (summary, ids) = adopt(&self.0, c).await.map_err(ToolError::Failed)?;
        Ok(ToolOutput { summary: format!("{summary} They're in Assets. The website header and favicon will use the new logo next time it's published."), files: vec![], data: json!({ "assets": ids }) })
    }
}

pub struct LogoFiles(pub Arc<SiteEnv>);

#[async_trait]
impl Tool for LogoFiles {
    fn spec(&self) -> ToolSpec {
        spec("finish_logo", "The owner chose a logo image (for example one made in ChatGPT): vectorize it with Vectorverter 5000 and make the complete logo file set (SVG, print PDF, EPS, PNG and WebP in high-res and screen sizes), then make it the brand's logo. Pass the asset id from the message that named it. Approving a logo in Assets does this by itself, so only call it when the owner asks in chat.",
            json!({"type":"object","required":["asset"],"properties":{"asset":{"type":"string","description":"asset id of the chosen logo image"}}}), Risk::Modify)
    }
    async fn run(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let asset = args["asset"].as_str().map(str::trim).filter(|a| !a.is_empty()).ok_or_else(|| ToolError::Args("asset is required".into()))?;
        let _ = sqlx::query("UPDATE assets SET status = 'final', decided_at = ? WHERE id = ? AND status != 'final'").bind(jugg_core::now_unix()).bind(asset).execute(&self.0.pool).await;
        let (summary, ids) = finish_logo(&self.0, "agent", asset).await.map_err(ToolError::Failed)?;
        Ok(ToolOutput { summary: format!("{summary} The files are in Assets."), files: vec![], data: json!({ "assets": ids }) })
    }
}

#[async_trait]
impl Tool for SocialPost {
    fn spec(&self) -> ToolSpec {
        spec("social_post", "Make an on-brand social post image at every platform size (square, portrait, story, link preview). Keep the headline short (under 60 characters). Optional photo: \"asset:<id>\" of an approved image.",
            json!({"type":"object","required":["headline"],"properties":{
                "headline":{"type":"string"},"body":{"type":"string"},"cta":{"type":"string"},
                "tone":{"type":"string","enum":["dark","accent"]},"photo":{"type":"string"},
                "sizes":{"type":"array","items":{"type":"string","enum":["square","portrait","story","link"]}}}}), Risk::Create)
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let mut input: jugg_print::SocialInput = serde_json::from_value(args.clone()).map_err(|e| ToolError::Args(e.to_string()))?;
        if input.headline.trim().is_empty() || input.headline.chars().count() > 120 {
            return Err(ToolError::Args("headline is required (keep it short)".into()));
        }
        let b = self.0.brand.read().await.clone();
        input.icon_svg = logo_file(&self.0, &b.logo.mark_asset).await;
        if let Some(id) = args["photo"].as_str().and_then(|p| p.strip_prefix("asset:")) {
            let row: Option<(String, String, String)> = sqlx::query_as("SELECT storage_path, mime, status FROM assets WHERE id = ?").bind(id).fetch_optional(&self.0.pool).await.ok().flatten();
            match row {
                Some((p, mime, status)) if status == "final" && mime.starts_with("image/") && !mime.contains("svg") => input.photo = std::fs::read(self.0.cfg.assets_dir().join(p)).ok().map(|bytes| (bytes, mime)),
                _ => return Err(ToolError::Args("photo must be an approved image asset".into())),
            }
        }
        let wanted: Vec<String> = args["sizes"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_else(|| vec!["square".into(), "portrait".into(), "story".into(), "link".into()]);
        let slug: String = input.headline.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|s| !s.is_empty()).take(6).collect::<Vec<_>>().join("-");
        let mut files = Vec::new();
        for (size, label, w, h) in jugg_print::SOCIAL_SIZES {
            let key = serde_json::to_value(size).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            if !wanted.contains(&key) {
                continue;
            }
            let (b2, i2) = (b.clone(), input.clone());
            let (_, png) = tokio::task::spawn_blocking(move || jugg_print::social_post(&b2, &i2, size)).await.map_err(|e| ToolError::Failed(e.to_string()))?.map_err(|e| ToolError::Failed(e.to_string()))?;
            let name = format!("social-{slug}-{key}.png");
            std::fs::write(job_path(ctx, &name)?, png).map_err(|e| ToolError::Failed(e.to_string()))?;
            jugg_tools::write_meta(&job_path(ctx, &name)?, AssetKind::SocialPost, &format!("{} · {label} {w}×{h}", input.headline), None)?;
            files.push(name.into());
        }
        Ok(ToolOutput { summary: format!("Made {} social image(s) for “{}”.", files.len(), input.headline), files, data: Value::Null })
    }
}

#[async_trait]
impl Tool for BrandBook {
    fn spec(&self) -> ToolSpec {
        spec("brand_book", "Regenerate the brand guidelines PDF from the current brand (after colours, fonts or the logo change).", json!({"type":"object","properties":{}}), Risk::Create)
    }
    async fn run(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let b = self.0.brand.read().await.clone();
        let concept = b.logo.concept.as_deref().and_then(parse_concept).unwrap_or(Concept::Monogram);
        let b2 = b.clone();
        let (pdf, cover) = tokio::task::spawn_blocking(move || {
            let logos = jugg_print::logo::variants(&b2, concept)?;
            jugg_print::brand_book(&b2, concept, &logos)
        })
        .await
        .map_err(|e| ToolError::Failed(e.to_string()))?
        .map_err(|e| ToolError::Failed(e.to_string()))?;
        std::fs::write(job_path(ctx, "brand-guidelines.pdf")?, pdf).map_err(|e| ToolError::Failed(e.to_string()))?;
        std::fs::write(job_path(ctx, "brand-guidelines-cover.png")?, cover).map_err(|e| ToolError::Failed(e.to_string()))?;
        jugg_tools::write_meta(&job_path(ctx, "brand-guidelines.pdf")?, AssetKind::Pdf, &format!("{} brand guidelines", b.name), Some("brand-guidelines-cover.png"))?;
        Ok(ToolOutput { summary: "Brand guidelines PDF ready (6 pages: cover, logo, don'ts, colour, type, voice & imagery).".into(), files: vec!["brand-guidelines.pdf".into()], data: Value::Null })
    }
}

// ------------------------------------------------------------------ image generation (owner's key)

pub struct GenerateImage(pub Arc<SiteEnv>);

pub async fn image_key(env: &SiteEnv) -> Option<(String, String)> {
    let row: Option<(String, Option<Vec<u8>>)> = sqlx::query_as("SELECT config_json, secret_enc FROM providers WHERE id = 'images'").fetch_optional(&env.pool).await.ok().flatten();
    let (cfg, enc) = row?;
    let key = String::from_utf8(jugg_crypto::open(&env.ring, &enc?, b"providers:secret:images").ok()?).ok()?;
    let model = serde_json::from_str::<Value>(&cfg).ok().and_then(|v| v["model"].as_str().map(str::to_string)).unwrap_or_else(|| "gpt-image-1".into());
    Some((key, model))
}

pub async fn save_image_key(env: &SiteEnv, key: Option<String>, model: Option<String>) -> Result<(), String> {
    let model = model.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "gpt-image-1".into());
    let enc = match key.filter(|k| !k.trim().is_empty()) {
        Some(k) => Some(jugg_crypto::seal(&env.ring, k.trim().as_bytes(), b"providers:secret:images").map_err(|e| e.to_string())?),
        None => sqlx::query_scalar::<_, Option<Vec<u8>>>("SELECT secret_enc FROM providers WHERE id = 'images'").fetch_optional(&env.pool).await.map_err(|e| e.to_string())?.flatten(),
    };
    sqlx::query("INSERT INTO providers (id, kind, label, config_json, secret_enc, priority, enabled, created_at) VALUES ('images','openai_api','Image generation',?,?,50,1,?)
                 ON CONFLICT(id) DO UPDATE SET config_json = excluded.config_json, secret_enc = excluded.secret_enc")
        .bind(json!({ "model": model }).to_string())
        .bind(enc)
        .bind(jugg_core::now_unix())
        .execute(&env.pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[async_trait]
impl Tool for GenerateImage {
    fn spec(&self) -> ToolSpec {
        spec("generate_image", "Make ONE image (hero photo-style image, texture, icon, illustration, logo exploration) with GPT Image in the owner's own ChatGPT plan. The brand's palette and imagery rules are written into the prompt for you. When Codex is signed in on this box the image is made automatically in the background (about a minute); otherwise the owner gets an Open in ChatGPT button and drops the result back here. Either way it lands in Assets for review and you get a message naming it. For a logo (kind logo): don't vectorize it yourself. When the owner approves it in Assets, Vectorverter makes the full logo file set (SVG, PDF, EPS, PNG, WebP) automatically; if they approve it in chat instead, call finish_logo with its asset id. Call once per image the owner needs (at most 3 per reply), then stop and tell the owner in one short sentence that the image is on its way. Never call again for the same image.",
            json!({"type":"object","required":["prompt"],"properties":{
                "prompt":{"type":"string","description":"what to show; the brand rules are added automatically"},
                "title":{"type":"string","description":"short name for the asset, e.g. 'Homepage hero'"},
                "kind":{"type":"string","enum":["image","texture","icon_set","logo"]},
                "shape":{"type":"string","enum":["square","landscape","portrait"]}}}), Risk::Create)
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let prompt = args["prompt"].as_str().unwrap_or_default().trim().to_string();
        if prompt.is_empty() || prompt.len() > 2000 {
            return Err(ToolError::Args("prompt is required".into()));
        }
        let kind = match args["kind"].as_str() {
            Some(k @ ("texture" | "icon_set" | "logo")) => k,
            _ => "image",
        };
        let shape = match args["shape"].as_str() {
            Some(s @ ("landscape" | "portrait")) => s,
            _ => "square",
        };
        let title: String = args["title"].as_str().filter(|t| !t.trim().is_empty()).unwrap_or(&prompt).trim().chars().take(60).collect();
        let conversation = ctx.job_dir.file_name().map(|n| n.to_string_lossy().to_string()).ok_or_else(|| ToolError::Failed("no conversation".into()))?;
        let brand = self.0.brand.read().await.clone();
        let full = format!("{} {}", brand.image_prompt_preamble(), prompt);
        let id = crate::images::create_handoff(&self.0, &conversation, &full, kind, shape, &title).await.map_err(ToolError::Failed)?;
        Ok(ToolOutput {
            summary: if crate::images::codex_ready(&self.0.cfg).await {
                format!("Image “{title}” is being made automatically in the owner's ChatGPT (about a minute). It appears in this chat and in Assets by itself; the owner has nothing to do.")
            } else {
                format!("Image “{title}” is ready for the owner to make: they get an Open in ChatGPT button with the prompt filled in, then drop the image back here. It then lands in Assets.")
            },
            files: vec![],
            data: json!({ "handoff": id }),
        })
    }
}
