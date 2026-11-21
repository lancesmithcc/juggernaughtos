//! Sub-pixel boundary placement.
//!
//! Anti-aliased pixels along an edge are blends of the two shapes they sit
//! between, in proportion to coverage. For every unit edge of a chain we read
//! the pixels straight across it, measure how far each one is from shape A's
//! colour towards shape B's (or its opacity, against transparency), and move
//! the boundary point to where that blend crosses 50 %. Hard-edged art has no
//! blends, so its points stay on the pixel grid.

use crate::boundary::Chain;
use crate::color::{rgb_to_lab, segment_distance};
use crate::fit::P;
use crate::quantize::{Palette, ALPHA_CUTOFF, TRANSPARENT};
use crate::segment::Segmentation;
use image::RgbaImage;

pub struct Refiner<'a> {
    img: &'a RgbaImage,
    seg: &'a Segmentation,
    pal: &'a Palette,
    /// Pixels sampled on each side of the edge.
    reach: i32,
    /// Largest allowed move away from the grid edge, in working pixels.
    max_shift: f64,
}

impl<'a> Refiner<'a> {
    pub fn new(img: &'a RgbaImage, seg: &'a Segmentation, pal: &'a Palette, scale: f64) -> Self {
        Refiner { img, seg, pal, reach: scale.ceil() as i32 + 1, max_shift: 0.5 * scale + 0.35 }
    }

    fn label(&self, x: i32, y: i32) -> Option<u16> {
        if x < 0 || y < 0 || x >= self.seg.width as i32 || y >= self.seg.height as i32 {
            return None;
        }
        let r = self.seg.map[y as usize * self.seg.width + x as usize];
        Some(self.seg.regions[r as usize].label)
    }

    /// How much pixel (x, y) looks like `b` rather than `a` (0 = a, 1 = b).
    fn field(&self, x: i32, y: i32, a: u16, b: u16) -> Option<f64> {
        if x < 0 || y < 0 || x >= self.img.width() as i32 || y >= self.img.height() as i32 {
            return None;
        }
        let p = self.img.get_pixel(x as u32, y as u32).0;
        let alpha = p[3] as f64 / 255.0;
        match (a == TRANSPARENT, b == TRANSPARENT) {
            (false, false) => {
                if p[3] < ALPHA_CUTOFF {
                    return None;
                }
                let (la, lb) = (self.pal.lab[a as usize], self.pal.lab[b as usize]);
                let span = crate::color::dist2(la, lb).sqrt();
                if span < 6.0 {
                    return None;
                }
                let (d, t) = segment_distance(rgb_to_lab([p[0], p[1], p[2]]), la, lb);
                // Far off the a→b line: some third colour, not a blend of these two.
                (d <= (0.35 * span).max(8.0)).then(|| (t as f64).clamp(0.0, 1.0))
            }
            (true, false) => Some(alpha),
            (false, true) => Some(1.0 - alpha),
            (true, true) => None,
        }
    }

    /// Boundary point for each unit edge of `chain` (see `fit::grid_mids`).
    pub fn mids(&self, chain: &Chain) -> Vec<P> {
        let raw = &chain.pts;
        let n = raw.len();
        let m = if chain.closed { n } else { n.saturating_sub(1) };
        (0..m)
            .map(|j| {
                let (a, b) = (raw[j], raw[(j + 1) % n]);
                let mid = P::new((a.0 + b.0) as f64 * 0.5, (a.1 + b.1) as f64 * 0.5);
                // Pixel k (k = -1 on the top/left side, 0 on the bottom/right)
                // along the axis that crosses this edge.
                let (px, dir): (Box<dyn Fn(i32) -> (i32, i32)>, P) = if a.1 == b.1 {
                    let (x, y) = (a.0.min(b.0), a.1);
                    (Box::new(move |k| (x, y + k)), P::new(0.0, 1.0))
                } else {
                    let (x, y) = (a.0, a.1.min(b.1));
                    (Box::new(move |k| (x + k, y)), P::new(1.0, 0.0))
                };
                let (Some(la), Some(lb)) = (self.label(px(-1).0, px(-1).1), self.label(px(0).0, px(0).1)) else {
                    return mid;
                };
                if la == lb {
                    return mid;
                }
                let g: Vec<Option<f64>> = (-self.reach..self.reach)
                    .map(|k| {
                        let (x, y) = px(k);
                        self.field(x, y, la, lb)
                    })
                    .collect();
                // Both sides must reach their full colour nearby. Across a stroke
                // only a pixel or two wide the two edges' blurs overlap, the 50 %
                // point drifts, and the stroke's width would wobble: keep the grid.
                let r = self.reach as usize;
                let a_full = g[..r].iter().flatten().any(|&v| v <= 0.35);
                let b_full = g[r..].iter().flatten().any(|&v| v >= 0.65);
                if !(a_full && b_full) {
                    return mid;
                }
                // Rising 50 % crossing nearest the grid edge (position 0).
                let mut best: Option<f64> = None;
                for i in 0..g.len() - 1 {
                    let (Some(g0), Some(g1)) = (g[i], g[i + 1]) else { continue };
                    if g0 < 0.5 && g1 >= 0.5 && g1 > g0 {
                        let k = i as i32 - self.reach;
                        let pos = k as f64 + 0.5 + (0.5 - g0) / (g1 - g0);
                        if best.map_or(true, |b: f64| pos.abs() < b.abs()) {
                            best = Some(pos);
                        }
                    }
                }
                match best {
                    Some(pos) if pos.abs() <= self.max_shift => mid + dir * pos,
                    _ => mid,
                }
            })
            .collect()
    }
}
