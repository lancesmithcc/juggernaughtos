//! Segmentation: label map → 4-connected regions, with noise cleanup.
//!
//! Specks below the minimum area and anti-aliasing slivers (thin regions whose
//! colour is a blend of their two main neighbours) are dissolved pixel by
//! pixel into whichever neighbouring region matches each pixel's colour best.

use crate::color::{dist2, rgb_to_lab, segment_distance};
use crate::quantize::{Palette, ALPHA_CUTOFF, TRANSPARENT};
use image::RgbaImage;

#[derive(Clone, Debug)]
pub struct Region {
    pub label: u16,
    pub area: u32,
    pub touches_border: bool,
}

pub struct Segmentation {
    pub width: usize,
    pub height: usize,
    /// Region id per pixel.
    pub map: Vec<u32>,
    pub regions: Vec<Region>,
}

pub struct SegmentParams {
    /// Regions smaller than this (in working pixels) are dissolved.
    pub min_area: f32,
    /// Regions thinner than this (2·area/perimeter) are sliver candidates.
    pub sliver_thickness: f32,
    pub remove_background: bool,
    /// Compact, high-contrast blobs at least this big are dots, not specks.
    pub dot_min_area: f32,
}

pub fn components(w: usize, h: usize, labels: &[u16]) -> Segmentation {
    let mut map = vec![u32::MAX; w * h];
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if map[start] != u32::MAX {
            continue;
        }
        let id = regions.len() as u32;
        let label = labels[start];
        let mut area = 0u32;
        let mut border = false;
        map[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = (i % w, i / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                border = true;
            }
            let mut visit = |j: usize| {
                if map[j] == u32::MAX && labels[j] == label {
                    map[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        regions.push(Region { label, area, touches_border: border });
    }
    Segmentation { width: w, height: h, map, regions }
}

/// Sorted (a, b, shared_length) for every adjacent region pair, plus perimeters.
fn adjacency(seg: &Segmentation) -> (Vec<(u32, u32, u32)>, Vec<u32>) {
    let (w, h) = (seg.width, seg.height);
    let mut perim = vec![0u32; seg.regions.len()];
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let a = seg.map[y * w + x];
            if x == 0 || x == w - 1 {
                perim[a as usize] += 1;
            }
            if y == 0 || y == h - 1 {
                perim[a as usize] += 1;
            }
            if x + 1 < w {
                let b = seg.map[y * w + x + 1];
                if a != b {
                    perim[a as usize] += 1;
                    perim[b as usize] += 1;
                    pairs.push((a.min(b), a.max(b)));
                }
            }
            if y + 1 < h {
                let b = seg.map[(y + 1) * w + x];
                if a != b {
                    perim[a as usize] += 1;
                    perim[b as usize] += 1;
                    pairs.push((a.min(b), a.max(b)));
                }
            }
        }
    }
    pairs.sort_unstable();
    let mut out: Vec<(u32, u32, u32)> = Vec::new();
    for p in pairs {
        match out.last_mut() {
            Some(l) if l.0 == p.0 && l.1 == p.1 => l.2 += 1,
            _ => out.push((p.0, p.1, 1)),
        }
    }
    (out, perim)
}

/// Round, solid, clearly contrasting blobs (bead borders, dotted rules, the
/// dot of an i) are design elements even when small: true for each speck
/// that is such a dot.
fn speck_dots(seg: &Segmentation, pal: &Palette, perim: &[u32], nbrs: &[Vec<(u32, u32)>], min_area: f32, dot_min_area: f32) -> Vec<bool> {
    let is_dot = |r: usize| {
        let reg = &seg.regions[r];
        if reg.label == TRANSPARENT || (reg.area as f32) < dot_min_area {
            return false;
        }
        let p = perim[r].max(1) as f32;
        let compact = 4.0 * std::f32::consts::PI * reg.area as f32 / (p * p);
        let surround = nbrs[r].iter().max_by_key(|&&(o, len)| (len, std::cmp::Reverse(o))).map(|&(o, _)| seg.regions[o as usize].label);
        let contrast = match surround {
            Some(l) if l != TRANSPARENT => dist2(pal.lab[reg.label as usize], pal.lab[l as usize]).sqrt(),
            _ => 100.0,
        };
        compact >= 0.45 && contrast >= 15.0
    };
    (0..seg.regions.len()).map(|r| (seg.regions[r].area as f32) < min_area && is_dot(r)).collect()
}

/// Split anti-aliasing specks between their neighbours, pixel by pixel.
///
/// A speck whose colour is a blend of two neighbouring colours is the soft
/// edge between them (typically a narrow gap between two strokes, like the
/// spaces between the rays of a star). Merging it whole into one neighbour
/// closes the gap; instead each pixel joins the neighbour colour it is
/// closest to, so the light middle of a gap stays light.
fn split_blend_specks(img: &RgbaImage, labels: &mut [u16], seg: &Segmentation, pal: &Palette, min_area: f32, dot_min_area: f32) -> bool {
    let n = seg.regions.len();
    if n <= 1 || !seg.regions.iter().any(|r| (r.area as f32) < min_area) {
        return false;
    }
    let (adj, perim) = adjacency(seg);
    let mut nbrs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    for &(a, b, len) in &adj {
        nbrs[a as usize].push((b, len));
        nbrs[b as usize].push((a, len));
    }
    let dots = speck_dots(seg, pal, &perim, &nbrs, min_area, dot_min_area);
    // Candidate colours per blend speck (empty = leave to merge_specks).
    let mut cands: Vec<Vec<u16>> = vec![Vec::new(); n];
    let mut any = false;
    for r in 0..n {
        let reg = &seg.regions[r];
        // A speck touching a dot, or enclosing a small core, is the soft rim
        // of a bead: leave it to merge_specks, which keeps rim and core together.
        if reg.label == TRANSPARENT
            || (reg.area as f32) >= min_area
            || dots[r]
            || nbrs[r].iter().any(|&(o, _)| {
                let ou = o as usize;
                dots[ou]
                    || (seg.regions[ou].label != TRANSPARENT
                        && (seg.regions[ou].area as f32) < min_area
                        && nbrs[ou].iter().all(|&(q, _)| q as usize == r))
            })
        {
            continue;
        }
        let mut ls: Vec<u16> = nbrs[r]
            .iter()
            .map(|&(o, _)| seg.regions[o as usize].label)
            .filter(|&l| l != TRANSPARENT && l != reg.label)
            .collect();
        ls.sort_unstable();
        ls.dedup();
        let c = pal.lab[reg.label as usize];
        let blend = ls.iter().enumerate().any(|(i, &a)| {
            ls[i + 1..].iter().any(|&b| {
                let (la, lb) = (pal.lab[a as usize], pal.lab[b as usize]);
                let span = dist2(la, lb).sqrt();
                let (d, t) = segment_distance(c, la, lb);
                span > 40.0 && (0.15..=0.85).contains(&t) && d < (0.3 * span).max(10.0)
            })
        });
        if blend {
            cands[r] = ls;
            any = true;
        }
    }
    if !any {
        return false;
    }
    let raw = img.as_raw();
    let before = labels.to_vec();
    for (i, l) in labels.iter_mut().enumerate() {
        let cs = &cands[seg.map[i] as usize];
        if cs.is_empty() {
            continue;
        }
        let lab = rgb_to_lab([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]]);
        *l = *cs.iter().min_by(|&&a, &&b| dist2(lab, pal.lab[a as usize]).total_cmp(&dist2(lab, pal.lab[b as usize]))).unwrap();
    }
    // Grain makes single pixels flip: a split pixel that disagrees with at
    // least 3 of its 4 neighbours, which all agree, follows them. One-pixel
    // lines keep two same-coloured neighbours and survive.
    let (w, h) = (seg.width, seg.height);
    let snap = labels.to_vec();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            if cands[seg.map[i] as usize].is_empty() {
                continue;
            }
            let ns = [snap[i - 1], snap[i + 1], snap[i - w], snap[i + w]];
            for &m in &ns {
                if m != snap[i] && m != TRANSPARENT && ns.iter().filter(|&&o| o == m).count() >= 3 {
                    labels[i] = m;
                    break;
                }
            }
        }
    }
    labels != before.as_slice()
}

