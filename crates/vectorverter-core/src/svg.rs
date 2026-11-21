//! Shape assembly (draw order, stacked hole filling) and SVG serialisation.

use crate::boundary::{region_loops, Chain, ChainRef};
use crate::color::hex;
use crate::fit::{Fitted, Seg, P};
use crate::segment::Segmentation;
use crate::ShapeMode;
use std::collections::VecDeque;
use std::fmt::Write;

/// How far (in source pixels) a lower shape extends under the shape above it.
const TUCK: f64 = 0.6;

pub struct Assembled {
    pub svg: String,
    pub paths: usize,
    pub nodes: usize,
    pub colors: usize,
}

struct Shape {
    region: u32,
    color: [u8; 3],
    loops: Vec<Vec<ChainRef>>,
}

fn loop_area2(chains: &[Chain], lp: &[ChainRef]) -> i64 {
    let mut a = 0i64;
    for r in lp {
        let pts = &chains[r.chain].pts;
        let mut add = |p: (i32, i32), q: (i32, i32)| a += p.0 as i64 * q.1 as i64 - q.0 as i64 * p.1 as i64;
        if r.reversed {
            for w in pts.windows(2).rev() {
                add(w[1], w[0]);
            }
        } else {
            for w in pts.windows(2) {
                add(w[0], w[1]);
            }
        }
        if chains[r.chain].closed {
            let (f, l) = (pts[0], pts[pts.len() - 1]);
            if r.reversed { add(f, l) } else { add(l, f) }
        }
    }
    a
}

fn fmt_num(v: f64, decimals: usize, out: &mut String) {
    let s = format!("{:.*}", decimals, v);
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
    out.push_str(if s == "-0" { "0" } else { s });
}

fn write_pt(p: P, scale: f64, dec: usize, out: &mut String) {
    fmt_num(p.x / scale, dec, out);
    out.push(' ');
    fmt_num(p.y / scale, dec, out);
}

/// Append one closed loop to path data; returns node count.
/// `tuck[i]` pushes chain i of the loop `delta` under the neighbouring shape.
#[allow(clippy::too_many_arguments)]
fn write_loop(
    chains: &[Chain],
    fitted: &[Option<Fitted>],
    lp: &[ChainRef],
    tuck: &[bool],
    delta: f64,
    scale: f64,
    dec: usize,
    d: &mut String,
) -> usize {
    let mut segs: Vec<Seg> = Vec::new();
    let mut start: Option<P> = None;
    let mut pen = P::default();
    for (r, &t) in lp.iter().zip(tuck) {
        let Some(f) = fitted[r.chain].as_ref() else { continue };
        let mut f = if r.reversed { f.reversed() } else { f.clone() };
        if t {
            f = f.offset_left(delta, chains[r.chain].closed);
        }
        match start {
            None => start = Some(f.start),
            Some(_) if (f.start.x - pen.x).abs() > 1e-6 || (f.start.y - pen.y).abs() > 1e-6 => segs.push(Seg::Line(f.start)),
            _ => {}
        }
        pen = f.segs.last().map_or(f.start, |s| s.end());
        segs.extend(f.segs);
    }
    let Some(start) = start else { return 0 };
    // Z closes the loop, so a final straight line home is redundant.
    if matches!(segs.last(), Some(Seg::Line(p)) if (p.x - start.x).abs() < 1e-9 && (p.y - start.y).abs() < 1e-9) {
        segs.pop();
    }
    d.push('M');
    write_pt(start, scale, dec, d);
    for s in &segs {
        match *s {
            Seg::Line(p) => {
                d.push('L');
                write_pt(p, scale, dec, d);
            }
            Seg::Cubic(a, b, p) => {
                d.push('C');
                write_pt(a, scale, dec, d);
                d.push(' ');
                write_pt(b, scale, dec, d);
                d.push(' ');
                write_pt(p, scale, dec, d);
            }
        }
    }
    d.push('Z');
    segs.len().max(1)
}

