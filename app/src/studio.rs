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
        spec("generate_image", "Generate ONE image (hero photo-style image, texture, icon, illustration, logo exploration) through the owner's ChatGPT account (GPT Image), constrained to the brand's palette and imagery rules. Results land in Assets for review. Images are paced to protect the owner's account: one at a time, about 90 seconds apart, at most 10 an hour and 40 a day, so each call can take a few minutes. Make one image, show it, and only make more when the owner asks; never request more than 3 in one reply.",
            json!({"type":"object","required":["prompt"],"properties":{
                "prompt":{"type":"string","description":"what to show; the brand rules are added automatically"},
                "kind":{"type":"string","enum":["image","texture","icon_set","logo"]},
                "shape":{"type":"string","enum":["square","landscape","portrait"]}}}), Risk::Create)
    }
    async fn run(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let prompt = args["prompt"].as_str().unwrap_or_default().trim().to_string();
        if prompt.is_empty() || prompt.len() > 2000 {
            return Err(ToolError::Args("prompt is required".into()));
        }
        let brand = self.0.brand.read().await.clone();
        let full = format!("{} {}", brand.image_prompt_preamble(), prompt);
        let generated = crate::images::generate(&self.0, &full, args["shape"].as_str().unwrap_or("square")).await.map_err(|e| {
            ToolError::Failed(format!("No image this time: {e}. Tell the owner plainly, don't retry in a loop; logo_concepts and social_post need no image model."))
        })?;
        let bytes = generated.bytes;
        let kind = match args["kind"].as_str() {
            Some("texture") => AssetKind::Texture,
            Some("icon_set") => AssetKind::IconSet,
            Some("logo") => AssetKind::Logo,
            _ => AssetKind::Image,
        };
        let name = format!("generated-{}.png", &jugg_crypto::sha256_hex(&bytes)[..10]);
        std::fs::write(job_path(ctx, &name)?, &bytes).map_err(|e| ToolError::Failed(e.to_string()))?;
        let title: String = prompt.chars().take(60).collect();
        jugg_tools::write_meta(&job_path(ctx, &name)?, kind, &title, None)?;
        Ok(ToolOutput { summary: format!("Generated {name} ({}). It will be checked against the brand and shown for review.", generated.note), files: vec![name.into()], data: Value::Null })
    }
}