/// Merge regions below `min_area` into their most similar-coloured
/// neighbour (smaller neighbours included, so a stroke broken into small
/// pieces of two close colours fuses back together instead of vanishing).
fn merge_specks(labels: &mut [u16], seg: &Segmentation, pal: &Palette, min_area: f32, dot_min_area: f32) -> bool {
    let n = seg.regions.len();
    if n <= 1 {
        return false;
    }
    if !seg.regions.iter().any(|r| (r.area as f32) < min_area) {
        return false;
    }
    let (adj, perim) = adjacency(seg);
    let mut nbrs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    for &(a, b, len) in &adj {
        nbrs[a as usize].push((b, len));
        nbrs[b as usize].push((a, len));
    }
    // Dots are protected: they are never absorbed, and a speck merging into a
    // dot (e.g. the anti-aliased ring round a faint bead) takes the dot's colour.
    let mut protected = speck_dots(seg, pal, &perim, &nbrs, min_area, dot_min_area);
    let mut order: Vec<u32> = (0..n as u32)
        .filter(|&r| (seg.regions[r as usize].area as f32) < min_area && !protected[r as usize])
        .collect();
    if order.is_empty() {
        return false;
    }
    order.sort_by_key(|&r| (seg.regions[r as usize].area, r));
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut area: Vec<u64> = seg.regions.iter().map(|r| r.area as u64).collect();
    let label: Vec<u16> = seg.regions.iter().map(|r| r.label).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let colour_gap = |a: u16, b: u16| match (a == TRANSPARENT, b == TRANSPARENT) {
        (true, true) => 0.0,
        (false, false) => dist2(pal.lab[a as usize], pal.lab[b as usize]).sqrt(),
        _ => 1e6,
    };
    let mut any = false;
    for r in order {
        let root = find(&mut parent, r);
        if area[root as usize] as f32 >= min_area || protected[root as usize] {
            continue;
        }
        let mut best: Option<(u32, f32, u32)> = None;
        for i in 0..nbrs[root as usize].len() {
            let (o, len) = nbrs[root as usize][i];
            let ro = find(&mut parent, o);
            if ro == root {
                continue;
            }
            let gap = colour_gap(label[root as usize], label[ro as usize]);
            if best.map_or(true, |b| gap < b.1 || (gap == b.1 && len > b.2)) {
                best = Some((ro, gap, len));
            }
        }
        let Some((target, _, _)) = best else { continue };
        // The larger side keeps its colour (a dot always does); neighbour
        // lists follow the root.
        let (big, small) = if protected[target as usize] || area[target as usize] >= area[root as usize] {
            (target, root)
        } else {
            (root, target)
        };
        protected[big as usize] |= protected[small as usize];
        parent[small as usize] = big;
        area[big as usize] += area[small as usize];
        let moved = std::mem::take(&mut nbrs[small as usize]);
        nbrs[big as usize].extend(moved);
        any = true;
    }
    if any {
        for (i, l) in labels.iter_mut().enumerate() {
            let root = find(&mut parent, seg.map[i]);
            *l = label[root as usize];
        }
    }
    any
}