/// Centre of a small shape that is a dot, if it is one: near-square bounding
/// box and an area that fills it like an ellipse (≈ π/4), not like a square.
fn round_dot(chains: &[Chain], lp: &[ChainRef], area: f64) -> Option<P> {
    let pts: Vec<(i32, i32)> = lp.iter().flat_map(|cr| chains[cr.chain].pts.iter().copied()).collect();
    if pts.len() < 8 {
        return None;
    }
    let (x0, x1) = (pts.iter().map(|p| p.0).min()?, pts.iter().map(|p| p.0).max()?);
    let (y0, y1) = (pts.iter().map(|p| p.1).min()?, pts.iter().map(|p| p.1).max()?);
    let (w, h) = ((x1 - x0) as f64, (y1 - y0) as f64);
    if w < 2.0 || h < 2.0 || w.max(h) / w.min(h) > 1.4 {
        return None;
    }
    let fill = area / (std::f64::consts::FRAC_PI_4 * w * h);
    (0.75..=1.1).contains(&fill).then(|| P::new((x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0))
}

fn write_circle(c: P, r: f64, scale: f64, dec: usize, d: &mut String) {
    const K: f64 = 0.552_284_75;
    let k = r * K;
    let pt = |x: f64, y: f64, d: &mut String| write_pt(P::new(x, y), scale, dec, d);
    d.push('M');
    pt(c.x + r, c.y, d);
    for (b, c1, c2) in [
        ((c.x, c.y + r), (c.x + r, c.y + k), (c.x + k, c.y + r)),
        ((c.x - r, c.y), (c.x - k, c.y + r), (c.x - r, c.y + k)),
        ((c.x, c.y - r), (c.x - r, c.y - k), (c.x - k, c.y - r)),
        ((c.x + r, c.y), (c.x + k, c.y - r), (c.x + r, c.y - k)),
    ] {
        d.push('C');
        pt(c1.0, c1.1, d);
        d.push(' ');
        pt(c2.0, c2.1, d);
        d.push(' ');
        pt(b.0, b.1, d);
    }
    d.push('Z');
}

#[allow(clippy::too_many_arguments)]
pub fn assemble(
    seg: &Segmentation,
    palette: &[[u8; 3]],
    chains: &[Chain],
    fitted: &[Option<Fitted>],
    emitted: &[bool],
    mode: ShapeMode,
    optimize: bool,
    scale: f64,
    round_dots: bool,
    out_w: u32,
    out_h: u32,
) -> Assembled {
    let n = seg.regions.len();
    let outside = n as u32;
    let mut by_region: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n + 1];
    for (ci, c) in chains.iter().enumerate() {
        for r in [c.right, c.left] {
            if r != outside && emitted[r as usize] {
                by_region[r as usize].push(ci);
            }
        }
        adj[c.right as usize].push(c.left);
        adj[c.left as usize].push(c.right);
    }
    for a in adj.iter_mut() {
        a.sort_unstable();
        a.dedup();
    }

    // Paint order is by colour layer (most-used colour at the bottom), so
    // every colour becomes a single compound path. Shapes of one colour never
    // share an edge, so layers give every shared edge a clear lower side.
    let label_of = |r: u32| seg.regions[r as usize].label as usize;
    let mut color_area = vec![0u64; palette.len()];
    for r in (0..n as u32).filter(|&r| emitted[r as usize]) {
        color_area[label_of(r)] += seg.regions[r as usize].area as u64;
    }
    let mut layers: Vec<usize> = (0..palette.len()).collect();
    layers.sort_by(|&a, &b| color_area[b].cmp(&color_area[a]).then(a.cmp(&b)));
    let mut layer_of = vec![0usize; palette.len()];
    for (i, &l) in layers.iter().enumerate() {
        layer_of[l] = i;
    }
    let mut order: Vec<u32> = (0..n as u32).filter(|&r| emitted[r as usize]).collect();
    order.sort_by(|&a, &b| {
        layer_of[label_of(a)]
            .cmp(&layer_of[label_of(b)])
            .then(seg.regions[b as usize].area.cmp(&seg.regions[a as usize].area))
            .then(a.cmp(&b))
    });
    let mut rank = vec![usize::MAX; n + 1];
    for &r in &order {
        rank[r as usize] = layer_of[label_of(r)];
    }

    let mut shapes: Vec<Shape> = Vec::with_capacity(order.len());
    for &r in &order {
        let mut loops = region_loops(chains, &by_region[r as usize], r);
        if mode == ShapeMode::Stacked {
            // A hole whose whole contents are painted later can be filled in:
            // the later shapes cover it exactly, leaving no anti-aliased seam.
            loops.retain(|lp| {
                if loop_area2(chains, lp) > 0 {
                    return true; // outer boundary
                }
                let mut seen = std::collections::HashSet::new();
                let mut q = VecDeque::new();
                for cr in lp {
                    let c = &chains[cr.chain];
                    let other = if cr.reversed { c.right } else { c.left };
                    if seen.insert(other) {
                        q.push_back(other);
                    }
                }
                while let Some(o) = q.pop_front() {
                    if o == outside || !emitted[o as usize] || rank[o as usize] <= rank[r as usize] || seen.len() > 4000 {
                        return true; // keep the hole
                    }
                    for &nb in &adj[o as usize] {
                        if nb != r && seen.insert(nb) {
                            q.push_back(nb);
                        }
                    }
                }
                false
            });
        }
        let label = seg.regions[r as usize].label as usize;
        shapes.push(Shape { region: r, color: palette[label], loops });
    }

    // Group into <path> elements.
    let mut groups: Vec<([u8; 3], Vec<usize>)> = Vec::new();
    match (mode, optimize) {
        (ShapeMode::Adjoined, true) => {
            for (i, s) in shapes.iter().enumerate() {
                match groups.iter_mut().find(|g| g.0 == s.color) {
                    Some(g) => g.1.push(i),
                    None => groups.push((s.color, vec![i])),
                }
            }
        }
        (_, true) => {
            for (i, s) in shapes.iter().enumerate() {
                match groups.last_mut() {
                    Some(g) if g.0 == s.color => g.1.push(i),
                    _ => groups.push((s.color, vec![i])),
                }
            }
        }
        (_, false) => groups = shapes.iter().enumerate().map(|(i, s)| (s.color, vec![i])).collect(),
    }

    let dec = if optimize { 2 } else { 3 };
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" width=\"{out_w}\" height=\"{out_h}\" viewBox=\"0 0 {out_w} {out_h}\">"
    );
    let mut nodes = 0;
    let mut paths = 0;
    let mut colors: Vec<[u8; 3]> = Vec::new();
    for (color, members) in &groups {
        let mut d = String::new();
        for &i in members {
            let r = shapes[i].region;
            // Small round shapes (dots, beads) become exact circles with the
            // region's area. Safe in stacked mode: shapes below fill underneath.
            if round_dots && mode == ShapeMode::Stacked && shapes[i].loops.len() == 1 {
                let area = seg.regions[r as usize].area as f64;
                if area < 40.0 * scale * scale {
                    if let Some(c) = round_dot(chains, &shapes[i].loops[0], area) {
                        write_circle(c, (area / std::f64::consts::PI).sqrt(), scale, dec, &mut d);
                        nodes += 4;
                        continue;
                    }
                }
            }
            for lp in &shapes[i].loops {
                let tuck: Vec<bool> = lp
                    .iter()
                    .map(|cr| {
                        let c = &chains[cr.chain];
                        let other = if cr.reversed { c.right } else { c.left };
                        mode == ShapeMode::Stacked
                            && other != outside
                            && emitted[other as usize]
                            && rank[other as usize] > rank[r as usize]
                    })
                    .collect();
                nodes += write_loop(chains, fitted, lp, &tuck, TUCK * scale, scale, dec, &mut d);
            }
        }
        if d.is_empty() {
            continue;
        }
        if !colors.contains(color) {
            colors.push(*color);
        }
        paths += 1;
        let _ = writeln!(svg, "<path fill=\"{}\" d=\"{}\"/>", hex(*color), d);
    }
    svg.push_str("</svg>\n");
    Assembled { svg, paths, nodes, colors: colors.len() }
}
