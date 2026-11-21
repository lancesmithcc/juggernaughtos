//! Palette selection (automatic or fixed size) and per-pixel assignment.
//!
//! Blended edge pixels are down-weighted so anti-aliasing never becomes a
//! palette colour of its own; the palette describes the flat shapes.

use crate::classify::{is_blend_pixel, ImageKind};
use crate::color::{dist2, rgb_to_lab, Lab};
use image::RgbaImage;
use std::collections::HashMap;

pub const TRANSPARENT: u16 = u16::MAX;
pub const ALPHA_CUTOFF: u8 = 128;

#[derive(Clone, Debug)]
pub struct Palette {
    pub rgb: Vec<[u8; 3]>,
    pub lab: Vec<Lab>,
}

impl Palette {
    /// Parse "#rrggbb" / "rrggbb" entries, skipping invalid ones and duplicates.
    pub fn from_hex(list: &[String]) -> Palette {
        let mut rgb: Vec<[u8; 3]> = Vec::new();
        for h in list {
            let h = h.trim().trim_start_matches('#');
            if h.len() != 6 {
                continue;
            }
            let Ok(v) = u32::from_str_radix(h, 16) else { continue };
            let c = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
            if !rgb.contains(&c) {
                rgb.push(c);
            }
        }
        let lab = rgb.iter().map(|&c| rgb_to_lab(c)).collect();
        Palette { rgb, lab }
    }
}

#[derive(Clone, Copy)]
struct Bin {
    rgb: [f32; 3],
    lab: Lab,
    w: f32,
}

#[derive(Clone, Copy, Debug)]
struct Center {
    rgb: [f32; 3],
    lab: Lab,
    w: f32,
}

pub struct PaletteParams {
    pub fixed: Option<u32>,
    pub max_colors: u32,
    pub detail: f32,
    pub kind: ImageKind,
}

fn collect_bins(img: &RgbaImage, kind: ImageKind) -> Vec<Bin> {
    // 6 bits per channel: 262k bins holding weighted sums.
    let mut acc = vec![[0f32; 4]; 1 << 18];
    let (w, h) = (img.width() as i64, img.height() as i64);
    for y in 0..h {
        for x in 0..w {
            let p = img.get_pixel(x as u32, y as u32).0;
            if p[3] < ALPHA_CUTOFF {
                continue;
            }
            let weight = if kind != ImageKind::Photo && is_blend_pixel(img, x, y) { 0.03 } else { 1.0 };
            let key = ((p[0] as usize >> 2) << 12) | ((p[1] as usize >> 2) << 6) | (p[2] as usize >> 2);
            let a = &mut acc[key];
            a[0] += weight;
            a[1] += weight * p[0] as f32;
            a[2] += weight * p[1] as f32;
            a[3] += weight * p[2] as f32;
        }
    }
    acc.iter()
        .filter(|a| a[0] > 0.0)
        .map(|a| {
            let rgb = [a[1] / a[0], a[2] / a[0], a[3] / a[0]];
            Bin { rgb, lab: rgb_to_lab(round_rgb(rgb)), w: a[0] }
        })
        .collect()
}

fn round_rgb(c: [f32; 3]) -> [u8; 3] {
    [c[0].round().clamp(0.0, 255.0) as u8, c[1].round().clamp(0.0, 255.0) as u8, c[2].round().clamp(0.0, 255.0) as u8]
}

fn center_of(bins: &[Bin], idx: &[usize]) -> Center {
    let mut c = Center { rgb: [0.0; 3], lab: [0.0; 3], w: 0.0 };
    for &i in idx {
        let b = &bins[i];
        c.w += b.w;
        for k in 0..3 {
            c.rgb[k] += b.rgb[k] * b.w;
            c.lab[k] += b.lab[k] * b.w;
        }
    }
    if c.w > 0.0 {
        for k in 0..3 {
            c.rgb[k] /= c.w;
            c.lab[k] /= c.w;
        }
    }
    c
}