/// Re-join one area that the palette split in two.
///
/// When a flat area's colour falls between two palette entries it ends up
/// as a patchwork of both. Neighbouring regions whose *actual* average pixel
/// colours are far closer than their palette colours are such a split, and
/// are merged (the larger part's colour wins).
fn merge_palette_splits(img: &RgbaImage, labels: &mut [u16], seg: &Segmentation, pal: &Palette) -> bool {
    let n = seg.regions.len();
    if n <= 1 {
        return false;
    }
    let raw = img.as_raw();
    let mut sum = vec![[0f64; 3]; n];
    for (i, &r) in seg.map.iter().enumerate() {
        let l = rgb_to_lab([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]]);
        let s = &mut sum[r as usize];
        for k in 0..3 {
            s[k] += l[k] as f64;
        }
    }
    let mean: Vec<[f32; 3]> = sum
        .iter()
        .zip(&seg.regions)
        .map(|(s, r)| {
            let a = r.area.max(1) as f64;
            [(s[0] / a) as f32, (s[1] / a) as f32, (s[2] / a) as f32]
        })
        .collect();
    let (adj, _) = adjacency(seg);
    let mut cands: Vec<(f32, u32, u32)> = Vec::new();
    for &(a, b, len) in &adj {
        let (la, lb) = (seg.regions[a as usize].label, seg.regions[b as usize].label);
        if la == TRANSPARENT || lb == TRANSPARENT || la == lb || len < 3 {
            continue;
        }
        let sep = dist2(pal.lab[la as usize], pal.lab[lb as usize]).sqrt();
        if sep > 25.0 {
            continue;
        }
        let ratio = dist2(mean[a as usize], mean[b as usize]).sqrt() / sep.max(1e-3);
        if ratio < 0.45 {
            cands.push((ratio, a, b));
        }
    }
    if cands.is_empty() {
        return false;
    }
    cands.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)).then(x.2.cmp(&y.2)));
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut area: Vec<u64> = seg.regions.iter().map(|r| r.area as u64).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    for (_, a, b) in cands {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra == rb {
            continue;
        }
        let (big, small) = if area[ra as usize] >= area[rb as usize] { (ra, rb) } else { (rb, ra) };
        parent[small as usize] = big;
        area[big as usize] += area[small as usize];
    }
    for (i, l) in labels.iter_mut().enumerate() {
        let root = find(&mut parent, seg.map[i]);
        *l = seg.regions[root as usize].label;
    }
    true
}

