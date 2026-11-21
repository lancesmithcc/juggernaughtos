//! Vectorverter 5000 tracing engine.
//!
//! Pipeline: decode → classify → palette → (sub-pixel upscale) → segment →
//! shared boundary chains → corner detection + cubic fitting → SVG.
//! Fully local and deterministic: same input + settings ⇒ identical SVG.

mod boundary;
mod classify;
pub mod color;
mod fit;
mod quantize;
mod segment;
mod subpixel;
mod svg;

pub use classify::{ImageKind, ImageProfile};

use image::{imageops::FilterType, RgbaImage};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    Auto,
    Logo,
    Illustration,
    LineArt,
    Detailed,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Auto, Preset::Logo, Preset::Illustration, Preset::LineArt, Preset::Detailed];

    pub fn settings(self) -> Settings {
        let base = Settings {
            preset: self,
            colors: None,
            max_colors: 24,
            smoothness: 0.6,
            detail: 0.5,
            corner_preservation: 0.7,
            background: Background::Keep,
            optimize_paths: true,
            remove_small_elements: true,
            shape_mode: ShapeMode::Stacked,
            image_kind: None,
            max_upscale: 3,
            palette: None,
        };
        match self {
            Preset::Auto => base,
            Preset::Logo => Settings { max_colors: 12, smoothness: 0.65, detail: 0.45, corner_preservation: 0.85, ..base },
            Preset::Illustration => Settings { max_colors: 32, smoothness: 0.55, detail: 0.6, corner_preservation: 0.6, ..base },
            Preset::LineArt => Settings {
                colors: Some(2),
                smoothness: 0.6,
                detail: 0.6,
                corner_preservation: 0.75,
                background: Background::Transparent,
                ..base
            },
            Preset::Detailed => Settings {
                max_colors: 64,
                smoothness: 0.35,
                detail: 0.85,
                corner_preservation: 0.45,
                remove_small_elements: false,
                ..base
            },
        }
    }
}