fn median_cut(bins: &[Bin], k: usize) -> Vec<Center> {
    let mut boxes: Vec<Vec<usize>> = vec![(0..bins.len()).collect()];
    while boxes.len() < k {
        // Pick the box with the largest weight × extent.
        let mut best: Option<(usize, usize, f32)> = None;
        for (bi, bx) in boxes.iter().enumerate() {
            if bx.len() < 2 {
                continue;
            }
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            let mut w = 0.0;
            for &i in bx {
                w += bins[i].w;
                for c in 0..3 {
                    lo[c] = lo[c].min(bins[i].lab[c]);
                    hi[c] = hi[c].max(bins[i].lab[c]);
                }
            }
            let (axis, range) = (0..3).map(|c| (c, hi[c] - lo[c])).fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
            let score = range * w.sqrt();
            if range > 0.5 && best.is_none_or(|b| score > b.2) {
                best = Some((bi, axis, score));
            }
        }
        let Some((bi, axis, _)) = best else { break };
        let mut bx = boxes.swap_remove(bi);
        bx.sort_by(|&a, &b| bins[a].lab[axis].total_cmp(&bins[b].lab[axis]).then(a.cmp(&b)));
        let total: f32 = bx.iter().map(|&i| bins[i].w).sum();
        let mut acc = 0.0;
        let mut cut = bx.len() / 2;
        for (j, &i) in bx.iter().enumerate() {
            acc += bins[i].w;
            if acc >= total / 2.0 {
                cut = (j + 1).clamp(1, bx.len() - 1);
                break;
            }
        }
        let tail = bx.split_off(cut);
        boxes.push(bx);
        boxes.push(tail);
    }
    boxes.iter().map(|b| center_of(bins, b)).filter(|c| c.w > 0.0).collect()
}

fn nearest(centers: &[Center], lab: Lab) -> usize {
    let mut best = 0;
    let mut bd = f32::MAX;
    for (i, c) in centers.iter().enumerate() {
        let d = dist2(c.lab, lab);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

fn kmeans(bins: &[Bin], centers: &mut Vec<Center>, iters: usize) {
    for _ in 0..iters {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); centers.len()];
        for (i, b) in bins.iter().enumerate() {
            groups[nearest(centers, b.lab)].push(i);
        }
        let next: Vec<Center> = groups.iter().filter(|g| !g.is_empty()).map(|g| center_of(bins, g)).collect();
        let moved = next.len() != centers.len()
            || next.iter().zip(centers.iter()).any(|(a, b)| dist2(a.lab, b.lab) > 0.01);
        *centers = next;
        if !moved {
            break;
        }
    }
}

fn merge(a: Center, b: Center) -> Center {
    let w = a.w + b.w;
    let mut c = Center { rgb: [0.0; 3], lab: [0.0; 3], w };
    for k in 0..3 {
        c.rgb[k] = (a.rgb[k] * a.w + b.rgb[k] * b.w) / w;
        c.lab[k] = (a.lab[k] * a.w + b.lab[k] * b.w) / w;
    }
    c
}

fn closest_pair(centers: &[Center]) -> Option<(usize, usize, f32)> {
    let mut best: Option<(usize, usize, f32)> = None;
    for i in 0..centers.len() {
        for j in i + 1..centers.len() {
            let d = dist2(centers[i].lab, centers[j].lab);
            if best.is_none_or(|b| d < b.2) {
                best = Some((i, j, d));
            }
        }
    }
    best
}