/// Merge thin or small regions into a near-identical neighbour.
///
/// A line split between two close colours, a blotch of paper grain, or a
/// fringe hugging an edge is a region with a low-contrast neighbour that is
/// too thin or too small to be a design element on its own. High-contrast
/// detail never qualifies, so real thin strokes and dots survive.
fn merge_weak(labels: &mut [u16], seg: &Segmentation, pal: &Palette, p: &SegmentParams) -> bool {
    const NEAR: f32 = 12.0;
    let n = seg.regions.len();
    if n <= 1 {
        return false;
    }
    let (adj, perim) = adjacency(seg);
    let mut nbrs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    for &(a, b, len) in &adj {
        nbrs[a as usize].push((b, len));
        nbrs[b as usize].push((a, len));
    }
    let weak = |r: usize| {
        let reg = &seg.regions[r];
        let thickness = 2.0 * reg.area as f32 / perim[r].max(1) as f32;
        reg.label != TRANSPARENT && (thickness < 1.4 * p.sliver_thickness || (reg.area as f32) < 6.0 * p.min_area)
    };
    let mut order: Vec<usize> = (0..n).filter(|&r| weak(r)).collect();
    order.sort_by_key(|&r| (seg.regions[r].area, r));
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut area: Vec<u64> = seg.regions.iter().map(|r| r.area as u64).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let mut any = false;
    for r in order {
        let root = find(&mut parent, r as u32);
        let lr = seg.regions[root as usize].label;
        // Contrast against the surround (longest-border neighbour): pieces of
        // one stroke differ little compared to how much they stand out.
        let surround = nbrs[r]
            .iter()
            .filter(|&&(o, _)| seg.regions[o as usize].label != TRANSPARENT)
            .max_by_key(|&&(o, len)| (len, std::cmp::Reverse(o)))
            .map(|&(o, _)| dist2(pal.lab[lr as usize], pal.lab[seg.regions[o as usize].label as usize]).sqrt())
            .unwrap_or(0.0);
        let near = NEAR.max((0.4 * surround).min(25.0));
        let mut best: Option<(u32, f32)> = None;
        for &(o, len) in &nbrs[r] {
            let ro = find(&mut parent, o);
            let lo = seg.regions[ro as usize].label;
            if ro == root || lo == TRANSPARENT || len < 2 {
                continue;
            }
            let gap = dist2(pal.lab[lr as usize], pal.lab[lo as usize]).sqrt();
            if gap < near && best.map_or(true, |b| gap < b.1) {
                best = Some((ro, gap));
            }
        }
        let Some((target, _)) = best else { continue };
        let (big, small) = if area[target as usize] >= area[root as usize] { (target, root) } else { (root, target) };
        parent[small as usize] = big;
        area[big as usize] += area[small as usize];
        any = true;
    }
    if any {
        for (i, l) in labels.iter_mut().enumerate() {
            let root = find(&mut parent, seg.map[i]);
            *l = seg.regions[root as usize].label;
        }
    }
    any
}