impl std::str::FromStr for Preset {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "auto" => Ok(Preset::Auto),
            "logo" => Ok(Preset::Logo),
            "illustration" => Ok(Preset::Illustration),
            "line-art" | "lineart" => Ok(Preset::LineArt),
            "detailed" => Ok(Preset::Detailed),
            other => Err(format!("unknown preset '{other}' (auto, logo, illustration, line-art, detailed)")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Background {
    /// Trace every colour region (source alpha is always honoured).
    Keep,
    /// Drop the dominant border colour, giving a transparent background.
    Transparent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShapeMode {
    /// Larger shapes painted first, holes filled where later shapes cover
    /// them: no hairline seams in any renderer.
    Stacked,
    /// Shapes tile the plane exactly, never overlapping.
    Adjoined,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub preset: Preset,
    /// Fixed palette size; `None` picks automatically.
    pub colors: Option<u32>,
    /// Upper bound for automatic palettes.
    pub max_colors: u32,
    pub smoothness: f32,
    pub detail: f32,
    pub corner_preservation: f32,
    pub background: Background,
    pub optimize_paths: bool,
    pub remove_small_elements: bool,
    pub shape_mode: ShapeMode,
    /// Override image-type detection.
    pub image_kind: Option<ImageKind>,
    /// Sub-pixel upscale cap for blended artwork.
    pub max_upscale: u32,
    /// Exact colours to trace with ("#rrggbb"); overrides `colors`.
    pub palette: Option<Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Preset::Auto.settings()
    }
}

impl Settings {
    /// Check every value is in range. Messages name the offending setting.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |name: &str, v: f32| {
            if (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("{name} must be between 0 and 1 (got {v})"))
            }
        };
        unit("smoothness", self.smoothness)?;
        unit("detail", self.detail)?;
        unit("cornerPreservation", self.corner_preservation)?;
        if let Some(c) = self.colors {
            if !(1..=256).contains(&c) {
                return Err(format!("colors must be between 1 and 256 (got {c})"));
            }
        }
        if !(1..=256).contains(&self.max_colors) {
            return Err(format!("maxColors must be between 1 and 256 (got {})", self.max_colors));
        }
        if !(1..=4).contains(&self.max_upscale) {
            return Err(format!("maxUpscale must be between 1 and 4 (got {})", self.max_upscale));
        }
        if let Some(p) = &self.palette {
            if p.is_empty() {
                return Err("palette must list at least one colour".into());
            }
            for c in p {
                let h = c.trim().trim_start_matches('#');
                if h.len() != 6 || u32::from_str_radix(h, 16).is_err() {
                    return Err(format!("palette entry '{c}' is not a #rrggbb colour"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub colors: usize,
    pub paths: usize,
    pub nodes: usize,
    pub regions: usize,
    pub duration_ms: u64,
    pub svg_bytes: usize,
    pub work_scale: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceOutput {
    pub svg: String,
    pub width: u32,
    pub height: u32,
    pub kind: ImageKind,
    pub profile: ImageProfile,
    pub palette: Vec<String>,
    /// Share of the traced (opaque) area each palette colour covers, 0–1.
    pub coverage: Vec<f32>,
    pub stats: Stats,
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("unable to read image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("cannot read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("image too large: {0} (limits: {MAX_DIMENSION} px per side, {MAX_PIXELS} pixels, {MAX_FILE_BYTES} bytes)")]
    TooLarge(String),
    #[error("image is empty")]
    Empty,
    #[error("image has no opaque pixels to trace")]
    FullyTransparent,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Max pixels processed after any sub-pixel upscale.
const WORK_BUDGET: f64 = 4_500_000.0;
/// Images larger than this are downsampled before tracing.
const MAX_WORK_PIXELS: f64 = 16_000_000.0;

/// Hard caps for untrusted input (decompression-bomb protection): checked
/// from the image header before any pixel is decoded.
pub const MAX_DIMENSION: u32 = 20_000;
pub const MAX_PIXELS: u64 = 64_000_000;
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DECODE_ALLOC: u64 = 1 << 30;

fn reader(bytes: &[u8]) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, TraceError> {
    let mut r = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    r.limits(limits);
    Ok(r)
}

/// Decode image bytes safely: size limits are enforced before decoding.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, TraceError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(TraceError::TooLarge(format!("{} bytes", bytes.len())));
    }
    let (w, h) = reader(bytes)?.into_dimensions().map_err(|e| match e {
        image::ImageError::Limits(_) => TraceError::TooLarge("dimensions exceed limits".into()),
        e => TraceError::Decode(e),
    })?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(TraceError::TooLarge(format!("{w}×{h}")));
    }
    let img = reader(bytes)?.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => TraceError::TooLarge(format!("{w}×{h} needs too much memory")),
        e => TraceError::Decode(e),
    })?;
    Ok(img.to_rgba8())
}

/// Read and decode an image file with the same limits as [`decode`].
pub fn decode_file(path: impl AsRef<Path>) -> Result<RgbaImage, TraceError> {
    let path = path.as_ref();
    let len = std::fs::metadata(path)?.len();
    if len > MAX_FILE_BYTES {
        return Err(TraceError::TooLarge(format!("{len} bytes")));
    }
    decode(&std::fs::read(path)?)
}

pub fn trace_file(path: impl AsRef<Path>, settings: &Settings) -> Result<TraceOutput, TraceError> {
    trace_rgba(&decode_file(path)?, settings)
}

pub fn trace_bytes(bytes: &[u8], settings: &Settings) -> Result<TraceOutput, TraceError> {
    trace_rgba(&decode(bytes)?, settings)
}

/// Resize with premultiplied alpha so transparent pixels don't bleed dark fringes.
fn resize_premultiplied(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    let mut pre = img.clone();
    for p in pre.pixels_mut() {
        let a = p.0[3] as u32;
        for c in 0..3 {
            p.0[c] = ((p.0[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let mut out = image::imageops::resize(&pre, w, h, FilterType::Triangle);
    for p in out.pixels_mut() {
        let a = p.0[3] as u32;
        if a > 0 {
            for c in 0..3 {
                p.0[c] = ((p.0[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    out
}

pub fn trace_rgba(img: &RgbaImage, settings: &Settings) -> Result<TraceOutput, TraceError> {
    let started = Instant::now();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(TraceError::Empty);
    }
    if !img.pixels().any(|p| p.0[3] >= quantize::ALPHA_CUTOFF) {
        return Err(TraceError::FullyTransparent);
    }
    let s = settings;
    let profile = classify::profile(img);
    let kind = s.image_kind.unwrap_or_else(|| classify::classify(&profile));

    // Palette: the user's exact colours, or chosen from the original pixels.
    let custom = s.palette.as_deref().map(quantize::Palette::from_hex).filter(|p| !p.rgb.is_empty());
    let palette = custom.unwrap_or_else(|| {
        let p = quantize::build_palette(img, &quantize::PaletteParams { fixed: s.colors, max_colors: s.max_colors, detail: s.detail, kind });
        if s.colors.is_none() && kind != ImageKind::Photo {
            let cap = (s.max_colors as usize * 3 / 2).max(p.rgb.len());
            quantize::refine_palette(img, p, s.detail, cap)
        } else {
            p
        }
    });

    // Working resolution: blended art is upscaled so its anti-aliasing turns
    // into sub-pixel edge positions; huge images are reduced.
    let area = w as f64 * h as f64;
    let mut scale = 1.0f64;
    if kind == ImageKind::Blended {
        // Half-step factors; anything under 1.5× isn't worth the cost.
        let f = ((WORK_BUDGET / area).sqrt() * 2.0).floor() / 2.0;
        scale = if f < 1.5 { 1.0 } else { f.min(s.max_upscale.max(1) as f64) };
    }
    if area > MAX_WORK_PIXELS {
        scale = (MAX_WORK_PIXELS / area).sqrt();
    }
    let (ww, wh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    let work = if (ww, wh) == (w, h) { img.clone() } else { resize_premultiplied(img, ww, wh) };
    let mut labels = quantize::assign(&work, &palette);
    if kind != ImageKind::Photo {
        quantize::coherence_filter(&work, &mut labels, &palette, 4);
    }
    let debug_dump = |name: &str, labels: &[u16]| {
        if let Ok(dir) = std::env::var("VV_DEBUG_DIR") {
            let mut im = RgbaImage::new(ww, wh);
            for (px, &l) in im.pixels_mut().zip(labels) {
                if l != quantize::TRANSPARENT {
                    let c = palette.rgb[l as usize];
                    px.0 = [c[0], c[1], c[2], 255];
                }
            }
            let _ = im.save(Path::new(&dir).join(name));
        }
    };
    debug_dump("1-labels.png", &labels);

    let orig_min_area = if s.remove_small_elements {
        let frac = lerp((3e-4f32).ln(), (2e-6f32).ln(), s.detail).exp();
        (frac * area as f32).max(lerp(6.0, 1.5, s.detail))
    } else {
        1.0
    };
    let sc2 = (scale * scale) as f32;
    let seg = segment::segment(
        kind != ImageKind::Photo,
        &work,
        &palette,
        labels,
        &segment::SegmentParams {
            min_area: (orig_min_area * sc2).max(0.75 * sc2),
            sliver_thickness: scale as f32 * lerp(1.8, 0.9, s.detail),
            remove_background: s.background == Background::Transparent,
            dot_min_area: 2.5 * sc2,
        },
    );

    debug_dump("2-segments.png", &seg.map.iter().map(|&r| seg.regions[r as usize].label).collect::<Vec<_>>());
    let emitted: Vec<bool> = seg.regions.iter().map(|r| r.label != quantize::TRANSPARENT).collect();
    let graph = boundary::Graph::new(&seg);
    let chains = graph.chains();

    let fp = fit::FitParams {
        corner_angle: (lerp(115.0, 30.0, s.corner_preservation) as f64).to_radians(),
        k: ((2.5 * scale.max(1.0)).round() as usize).max(3),
        smooth_passes: {
            let sigma = lerp(0.4, 2.6, s.smoothness) as f64 * scale.max(1.0);
            (sigma * sigma).round() as usize
        },
        tolerance: (lerp(0.22, 0.9, s.smoothness) * lerp(1.3, 0.7, s.detail)) as f64 * scale,
        tiny_corners: kind == ImageKind::Hard,
    };
    let outside = graph.outside;
    let needed = |c: &boundary::Chain| {
        [c.right, c.left].iter().any(|&r| r != outside && emitted[r as usize])
    };
    let mut fitted: Vec<Option<fit::Fitted>> = vec![None; chains.len()];
    let refiner = subpixel::Refiner::new(&work, &seg, &palette, scale);
    // Boundaries between two close colours are shading bands, not drawn
    // edges: smooth them harder and fit them looser so gradients come out as
    // calm contours instead of jagged blotches. Real edges stay precise.
    let soften = |c: &boundary::Chain| {
        let lab = |r: u32| {
            (r != outside)
                .then(|| seg.regions[r as usize].label)
                .filter(|&l| l != quantize::TRANSPARENT)
                .map(|l| palette.lab[l as usize])
        };
        let contrast = match (lab(c.right), lab(c.left)) {
            (Some(a), Some(b)) => color::dist2(a, b).sqrt(),
            _ => 100.0,
        };
        let (m, t) = if contrast < 12.0 { (3, 1.8) } else if contrast < 22.0 { (2, 1.4) } else { (1, 1.0) };
        fit::FitParams { smooth_passes: fp.smooth_passes * m, tolerance: fp.tolerance * t, ..fp }
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let chunk = chains.len().div_ceil(threads).max(64);
    std::thread::scope(|sc| {
        for (cs, out) in chains.chunks(chunk).zip(fitted.chunks_mut(chunk)) {
            let needed = &needed;
            let refiner = &refiner;
            let soften = &soften;
            sc.spawn(move || {
                for (c, o) in cs.iter().zip(out.iter_mut()) {
                    if needed(c) {
                        let cp = soften(c);
                        *o = Some(fit::fit_chain(&c.pts, &refiner.mids(c), c.closed, &cp));
                    }
                }
            });
        }
    });

    if let Ok(dir) = std::env::var("VV_DEBUG_DIR") {
        let mut js = String::from("[");
        for (c, f) in chains.iter().zip(&fitted) {
            let Some(f) = f else { continue };
            let pts: Vec<String> = c.pts.iter().map(|p| format!("[{},{}]", p.0, p.1)).collect();
            let segs: Vec<String> = f
                .segs
                .iter()
                .map(|s| match *s {
                    fit::Seg::Line(e) => format!("[{},{}]", e.x, e.y),
                    fit::Seg::Cubic(a, b, e) => format!("[{},{},{},{},{},{}]", a.x, a.y, b.x, b.y, e.x, e.y),
                })
                .collect();
            js.push_str(&format!(
                "{{\"closed\":{},\"pts\":[{}],\"start\":[{},{}],\"segs\":[{}]}},",
                c.closed,
                pts.join(","),
                f.start.x,
                f.start.y,
                segs.join(",")
            ));
        }
        js.pop();
        js.push(']');
        let _ = std::fs::write(Path::new(&dir).join("chains.json"), js);
    }
    let assembled = svg::assemble(&seg, &palette.rgb, &chains, &fitted, &emitted, s.shape_mode, s.optimize_paths, scale, kind != ImageKind::Hard, w, h);
    let stats = Stats {
        colors: assembled.colors,
        paths: assembled.paths,
        nodes: assembled.nodes,
        regions: emitted.iter().filter(|&&e| e).count(),
        duration_ms: started.elapsed().as_millis() as u64,
        svg_bytes: assembled.svg.len(),
        work_scale: scale,
    };
    let mut cover = vec![0u64; palette.rgb.len()];
    for r in seg.regions.iter().filter(|r| r.label != quantize::TRANSPARENT) {
        cover[r.label as usize] += r.area as u64;
    }
    let covered = cover.iter().sum::<u64>().max(1) as f32;
    Ok(TraceOutput {
        coverage: cover.iter().map(|&c| c as f32 / covered).collect(),
        svg: assembled.svg,
        width: w,
        height: h,
        kind,
        profile,
        palette: palette.rgb.iter().map(|&c| color::hex(c)).collect(),
        stats,
    })
}
