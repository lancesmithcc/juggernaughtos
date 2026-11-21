//! Image-type detection: photo, artwork with blended (anti-aliased) edges,
//! or artwork with hard pixel edges. Each needs different preprocessing.

use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    Photo,
    Blended,
    Hard,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ImageProfile {
    /// Share of strong colour edges that have an intermediate (blended) pixel.
    pub blend_ratio: f32,
    /// Share of opaque pixels whose 4-neighbours are near-identical.
    pub flat_ratio: f32,
    /// Distinct colours in the (sub)sampled image.
    pub unique_colors: usize,
}

const ALPHA_CUTOFF: u8 = 128;

#[inline]
fn rgb_at(img: &RgbaImage, x: i64, y: i64) -> Option<[f32; 3]> {
    if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
        return None;
    }
    let p = img.get_pixel(x as u32, y as u32).0;
    (p[3] >= ALPHA_CUTOFF).then(|| [p[0] as f32, p[1] as f32, p[2] as f32])
}

/// True when pixel (x, y) sits between two clearly different neighbours on
/// opposite sides — i.e. it is an anti-aliasing blend of them.
pub fn is_blend_pixel(img: &RgbaImage, x: i64, y: i64) -> bool {
    let Some(p) = rgb_at(img, x, y) else { return false };
    for (dx, dy) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
        let (Some(a), Some(b)) = (rgb_at(img, x - dx, y - dy), rgb_at(img, x + dx, y + dy)) else {
            continue;
        };
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        if len2 < 48.0 * 48.0 {
            continue;
        }
        let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        let t = (ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2;
        if !(0.12..=0.88).contains(&t) {
            continue;
        }
        let proj = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
        let d2 = (p[0] - proj[0]).powi(2) + (p[1] - proj[1]).powi(2) + (p[2] - proj[2]).powi(2);
        let tol = (0.22 * len2.sqrt()).max(14.0);
        if d2 < tol * tol {
            return true;
        }
    }
    false
}

pub fn profile(img: &RgbaImage) -> ImageProfile {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let total = (w * h).max(1) as f64;
    let step = ((total / 400_000.0).sqrt().floor() as i64).max(1);

    let mut opaque = 0u64;
    let mut flat = 0u64;
    let mut blended = 0u64;
    let mut hard = 0u64;
    let mut colors: HashSet<u32> = HashSet::new();

    let mut y = 1;
    while y < h - 1 {
        let mut x = 1;
        while x < w - 1 {
            if let Some(p) = rgb_at(img, x, y) {
                opaque += 1;
                if colors.len() < 200_000 {
                    colors.insert(((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32);
                }
                let mut maxd = 0f32;
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    if let Some(q) = rgb_at(img, x + dx, y + dy) {
                        let d = (p[0] - q[0]).abs().max((p[1] - q[1]).abs()).max((p[2] - q[2]).abs());
                        maxd = maxd.max(d);
                    }
                }
                if maxd <= 10.0 {
                    flat += 1;
                }
                // Strong edge across this pixel horizontally or vertically?
                for (dx, dy) in [(1, 0), (0, 1)] {
                    if let (Some(a), Some(b)) = (rgb_at(img, x - dx, y - dy), rgb_at(img, x + dx, y + dy)) {
                        let d = (a[0] - b[0]).abs().max((a[1] - b[1]).abs()).max((a[2] - b[2]).abs());
                        if d >= 64.0 {
                            let near_a = (p[0] - a[0]).abs().max((p[1] - a[1]).abs()).max((p[2] - a[2]).abs()) < 12.0;
                            let near_b = (p[0] - b[0]).abs().max((p[1] - b[1]).abs()).max((p[2] - b[2]).abs()) < 12.0;
                            if near_a || near_b {
                                hard += 1;
                            } else {
                                blended += 1;
                            }
                        }
                    }
                }
            }
            x += step;
        }
        y += step;
    }

    ImageProfile {
        blend_ratio: if blended + hard == 0 { 0.0 } else { blended as f32 / (blended + hard) as f32 },
        flat_ratio: if opaque == 0 { 1.0 } else { flat as f32 / opaque as f32 },
        unique_colors: colors.len(),
    }
}

pub fn classify(p: &ImageProfile) -> ImageKind {
    if p.flat_ratio < 0.4 && p.unique_colors > 6_000 {
        ImageKind::Photo
    } else if p.blend_ratio > 0.18 {
        ImageKind::Blended
    } else {
        ImageKind::Hard
    }
}