fn dissolve_pass(img: &RgbaImage, pal: &Palette, labels: &mut [u16], seg: &Segmentation, p: &SegmentParams) -> bool {
    let n = seg.regions.len();
    if n <= 1 {
        return false;
    }
    let (adj, perim) = adjacency(seg);
    let mut neighbours: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n];
    for &(a, b, len) in &adj {
        neighbours[a as usize].push((b, len));
        neighbours[b as usize].push((a, len));
    }
    let mut dissolve = vec![false; n];
    let mut any = false;
    for (r, reg) in seg.regions.iter().enumerate() {
        if neighbours[r].is_empty() {
            continue;
        }
        let mut kill = false;
        if reg.label != TRANSPARENT {
            let thickness = 2.0 * reg.area as f32 / perim[r].max(1) as f32;
            if thickness < p.sliver_thickness {
                let mut nb: Vec<(u32, u32)> = neighbours[r]
                    .iter()
                    .copied()
                    .filter(|&(o, _)| seg.regions[o as usize].label != TRANSPARENT)
                    .collect();
                nb.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                // A true AA sliver runs *between* two regions, bordering both.
                if nb.len() >= 2 && nb[1].1 as f32 >= 0.3 * nb[0].1 as f32 {
                    let c = pal.lab[reg.label as usize];
                    let a = pal.lab[seg.regions[nb[0].0 as usize].label as usize];
                    let b = pal.lab[seg.regions[nb[1].0 as usize].label as usize];
                    let span = dist2(a, b).sqrt();
                    let (d, t) = segment_distance(c, a, b);
                    // A blend sits well inside the a→b range; near either end it is
                    // a variant of that neighbour (e.g. a line changing shade), not AA.
                    kill = span > 12.0 && (0.2..=0.8).contains(&t) && d < (0.3 * span).max(10.0);
                }
            }
        }
        if kill {
            dissolve[r] = true;
            any = true;
        }
    }
    if !any {
        return false;
    }

    // Grow surviving regions into dissolved pixels, best colour match first.
    const NONE: u16 = TRANSPARENT - 1;
    let (w, h) = (seg.width, seg.height);
    let mut cur: Vec<u16> = (0..w * h)
        .map(|i| if dissolve[seg.map[i] as usize] { NONE } else { labels[i] })
        .collect();
    let mut queued = vec![false; w * h];
    let nbrs = |i: usize| {
        let (x, y) = (i % w, i / w);
        let mut v = [usize::MAX; 4];
        if x > 0 {
            v[0] = i - 1;
        }
        if x + 1 < w {
            v[1] = i + 1;
        }
        if y > 0 {
            v[2] = i - w;
        }
        if y + 1 < h {
            v[3] = i + w;
        }
        v
    };
    let mut frontier: Vec<usize> = Vec::new();
    for i in 0..w * h {
        if cur[i] == NONE && nbrs(i).iter().any(|&j| j != usize::MAX && cur[j] != NONE) {
            frontier.push(i);
            queued[i] = true;
        }
    }
    while !frontier.is_empty() {
        let mut updates = Vec::with_capacity(frontier.len());
        for &i in &frontier {
            let px = img.as_raw();
            let o = i * 4;
            let transparent_px = px[o + 3] < ALPHA_CUTOFF;
            let lab = rgb_to_lab([px[o], px[o + 1], px[o + 2]]);
            let mut best = NONE;
            let mut bd = f32::MAX;
            for j in nbrs(i) {
                if j == usize::MAX || cur[j] == NONE {
                    continue;
                }
                let l = cur[j];
                let d = match (l == TRANSPARENT, transparent_px) {
                    (true, true) => -1.0,
                    (true, false) => 1e9,
                    (false, true) => 0.0,
                    (false, false) => dist2(lab, pal.lab[l as usize]),
                };
                if d < bd {
                    bd = d;
                    best = l;
                }
            }
            updates.push((i, best));
        }
        let mut next = Vec::new();
        for (i, l) in updates {
            cur[i] = l;
            for j in nbrs(i) {
                if j != usize::MAX && cur[j] == NONE && !queued[j] {
                    queued[j] = true;
                    next.push(j);
                }
            }
        }
        frontier = next;
    }
    for i in 0..w * h {
        if cur[i] != NONE {
            labels[i] = cur[i];
        }
    }
    true
}

