//! Chain → curves: corner detection, staircase removal, smoothing and
//! least-squares cubic Bézier fitting (after Philip J. Schneider,
//! "An Algorithm for Automatically Fitting Digitized Curves", Graphics Gems, 1990).

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

impl P {
    pub fn new(x: f64, y: f64) -> Self {
        P { x, y }
    }
    fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn norm(self) -> P {
        let l = self.len();
        if l < 1e-12 { P::default() } else { self * (1.0 / l) }
    }
}
impl Add for P {
    type Output = P;
    fn add(self, o: P) -> P {
        P::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for P {
    type Output = P;
    fn sub(self, o: P) -> P {
        P::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for P {
    type Output = P;
    fn mul(self, s: f64) -> P {
        P::new(self.x * s, self.y * s)
    }
}
impl Neg for P {
    type Output = P;
    fn neg(self) -> P {
        P::new(-self.x, -self.y)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Seg {
    Line(P),
    Cubic(P, P, P),
}

impl Seg {
    pub fn end(&self) -> P {
        match *self {
            Seg::Line(p) | Seg::Cubic(_, _, p) => p,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Fitted {
    pub start: P,
    pub segs: Vec<Seg>,
}

/// Split a segment until each piece turns ≤ ~45°, so offsetting its
/// control points along the end normals stays accurate.
fn split_for_offset(s: P, seg: Seg, out: &mut Vec<Seg>, depth: usize) {
    let Seg::Cubic(c1, c2, e) = seg else {
        out.push(seg);
        return;
    };
    let t0 = [c1, c2, e].into_iter().map(|q| q - s).find(|v| v.len() > 1e-9).unwrap_or_default().norm();
    let t1 = [c2, c1, s].into_iter().map(|q| e - q).find(|v| v.len() > 1e-9).unwrap_or_default().norm();
    if depth >= 4 || t0.dot(t1) > 0.7 {
        out.push(seg);
        return;
    }
    // de Casteljau at t = 0.5
    let (a, b, c) = ((s + c1) * 0.5, (c1 + c2) * 0.5, (c2 + e) * 0.5);
    let (d, f) = ((a + b) * 0.5, (b + c) * 0.5);
    let m = (d + f) * 0.5;
    split_for_offset(s, Seg::Cubic(a, d, m), out, depth + 1);
    split_for_offset(m, Seg::Cubic(f, c, e), out, depth + 1);
}

impl Fitted {
    /// Same geometry traversed backwards.
    pub fn reversed(&self) -> Fitted {
        let mut segs = Vec::with_capacity(self.segs.len());
        let mut prev_end = self.start;
        let mut starts = Vec::with_capacity(self.segs.len());
        for s in &self.segs {
            starts.push(prev_end);
            prev_end = s.end();
        }
        for (s, st) in self.segs.iter().zip(starts).rev() {
            segs.push(match *s {
                Seg::Line(_) => Seg::Line(st),
                Seg::Cubic(c1, c2, _) => Seg::Cubic(c2, c1, st),
            });
        }
        Fitted { start: prev_end, segs }
    }

    /// Shift the curve `delta` to the left of its travel direction (nodes move
    /// along their mitred normals, control points move with their node).
    pub fn offset_left(&self, delta: f64, closed: bool) -> Fitted {
        // Moving control points with their end normals only offsets a cubic
        // that turns a little; split strongly curving ones first.
        let mut split = Vec::with_capacity(self.segs.len());
        let mut s = self.start;
        for seg in &self.segs {
            split_for_offset(s, *seg, &mut split, 0);
            s = seg.end();
        }
        let this = Fitted { start: self.start, segs: split };
        let n = this.segs.len();
        if n == 0 {
            return this;
        }
        let mut nodes = Vec::with_capacity(n + 1);
        nodes.push(this.start);
        nodes.extend(this.segs.iter().map(|s| s.end()));
        // Unit tangents leaving / entering each segment.
        let out_t: Vec<P> = (0..n)
            .map(|i| {
                let a = nodes[i];
                match this.segs[i] {
                    Seg::Cubic(c1, c2, e) => [c1, c2, e].into_iter().map(|q| q - a).find(|v| v.len() > 1e-9).unwrap_or_default().norm(),
                    Seg::Line(e) => (e - a).norm(),
                }
            })
            .collect();
        let in_t: Vec<P> = (0..n)
            .map(|i| {
                let e = nodes[i + 1];
                match this.segs[i] {
                    Seg::Cubic(c1, c2, _) => [c2, c1, nodes[i]].into_iter().map(|q| e - q).find(|v| v.len() > 1e-9).unwrap_or_default().norm(),
                    Seg::Line(_) => (e - nodes[i]).norm(),
                }
            })
            .collect();
        let left = |t: P| P::new(t.y, -t.x);
        let node_off = |tin: Option<P>, tout: Option<P>| -> P {
            match (tin, tout) {
                (Some(a), Some(b)) => {
                    let m = (a + b).norm();
                    if m.len() < 1e-9 {
                        return left(b) * delta;
                    }
                    let cos_half = m.dot(b).max(0.5);
                    left(m) * (delta / cos_half)
                }
                (Some(a), None) => left(a) * delta,
                (None, Some(b)) => left(b) * delta,
                (None, None) => P::default(),
            }
        };
        let offs: Vec<P> = (0..=n)
            .map(|i| {
                let tin = if i > 0 { Some(in_t[i - 1]) } else if closed { Some(in_t[n - 1]) } else { None };
                let tout = if i < n { Some(out_t[i]) } else if closed { Some(out_t[0]) } else { None };
                node_off(tin, tout)
            })
            .collect();
        let segs = this
            .segs
            .iter()
            .enumerate()
            .map(|(i, s)| match *s {
                Seg::Line(e) => Seg::Line(e + offs[i + 1]),
                Seg::Cubic(c1, c2, e) => Seg::Cubic(c1 + offs[i], c2 + offs[i + 1], e + offs[i + 1]),
            })
            .collect();
        Fitted { start: this.start + offs[0], segs }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FitParams {
    /// Turn angle (radians) above which a vertex is a corner.
    pub corner_angle: f64,
    /// Look-ahead in grid steps for the turn angle.
    pub k: usize,
    pub smooth_passes: usize,
    /// Max fitting error in working pixels.
    pub tolerance: f64,
    /// Hard-edged (pixel) art: tiny loops may be squares. Otherwise tiny
    /// loops are dots and stay round.
    pub tiny_corners: bool,
}

fn pt(p: (i32, i32)) -> P {
    P::new(p.0 as f64, p.1 as f64)
}

fn turn_angle(a: P, b: P) -> f64 {
    let (la, lb) = (a.len(), b.len());
    if la < 1e-9 || lb < 1e-9 {
        return 0.0;
    }
    (a.dot(b) / (la * lb)).clamp(-1.0, 1.0).acos()
}

fn corners(raw: &[(i32, i32)], closed: bool, p: &FitParams) -> Vec<usize> {
    let n = raw.len();
    if n < 3 {
        return Vec::new();
    }
    let k = p.k.max(1);
    // Tiny closed shapes are too small for the angle test: every pixel step of
    // a dot looks sharp. Only a turn between two long straight runs (a small
    // square or rectangle) counts as a corner; dots and beads stay round.
    if closed && n < 10 * k {
        if !p.tiny_corners {
            return Vec::new();
        }
        let dir = |j: usize| {
            let (a, b) = (raw[j % n], raw[(j + 1) % n]);
            (b.0 - a.0, b.1 - a.1)
        };
        let long = (0.75 * n as f64 / 4.0).max(2.0);
        let run_from = |j: usize, step: isize| {
            let d0 = dir(j);
            let mut len = 1usize;
            let mut i = j as isize;
            while len < n {
                i += step;
                if dir(i.rem_euclid(n as isize) as usize) != d0 {
                    break;
                }
                len += 1;
            }
            len as f64
        };
        return (0..n)
            .filter(|&i| {
                let prev = (i + n - 1) % n;
                dir(prev) != dir(i) && run_from(prev, -1) >= long && run_from(i, 1) >= long
            })
            .collect();
    }
    let at = |i: isize| -> P {
        if closed {
            pt(raw[i.rem_euclid(n as isize) as usize])
        } else {
            pt(raw[i.clamp(0, n as isize - 1) as usize])
        }
    };
    let range: Vec<usize> = if closed { (0..n).collect() } else { (1..n - 1).collect() };
    let mut ang = vec![0f64; n];
    for &i in &range {
        let ii = i as isize;
        let prev = at(ii - 1);
        let next = at(ii + 1);
        let cur = at(ii);
        // Only vertices where the grid path actually turns.
        if (cur - prev) == (next - cur) {
            continue;
        }
        let kk = if closed { k.min(n / 3).max(1) } else { k };
        ang[i] = turn_angle(cur - at(ii - kk as isize), at(ii + kk as isize) - cur);
    }
    let mut out = Vec::new();
    for &i in &range {
        if ang[i] <= p.corner_angle {
            continue;
        }
        let mut is_max = true;
        for d in 1..=k as isize {
            for j in [i as isize - d, i as isize + d] {
                let j = if closed {
                    j.rem_euclid(n as isize) as usize
                } else if j < 0 || j >= n as isize {
                    continue;
                } else {
                    j as usize
                };
                if ang[j] > ang[i] || (ang[j] == ang[i] && j < i) {
                    is_max = false;
                }
            }
        }
        if is_max {
            out.push(i);
        }
    }
    out
}

/// Corner-to-corner span → smoothed polyline (unit-edge midpoints remove
/// the pixel staircase; ends stay pinned).
/// Corner-to-corner span → smoothed polyline. `mids` are the (sub-pixel
/// refined) midpoints of the span's unit grid edges and `dirs` their step
/// directions; the corner ends stay pinned.
///
/// Consecutive steps in the same direction (a "run" of the pixel staircase)
/// collapse to their centroid: run centres lie on the true edge, so a
/// shallow straight or gently curving edge comes out without the staircase
/// wobble that smoothing alone cannot remove.
fn span_polyline(a: P, mids: &[P], dirs: &[(i32, i32)], b: P, passes: usize) -> Vec<P> {
    let mut v = Vec::with_capacity(mids.len() + 2);
    v.push(a);
    let mut j = 0;
    while j < mids.len() {
        let mut k = j + 1;
        while k < mids.len() && dirs[k] == dirs[j] {
            k += 1;
        }
        let n = (k - j) as f64;
        let sum = mids[j..k].iter().fold(P::default(), |acc, &q| acc + q);
        v.push(sum * (1.0 / n));
        j = k;
    }
    v.push(b);
    smooth(&mut v, passes / 2, false);
    v
}

/// Unit step direction leaving each chain vertex (wrapping when closed).
fn step_dirs(raw: &[(i32, i32)], closed: bool) -> Vec<(i32, i32)> {
    let n = raw.len();
    let m = if closed { n } else { n.saturating_sub(1) };
    (0..m).map(|j| {
        let (a, b) = (raw[j], raw[(j + 1) % n]);
        (b.0 - a.0, b.1 - a.1)
    }).collect()
}

/// Plain unit-edge midpoints (no sub-pixel information).
#[cfg(test)]
pub fn grid_mids(raw: &[(i32, i32)], closed: bool) -> Vec<P> {
    let n = raw.len();
    let m = if closed { n } else { n.saturating_sub(1) };
    (0..m).map(|j| (pt(raw[j]) + pt(raw[(j + 1) % n])) * 0.5).collect()
}

/// Taubin λ|μ smoothing: low-pass without the shrinkage of plain Laplacian
/// smoothing, so round shapes keep their size.
fn smooth(v: &mut Vec<P>, passes: usize, closed: bool) {
    let n = v.len();
    if n < 3 {
        return;
    }
    let mut tmp = v.clone();
    for _ in 0..passes {
        for factor in [0.5, -0.53] {
            for i in 0..n {
                if !closed && (i == 0 || i == n - 1) {
                    tmp[i] = v[i];
                    continue;
                }
                let a = v[(i + n - 1) % n];
                let b = v[(i + 1) % n];
                tmp[i] = v[i] + ((a + b) * 0.5 - v[i]) * factor;
            }
            std::mem::swap(v, &mut tmp);
        }
    }
}

fn bez(b: &[P; 4], t: f64) -> P {
    let mt = 1.0 - t;
    b[0] * (mt * mt * mt) + b[1] * (3.0 * mt * mt * t) + b[2] * (3.0 * mt * t * t) + b[3] * (t * t * t)
}

fn bez_d1(b: &[P; 4], t: f64) -> P {
    let mt = 1.0 - t;
    (b[1] - b[0]) * (3.0 * mt * mt) + (b[2] - b[1]) * (6.0 * mt * t) + (b[3] - b[2]) * (3.0 * t * t)
}

fn bez_d2(b: &[P; 4], t: f64) -> P {
    (b[2] - b[1] * 2.0 + b[0]) * (6.0 * (1.0 - t)) + (b[3] - b[2] * 2.0 + b[1]) * (6.0 * t)
}

fn chord_params(d: &[P]) -> Vec<f64> {
    let mut u = vec![0.0; d.len()];
    for i in 1..d.len() {
        u[i] = u[i - 1] + (d[i] - d[i - 1]).len();
    }
    let total = *u.last().unwrap();
    if total > 0.0 {
        for x in u.iter_mut() {
            *x /= total;
        }
    }
    u
}

fn generate(d: &[P], u: &[f64], t1: P, t2: P) -> [P; 4] {
    let (first, last) = (d[0], d[d.len() - 1]);
    let mut c = [[0.0f64; 2]; 2];
    let mut x = [0.0f64; 2];
    for (i, &t) in u.iter().enumerate() {
        let mt = 1.0 - t;
        let (b0, b1, b2, b3) = (mt * mt * mt, 3.0 * t * mt * mt, 3.0 * t * t * mt, t * t * t);
        let a1 = t1 * b1;
        let a2 = t2 * b2;
        c[0][0] += a1.dot(a1);
        c[0][1] += a1.dot(a2);
        c[1][1] += a2.dot(a2);
        let tmp = d[i] - (first * (b0 + b1) + last * (b2 + b3));
        x[0] += a1.dot(tmp);
        x[1] += a2.dot(tmp);
    }
    c[1][0] = c[0][1];
    let det = c[0][0] * c[1][1] - c[0][1] * c[1][0];
    let seg = (last - first).len();
    let (mut al, mut ar) = if det.abs() > 1e-12 {
        ((x[0] * c[1][1] - x[1] * c[0][1]) / det, (c[0][0] * x[1] - c[1][0] * x[0]) / det)
    } else {
        (0.0, 0.0)
    };
    let eps = 1e-6 * seg;
    if al < eps || ar < eps || al > 2.0 * seg || ar > 2.0 * seg {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    [first, first + t1 * al, last + t2 * ar, last]
}

fn max_error(d: &[P], b: &[P; 4], u: &[f64]) -> (f64, usize) {
    let n = d.len();
    let mut worst = 0.0;
    let mut split = n / 2;
    for i in 1..n - 1 {
        let e = (bez(b, u[i]) - d[i]).dot(bez(b, u[i]) - d[i]);
        if e >= worst {
            worst = e;
            split = i;
        }
    }
    (worst, split.clamp(1, n - 2))
}

fn reparam(d: &[P], u: &mut [f64], b: &[P; 4]) {
    for (i, t) in u.iter_mut().enumerate() {
        let q = bez(b, *t) - d[i];
        let q1 = bez_d1(b, *t);
        let q2 = bez_d2(b, *t);
        let den = q1.dot(q1) + q.dot(q2);
        if den.abs() > 1e-12 {
            *t = (*t - q.dot(q1) / den).clamp(0.0, 1.0);
        }
    }
}

fn fit_cubic(d: &[P], t1: P, t2: P, err2: f64, out: &mut Vec<Seg>, depth: usize) {
    let n = d.len();
    if n == 2 {
        out.push(Seg::Line(d[1]));
        return;
    }
    let mut u = chord_params(d);
    let mut b = generate(d, &u, t1, t2);
    let (mut e, mut split) = max_error(d, &b, &u);
    if e < err2 {
        out.push(Seg::Cubic(b[1], b[2], b[3]));
        return;
    }
    if e < err2 * 4.0 {
        for _ in 0..6 {
            reparam(d, &mut u, &b);
            b = generate(d, &u, t1, t2);
            (e, split) = max_error(d, &b, &u);
            if e < err2 {
                out.push(Seg::Cubic(b[1], b[2], b[3]));
                return;
            }
        }
    }
    if depth > 40 || n < 3 {
        out.push(Seg::Cubic(b[1], b[2], b[3]));
        return;
    }
    let mut tc = (d[split - 1] - d[split + 1]).norm();
    if tc.len() < 0.5 {
        tc = (d[split - 1] - d[split]).norm();
    }
    fit_cubic(&d[..=split], t1, tc, err2, out, depth + 1);
    fit_cubic(&d[split..], -tc, t2, err2, out, depth + 1);
}

fn end_tangent(d: &[P], forward: bool) -> P {
    let n = d.len();
    let m = 3.min(n - 1);
    let mut acc = P::default();
    for j in 1..=m {
        acc = acc + if forward { d[j] - d[0] } else { d[n - 1 - j] - d[n - 1] };
    }
    acc.norm()
}

fn max_chord_dev(d: &[P]) -> f64 {
    let (a, b) = (d[0], d[d.len() - 1]);
    let ab = b - a;
    let l = ab.len();
    d.iter()
        .map(|&p| if l < 1e-9 { (p - a).len() } else { ((p - a).x * ab.y - (p - a).y * ab.x).abs() / l })
        .fold(0.0, f64::max)
}

/// Turn near-straight cubics into lines (fewer, cleaner nodes).
fn straighten(start: P, segs: &mut [Seg], tol: f64) {
    let mut s = start;
    for seg in segs.iter_mut() {
        if let Seg::Cubic(c1, c2, e) = *seg {
            if max_chord_dev(&[s, c1, c2, e]) < tol {
                let ab = e - s;
                let l2 = ab.dot(ab);
                let inside = |c: P| {
                    let t = if l2 > 0.0 { (c - s).dot(ab) / l2 } else { 0.0 };
                    (-0.05..=1.05).contains(&t)
                };
                if inside(c1) && inside(c2) {
                    *seg = Seg::Line(e);
                }
            }
        }
        s = seg.end();
    }
}

fn fit_span(d: &[P], p: &FitParams, out: &mut Vec<Seg>) {
    if d.len() <= 2 || max_chord_dev(d) <= p.tolerance * 0.55 {
        out.push(Seg::Line(d[d.len() - 1]));
        return;
    }
    let t1 = end_tangent(d, true);
    let t2 = end_tangent(d, false);
    fit_cubic(d, t1, t2, p.tolerance * p.tolerance, out, 0);
}

/// Clockwise (y down) circle from four cubic arcs, starting at the right.
fn circle(c: P, r: f64) -> Fitted {
    const K: f64 = 0.552_284_75;
    let pts = [P::new(r, 0.0), P::new(0.0, r), P::new(-r, 0.0), P::new(0.0, -r), P::new(r, 0.0)];
    let segs = (0..4)
        .map(|i| {
            let (a, b) = (pts[i], pts[i + 1]);
            // Tangent at a point (x, y) on a clockwise (y-down) circle is (-y, x).
            let ta = P::new(-a.y, a.x) * (K);
            let tb = P::new(-b.y, b.x) * (K);
            Seg::Cubic(c + a + ta, c + b - tb, c + b)
        })
        .collect();
    Fitted { start: c + pts[0], segs }
}

/// `mids[j]` is the boundary point between `raw[j]` and `raw[j + 1]`
/// (wrapping for closed chains), see [`grid_mids`].
pub fn fit_chain(raw: &[(i32, i32)], mids: &[P], closed: bool, p: &FitParams) -> Fitted {
    let n = raw.len();
    let cs = corners(raw, closed, p);
    let dirs = step_dirs(raw, closed);
    let mut segs = Vec::new();
    if !closed {
        let mut cuts = vec![0];
        cuts.extend(cs.iter().copied());
        cuts.push(n - 1);
        for w in cuts.windows(2) {
            let poly = span_polyline(pt(raw[w[0]]), &mids[w[0]..w[1]], &dirs[w[0]..w[1]], pt(raw[w[1]]), p.smooth_passes);
            fit_span(&poly, p, &mut segs);
        }
        let start = pt(raw[0]);
        straighten(start, &mut segs, p.tolerance * 0.3);
        return Fitted { start, segs };
    }
    if cs.is_empty() {
        // Smooth closed curve: run centres all round, tangent-continuous at the seam.
        // Rotate so the seam sits at a run boundary, then take run centroids.
        let start_at = (0..n).find(|&j| dirs[j] != dirs[(j + n - 1) % n]).unwrap_or(0);
        let rm: Vec<P> = (0..n).map(|j| mids[(j + start_at) % n]).collect();
        let rd: Vec<(i32, i32)> = (0..n).map(|j| dirs[(j + start_at) % n]).collect();
        let mut v: Vec<P> = Vec::new();
        let mut j = 0;
        while j < n {
            let mut k = j + 1;
            while k < n && rd[k] == rd[j] {
                k += 1;
            }
            let sum = rm[j..k].iter().fold(P::default(), |acc, &q| acc + q);
            v.push(sum * (1.0 / (k - j) as f64));
            j = k;
        }
        let n = v.len();
        if n < 3 {
            v = mids.to_vec();
        }
        let n = n.max(v.len());
        smooth(&mut v, p.smooth_passes / 2, true);
        // Small round shapes (dots, beads) become exact circles: 4 nodes,
        // no lumps from fitting a handful of pixel points.
        if raw.len() < 10 * p.k.max(1) {
            let c = v.iter().fold(P::default(), |acc, &q| acc + q) * (1.0 / n as f64);
            let radii: Vec<f64> = v.iter().map(|&q| (q - c).len()).collect();
            let r = radii.iter().sum::<f64>() / n as f64;
            let spread = (radii.iter().map(|&x| (x - r) * (x - r)).sum::<f64>() / n as f64).sqrt();
            if r > 0.5 && spread / r < 0.2 {
                return circle(c, r);
            }
        }
        let start = v[0];
        let t = (v[1] - v[n - 1]).norm();
        let mut d = v.clone();
        d.push(start);
        if d.len() < 4 {
            segs.extend(d[1..].iter().map(|&q| Seg::Line(q)));
        } else {
            fit_cubic(&d, t, -t, p.tolerance * p.tolerance, &mut segs, 0);
        }
        straighten(start, &mut segs, p.tolerance * 0.3);
        return Fitted { start, segs };
    }
    // Closed with corners: rotate so a corner starts the loop.
    let mut cuts: Vec<usize> = cs.clone();
    cuts.push(cs[0] + n);
    for w in cuts.windows(2) {
        let span_mids: Vec<P> = (w[0]..w[1]).map(|j| mids[j % n]).collect();
        let span_dirs: Vec<(i32, i32)> = (w[0]..w[1]).map(|j| dirs[j % n]).collect();
        let poly = span_polyline(pt(raw[w[0] % n]), &span_mids, &span_dirs, pt(raw[w[1] % n]), p.smooth_passes);
        fit_span(&poly, p, &mut segs);
    }
    let start = pt(raw[cs[0]]);
    straighten(start, &mut segs, p.tolerance * 0.3);
    Fitted { start, segs }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixel-boundary staircase of a disc of radius r centred at (c, c).
    fn disc_boundary(r: f64) -> Vec<(i32, i32)> {
        let c = r + 4.0;
        let n = (2.0 * c) as i32;
        let inside = |x: i32, y: i32| {
            let (dx, dy) = (x as f64 + 0.5 - c, y as f64 + 0.5 - c);
            x >= 0 && y >= 0 && x < n && y < n && dx * dx + dy * dy <= r * r
        };
        // Walk the outer boundary clockwise (region on the right), starting at the top run.
        let y0 = (0..n).find(|&y| (0..n).any(|x| inside(x, y))).unwrap();
        let x0 = (0..n).find(|&x| inside(x, y0)).unwrap();
        let mut pts = vec![(x0, y0)];
        let (mut x, mut y, mut d) = (x0, y0, (1, 0));
        loop {
            // Right-hand rule on the pixel-corner grid.
            let cands = [(-d.1, d.0), d, (d.1, -d.0)];
            let right_px = |d: (i32, i32), x: i32, y: i32| match d {
                (1, 0) => inside(x, y) && !inside(x, y - 1),
                (0, 1) => inside(x - 1, y) && !inside(x, y),
                (-1, 0) => inside(x - 1, y - 1) && !inside(x - 1, y),
                _ => inside(x, y - 1) && !inside(x - 1, y - 1),
            };
            let nd = *cands.iter().find(|&&c| right_px(c, x, y)).unwrap();
            x += nd.0;
            y += nd.1;
            d = nd;
            if (x, y) == (x0, y0) {
                break;
            }
            pts.push((x, y));
        }
        pts
    }

    fn sample(f: &Fitted) -> Vec<P> {
        let mut out = Vec::new();
        let mut s = f.start;
        for seg in &f.segs {
            match *seg {
                Seg::Line(e) => (0..=20).for_each(|i| out.push(s + (e - s) * (i as f64 / 20.0))),
                Seg::Cubic(a, b, e) => (0..=20).for_each(|i| out.push(bez(&[s, a, b, e], i as f64 / 20.0))),
            }
            s = seg.end();
        }
        out
    }

    #[test]
    fn circle_fit_stays_round() {
        let r = 120.0;
        let raw = disc_boundary(r);
        for (passes, tol) in [(0, 0.5), (7, 1.1), (30, 1.1), (60, 2.0)] {
            let p = FitParams { corner_angle: 55f64.to_radians(), k: 4, smooth_passes: passes, tolerance: tol, tiny_corners: false };
            let f = fit_chain(&raw, &grid_mids(&raw, true), true, &p);
            let c = P::new(r + 4.0, r + 4.0);
            let worst = sample(&f).iter().map(|q| ((*q - c).len() - r).abs()).fold(0.0, f64::max);
            eprintln!("passes {passes} tol {tol}: segs {} worst radial error {worst:.2}", f.segs.len());
            assert!(worst < 1.0 + tol, "wobble {worst}");
        }
    }
}