fn merge_pair(centers: &mut Vec<Center>, i: usize, j: usize) {
    let b = centers.remove(j);
    centers[i] = merge(centers[i], b);
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn build_palette(img: &RgbaImage, p: &PaletteParams) -> Palette {
    let bins = collect_bins(img, p.kind);
    if bins.is_empty() {
        return Palette { rgb: vec![], lab: vec![] };
    }
    let mut centers = if let Some(k) = p.fixed {
        let mut c = median_cut(&bins, k.clamp(1, 256) as usize);
        kmeans(&bins, &mut c, 10);
        c
    } else {
        let photo = p.kind == ImageKind::Photo;
        let mut c = median_cut(&bins, if photo { 96 } else { 48 });
        kmeans(&bins, &mut c, 4);
        let thresh = if photo { lerp(12.0, 4.0, p.detail) } else { lerp(14.0, 5.0, p.detail) };
        while let Some((i, j, d)) = closest_pair(&c) {
            if d >= thresh * thresh {
                break;
            }
            merge_pair(&mut c, i, j);
        }
        let total: f32 = c.iter().map(|c| c.w).sum();
        let min_share = if photo { lerp(0.004, 0.0004, p.detail) } else { lerp(0.0015, 0.0001, p.detail) };
        loop {
            if c.len() <= 1 {
                break;
            }
            let Some((si, _)) = c.iter().enumerate().filter(|(_, x)| x.w / total < min_share).min_by(|a, b| a.1.w.total_cmp(&b.1.w)) else {
                break;
            };
            let small = c.remove(si);
            let ni = nearest(&c, small.lab);
            c[ni] = merge(c[ni], small);
        }
        while c.len() > p.max_colors.max(1) as usize {
            let (i, j, _) = closest_pair(&c).unwrap();
            merge_pair(&mut c, i, j);
        }
        kmeans(&bins, &mut c, 4);
        c
    };
    centers.sort_by(|a, b| b.w.total_cmp(&a.w));
    let rgb: Vec<[u8; 3]> = centers.iter().map(|c| round_rgb(c.rgb)).collect();
    let lab = rgb.iter().map(|&c| rgb_to_lab(c)).collect();
    Palette { rgb, lab }
}

/// Nearest-palette label per pixel (transparent pixels → `TRANSPARENT`).
pub fn assign(img: &RgbaImage, pal: &Palette) -> Vec<u16> {
    let mut cache: HashMap<u32, u16> = HashMap::new();
    img.pixels()
        .map(|p| {
            let p = p.0;
            if p[3] < ALPHA_CUTOFF || pal.lab.is_empty() {
                return TRANSPARENT;
            }
            let key = ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
            *cache.entry(key).or_insert_with(|| {
                let lab = rgb_to_lab([p[0], p[1], p[2]]);
                let mut best = 0u16;
                let mut bd = f32::MAX;
                for (i, c) in pal.lab.iter().enumerate() {
                    let d = dist2(*c, lab);
                    if d < bd {
                        bd = d;
                        best = i as u16;
                    }
                }
                best
            })
        })
        .collect()
}

/// Remove colour mottling where a flat area sits between two palette colours.
///
/// A pixel only slightly closer to its own colour than to the colour most of
/// its 8 neighbours have joins the neighbours. Genuine detail is clearly
/// closer to its own colour and is left alone.
pub fn coherence_filter(img: &RgbaImage, labels: &mut [u16], pal: &Palette, passes: usize) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 3 || h < 3 || pal.lab.len() < 2 {
        return;
    }
    let raw = img.as_raw();
    let lab: Vec<Lab> = (0..w * h).map(|i| rgb_to_lab([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]])).collect();
    let mut out = labels.to_vec();
    for _ in 0..passes {
        let mut changed = false;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                let own = labels[i];
                if own == TRANSPARENT {
                    continue;
                }
                let mut counts: [(u16, u8); 8] = [(u16::MAX, 0); 8];
                for (dx, dy) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let l = labels[((y as i64 + dy) as usize) * w + (x as i64 + dx) as usize];
                    if let Some(c) = counts.iter_mut().find(|c| c.0 == l || c.1 == 0) {
                        c.0 = l;
                        c.1 += 1;
                    }
                }
                let (m, n) = counts.iter().copied().max_by_key(|c| c.1).unwrap();
                if n < 5 || m == own || m == TRANSPARENT {
                    continue;
                }
                let (lo, lm) = (pal.lab[own as usize], pal.lab[m as usize]);
                let sep = dist2(lo, lm).sqrt();
                // Mottling only happens between close colours; real edges are left alone.
                if sep > 14.0 {
                    continue;
                }
                if dist2(lab[i], lm).sqrt() - dist2(lab[i], lo).sqrt() < 0.5 * sep {
                    out[i] = m;
                    changed = true;
                }
            }
        }
        labels.copy_from_slice(&out);
        if !changed {
            break;
        }
    }
}