/// Keep thin diagonal strokes 4-connected: where a sparse colour touches
/// itself only corner-to-corner, widen it by one pixel at the step.
fn bridge_diagonals(labels: &mut [u16], w: usize, h: usize) {
    if w < 2 || h < 2 {
        return;
    }
    let share = |labels: &[u16], x: usize, y: usize, l: u16| {
        let (x0, x1) = (x.saturating_sub(3), (x + 4).min(w - 1));
        let (y0, y1) = (y.saturating_sub(3), (y + 4).min(h - 1));
        let mut n = 0;
        let mut t = 0;
        for yy in y0..=y1 {
            for xx in x0..=x1 {
                t += 1;
                if labels[yy * w + xx] == l {
                    n += 1;
                }
            }
        }
        n as f32 / t as f32
    };
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let (i_tl, i_tr, i_bl, i_br) = (y * w + x, y * w + x + 1, (y + 1) * w + x, (y + 1) * w + x + 1);
            let (tl, tr, bl, br) = (labels[i_tl], labels[i_tr], labels[i_bl], labels[i_br]);
            let main = tl == br && tr != tl && bl != tl && tl != TRANSPARENT;
            let anti = tr == bl && tl != tr && br != tr && tr != TRANSPARENT;
            let pick = match (main, anti) {
                (true, true) => Some(if share(labels, x, y, tl) <= share(labels, x, y, tr) { (tl, i_tr) } else { (tr, i_tl) }),
                (true, false) => Some((tl, i_tr)),
                (false, true) => Some((tr, i_tl)),
                _ => None,
            };
            if let Some((l, target)) = pick {
                if share(labels, x, y, l) < 0.4 {
                    labels[target] = l;
                }
            }
        }
    }
}

/// Debug aid: write the label map as an image when VV_DEBUG_DIR is set.
fn dump(name: &str, labels: &[u16], w: usize, h: usize, pal: &Palette) {
    let Ok(dir) = std::env::var("VV_DEBUG_DIR") else { return };
    let mut im = RgbaImage::new(w as u32, h as u32);
    for (px, &l) in im.pixels_mut().zip(labels) {
        if l != TRANSPARENT {
            let c = pal.rgb[l as usize];
            px.0 = [c[0], c[1], c[2], 255];
        }
    }
    let _ = im.save(std::path::Path::new(&dir).join(name));
}

pub fn segment(bridge: bool, img: &RgbaImage, pal: &Palette, mut labels: Vec<u16>, p: &SegmentParams) -> Segmentation {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if bridge {
        bridge_diagonals(&mut labels, w, h);
    }
    let mut seg = components(w, h, &labels);
    // Photos are posterised anyway; splitting their specks only adds nodes.
    if bridge && split_blend_specks(img, &mut labels, &seg, pal, p.min_area, p.dot_min_area) {
        seg = components(w, h, &labels);
    }
    if merge_specks(&mut labels, &seg, pal, p.min_area, p.dot_min_area) {
        seg = components(w, h, &labels);
    }
    dump("s1-specks.png", &labels, w, h, pal);
    for _ in 0..3 {
        if !dissolve_pass(img, pal, &mut labels, &seg, p) {
            break;
        }
        seg = components(w, h, &labels);
    }
    dump("s2-slivers.png", &labels, w, h, pal);
    if merge_palette_splits(img, &mut labels, &seg, pal) {
        seg = components(w, h, &labels);
    }
    dump("s3-splits.png", &labels, w, h, pal);
    if merge_weak(&mut labels, &seg, pal, p) {
        seg = components(w, h, &labels);
    }
    dump("s4-weak.png", &labels, w, h, pal);
    if merge_specks(&mut labels, &seg, pal, p.min_area, p.dot_min_area) {
        seg = components(w, h, &labels);
    }
    if p.remove_background {
        // The dominant colour around the border is the background.
        let mut counts = vec![0u32; pal.rgb.len()];
        let mut total = 0u32;
        let mut tally = |i: usize| {
            if labels[i] != TRANSPARENT {
                counts[labels[i] as usize] += 1;
            }
            total += 1;
        };
        for x in 0..w {
            tally(x);
            tally((h - 1) * w + x);
        }
        for y in 1..h.saturating_sub(1) {
            tally(y * w);
            tally(y * w + w - 1);
        }
        if let Some((bg, &c)) = counts.iter().enumerate().max_by_key(|(i, c)| (**c, usize::MAX - *i)) {
            if c as f32 >= 0.3 * total as f32 {
                let mut changed = false;
                for i in 0..w * h {
                    let r = &seg.regions[seg.map[i] as usize];
                    if r.touches_border && r.label as usize == bg {
                        labels[i] = TRANSPARENT;
                        changed = true;
                    }
                }
                if changed {
                    seg = components(w, h, &labels);
                }
            }
        }
    }
    seg
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const BROWN: u16 = 0;
    const CREAM: u16 = 1;
    const TAN: u16 = 2;

    fn palette() -> Palette {
        Palette::from_hex(&["#976f2a".into(), "#faf4e1".into(), "#c8b286".into()])
    }

    #[test]
    fn blend_speck_in_a_gap_splits_by_pixel_colour() {
        // Brown above and below a 3-row gap that opens into cream on the right.
        // The gap was quantised to tan, but its middle row is really light.
        let (w, h) = (30usize, 12usize);
        let img = RgbaImage::from_fn(w as u32, h as u32, |x, y| match (x, y) {
            (20.., _) => Rgba([250, 244, 225, 255]),
            (_, 0..=3) | (_, 7..) => Rgba([151, 111, 42, 255]),
            (_, 5) => Rgba([240, 230, 205, 255]),
            _ => Rgba([165, 128, 60, 255]),
        });
        let mut labels: Vec<u16> = (0..w * h)
            .map(|i| match (i % w, i / w) {
                (20.., _) => CREAM,
                (_, 0..=3) | (_, 7..) => BROWN,
                _ => TAN,
            })
            .collect();
        let seg = components(w, h, &labels);
        assert!(split_blend_specks(&img, &mut labels, &seg, &palette(), 70.0, 4.0));
        for x in 1..20 {
            assert_eq!(labels[5 * w + x], CREAM, "gap middle at x={x}");
            assert_eq!(labels[4 * w + x], BROWN, "gap edge at x={x}");
            assert_eq!(labels[6 * w + x], BROWN, "gap edge at x={x}");
        }
    }

    #[test]
    fn rim_of_a_small_dot_is_not_split() {
        // A brown 3×3 bead core inside a tan anti-aliased rim on cream.
        let (w, h) = (15usize, 15usize);
        let ring = |x: usize, y: usize| (5..10).contains(&x) && (5..10).contains(&y);
        let core = |x: usize, y: usize| (6..9).contains(&x) && (6..9).contains(&y);
        let img = RgbaImage::from_fn(w as u32, h as u32, |x, y| {
            let (x, y) = (x as usize, y as usize);
            if core(x, y) {
                Rgba([151, 111, 42, 255])
            } else if ring(x, y) {
                Rgba([235, 222, 195, 255])
            } else {
                Rgba([250, 244, 225, 255])
            }
        });
        let mut labels: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if core(x, y) {
                    BROWN
                } else if ring(x, y) {
                    TAN
                } else {
                    CREAM
                }
            })
            .collect();
        let before = labels.clone();
        let seg = components(w, h, &labels);
        assert!(!split_blend_specks(&img, &mut labels, &seg, &palette(), 50.0, 4.0));
        assert_eq!(labels, before);
    }
}