/// Residual-driven palette refinement (automatic palettes only).
///
/// Sizeable flat areas whose pixels sit far from their assigned palette colour
/// are design elements the palette missed — typically low-contrast motifs that
/// lost a merge against a dominant neighbour. Their mean colours join the
/// palette. Anti-aliased and textured pixels are ignored so edges and noise
/// never add colours.
pub fn refine_palette(img: &RgbaImage, mut pal: Palette, detail: f32, cap: usize) -> Palette {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 3 || h < 3 || pal.lab.is_empty() {
        return pal;
    }
    let raw = img.as_raw();
    let lab: Vec<Lab> = (0..w * h).map(|i| rgb_to_lab([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]])).collect();
    // Flat, opaque, non-blend pixels: the only ones whose colour is trusted.
    let mut flat = vec![false; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            if raw[i * 4 + 3] < ALPHA_CUTOFF {
                continue;
            }
            let near = [i - 1, i + 1, i - w, i + w].iter().all(|&j| raw[j * 4 + 3] >= ALPHA_CUTOFF && dist2(lab[i], lab[j]) < 6.0 * 6.0);
            flat[i] = near && !is_blend_pixel(img, x as i64, y as i64);
        }
    }
    // Small areas must stand out clearly; large ones may be subtle (a faint
    // background motif is still a design element).
    let thresh = lerp(10.0, 5.0, detail);
    let faint = 4.0f32;
    let min_area = ((w * h) as f32 * 0.0004).max(24.0) as usize;
    let needed = |n: usize| {
        let t = ((n as f32 / min_area as f32).ln() / 25f32.ln()).clamp(0.0, 1.0);
        thresh + (faint - thresh) * t
    };
    let added_max = 12usize;
    let mut added = 0;
    for _ in 0..2 {
        let labels = assign(img, &pal);
        let bad: Vec<bool> = (0..w * h)
            .map(|i| flat[i] && labels[i] != TRANSPARENT && dist2(lab[i], pal.lab[labels[i] as usize]) > faint * faint)
            .collect();
        // Connected areas of badly represented pixels.
        let mut seen = vec![false; w * h];
        let mut comps: Vec<(usize, Lab, [f32; 3], u16)> = Vec::new();
        let mut stack = Vec::new();
        for s in 0..w * h {
            if !bad[s] || seen[s] {
                continue;
            }
            seen[s] = true;
            stack.push(s);
            let (mut n, mut sl, mut sr) = (0usize, [0f64; 3], [0f64; 3]);
            let mut perim = 0usize;
            let base = labels[s];
            while let Some(i) = stack.pop() {
                n += 1;
                for k in 0..3 {
                    sl[k] += lab[i][k] as f64;
                    sr[k] += raw[i * 4 + k] as f64;
                }
                let (x, y) = (i % w, i / w);
                for (ok, j) in [(x > 0, i.wrapping_sub(1)), (x + 1 < w, i + 1), (y > 0, i.wrapping_sub(w)), (y + 1 < h, i + w)] {
                    if !ok || !bad[j] {
                        perim += 1;
                    } else if !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            // Solid shapes only: paper grain and noise form thin, ragged scatter.
            let thickness = 2.0 * n as f32 / perim.max(1) as f32;
            if n >= min_area && thickness >= 3.0 {
                let m = |v: [f64; 3]| [(v[0] / n as f64) as f32, (v[1] / n as f64) as f32, (v[2] / n as f64) as f32];
                comps.push((n, m(sl), m(sr), base));
            }
        }
        comps.sort_by(|a, b| b.0.cmp(&a.0));
        let before = added;
        for (n, l, rgb, base) in comps {
            if added >= added_max || pal.rgb.len() >= cap {
                break;
            }
            let need = needed(n);
            if dist2(l, pal.lab[base as usize]).sqrt() >= need && pal.lab.iter().all(|&p| dist2(p, l) > (0.8 * need).powi(2)) {
                let c = round_rgb(rgb);
                pal.rgb.push(c);
                pal.lab.push(rgb_to_lab(c));
                added += 1;
            }
        }
        if added == before {
            break;
        }
    }
    pal
}
