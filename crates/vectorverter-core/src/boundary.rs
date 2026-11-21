//! Boundary graph on the pixel-corner grid.
//!
//! Every boundary between two regions is traced exactly once as a *chain*
//! running between junctions (grid vertices where three or more regions meet).
//! Shapes later borrow these shared chains, so neighbouring shapes always
//! agree on their common edge: no gaps, no overlaps.
//!
//! Orientation convention (y down): a chain's `right` region lies on the right
//! of its direction of travel, so a region's outer boundary runs clockwise on
//! screen and its holes counter-clockwise.

use crate::segment::Segmentation;

#[derive(Clone, Debug)]
pub struct Chain {
    pub pts: Vec<(i32, i32)>,
    pub right: u32,
    pub left: u32,
    pub closed: bool,
}

pub struct Graph<'a> {
    seg: &'a Segmentation,
    w: i32,
    h: i32,
    pub outside: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    E,
    S,
    W,
    N,
}

impl Dir {
    pub fn delta(self) -> (i32, i32) {
        match self {
            Dir::E => (1, 0),
            Dir::S => (0, 1),
            Dir::W => (-1, 0),
            Dir::N => (0, -1),
        }
    }
    pub fn from_delta(d: (i32, i32)) -> Dir {
        match d {
            (1, 0) => Dir::E,
            (0, 1) => Dir::S,
            (-1, 0) => Dir::W,
            _ => Dir::N,
        }
    }
    fn reverse(self) -> Dir {
        match self {
            Dir::E => Dir::W,
            Dir::S => Dir::N,
            Dir::W => Dir::E,
            Dir::N => Dir::S,
        }
    }
}

impl<'a> Graph<'a> {
    pub fn new(seg: &'a Segmentation) -> Self {
        Graph { seg, w: seg.width as i32, h: seg.height as i32, outside: seg.regions.len() as u32 }
    }

    #[inline]
    fn reg(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            self.outside
        } else {
            self.seg.map[(y * self.w + x) as usize]
        }
    }

    /// (right, left) regions when travelling from vertex (x, y) in `d`.
    fn sides(&self, x: i32, y: i32, d: Dir) -> (u32, u32) {
        match d {
            Dir::E => (self.reg(x, y), self.reg(x, y - 1)),
            Dir::W => (self.reg(x - 1, y - 1), self.reg(x - 1, y)),
            Dir::S => (self.reg(x - 1, y), self.reg(x, y)),
            Dir::N => (self.reg(x, y - 1), self.reg(x - 1, y - 1)),
        }
    }

    fn is_edge(&self, x: i32, y: i32, d: Dir) -> bool {
        let (nx, ny) = (x + d.delta().0, y + d.delta().1);
        if nx < 0 || ny < 0 || nx > self.w || ny > self.h {
            return false;
        }
        let (r, l) = self.sides(x, y, d);
        r != l
    }

    fn is_junction(&self, x: i32, y: i32) -> bool {
        let q = [self.reg(x - 1, y - 1), self.reg(x, y - 1), self.reg(x - 1, y), self.reg(x, y)];
        let mut distinct = 1;
        for i in 1..4 {
            if !q[..i].contains(&q[i]) {
                distinct += 1;
            }
        }
        if distinct >= 3 {
            return true;
        }
        // Two regions meeting diagonally: all four edges are boundaries.
        [Dir::E, Dir::S, Dir::W, Dir::N].iter().filter(|&&d| self.is_edge(x, y, d)).count() == 4
    }

    fn edge_index(&self, x: i32, y: i32, d: Dir) -> usize {
        // Horizontal edges: W × (H+1); vertical: (W+1) × H.
        let (w, h) = (self.w as usize, self.h as usize);
        match d {
            Dir::E => y as usize * w + x as usize,
            Dir::W => y as usize * w + (x - 1) as usize,
            Dir::S => w * (h + 1) + y as usize * (w + 1) + x as usize,
            Dir::N => w * (h + 1) + (y - 1) as usize * (w + 1) + x as usize,
        }
    }

    fn walk(&self, start: (i32, i32), d0: Dir, visited: &mut [bool]) -> Chain {
        let (right, left) = self.sides(start.0, start.1, d0);
        let mut pts = vec![start];
        let (mut x, mut y, mut d) = (start.0, start.1, d0);
        loop {
            visited[self.edge_index(x, y, d)] = true;
            x += d.delta().0;
            y += d.delta().1;
            pts.push((x, y));
            if (x, y) == start {
                return Chain { pts, right, left, closed: !self.is_junction(x, y) };
            }
            if self.is_junction(x, y) {
                return Chain { pts, right, left, closed: false };
            }
            let back = d.reverse();
            let next = [Dir::E, Dir::S, Dir::W, Dir::N]
                .into_iter()
                .find(|&nd| nd != back && self.is_edge(x, y, nd) && !visited[self.edge_index(x, y, nd)]);
            match next {
                Some(nd) => d = nd,
                None => return Chain { pts, right, left, closed: false },
            }
        }
    }

    pub fn chains(&self) -> Vec<Chain> {
        let (w, h) = (self.w, self.h);
        let mut visited = vec![false; (w * (h + 1) + (w + 1) * h) as usize];
        let mut out = Vec::new();
        // Pass 1: chains that start at junctions.
        for y in 0..=h {
            for x in 0..=w {
                if !self.is_junction(x, y) {
                    continue;
                }
                for d in [Dir::E, Dir::S, Dir::W, Dir::N] {
                    if self.is_edge(x, y, d) && !visited[self.edge_index(x, y, d)] {
                        out.push(self.walk((x, y), d, &mut visited));
                    }
                }
            }
        }
        // Pass 2: closed loops with no junction (islands).
        for y in 0..=h {
            for x in 0..=w {
                for d in [Dir::E, Dir::S] {
                    if self.is_edge(x, y, d) && !visited[self.edge_index(x, y, d)] {
                        let mut c = self.walk((x, y), d, &mut visited);
                        c.closed = true;
                        c.pts.pop();
                        out.push(c);
                    }
                }
            }
        }
        out
    }
}

/// An oriented reference to a chain (reversed when the region is on its left).
#[derive(Clone, Copy, Debug)]
pub struct ChainRef {
    pub chain: usize,
    pub reversed: bool,
}

/// Link a region's chains into closed loops, region always on the right.
pub fn region_loops(chains: &[Chain], ids: &[usize], region: u32) -> Vec<Vec<ChainRef>> {
    use std::collections::HashMap;
    let mut loops = Vec::new();
    let mut open: Vec<ChainRef> = Vec::new();
    for &ci in ids {
        let c = &chains[ci];
        let reversed = c.right != region;
        if c.closed {
            loops.push(vec![ChainRef { chain: ci, reversed }]);
        } else {
            open.push(ChainRef { chain: ci, reversed });
        }
    }
    let start = |r: &ChainRef| {
        let p = &chains[r.chain].pts;
        if r.reversed { *p.last().unwrap() } else { p[0] }
    };
    let end = |r: &ChainRef| {
        let p = &chains[r.chain].pts;
        if r.reversed { p[0] } else { *p.last().unwrap() }
    };
    // Direction of the first / last unit step, for turn preference.
    let first_dir = |r: &ChainRef| {
        let p = &chains[r.chain].pts;
        let (a, b) = if r.reversed { (p[p.len() - 1], p[p.len() - 2]) } else { (p[0], p[1]) };
        Dir::from_delta((b.0 - a.0, b.1 - a.1))
    };
    let last_dir = |r: &ChainRef| {
        let p = &chains[r.chain].pts;
        let (a, b) = if r.reversed { (p[1], p[0]) } else { (p[p.len() - 2], p[p.len() - 1]) };
        Dir::from_delta((b.0 - a.0, b.1 - a.1))
    };

    let mut by_start: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, r) in open.iter().enumerate() {
        by_start.entry(start(r)).or_default().push(i);
    }
    let mut used = vec![false; open.len()];
    for first in 0..open.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let origin = start(&open[first]);
        let mut lp = vec![open[first]];
        let mut cur = open[first];
        let mut guard = 0;
        while end(&cur) != origin && guard < open.len() + 1 {
            guard += 1;
            let v = end(&cur);
            let din = last_dir(&cur).delta();
            // Prefer right turn, then straight, then left (4-connected shapes).
            let rank = |d: Dir| {
                let dd = d.delta();
                let right = (-din.1, din.0);
                if dd == right {
                    0
                } else if dd == din {
                    1
                } else {
                    2
                }
            };
            let Some(cands) = by_start.get(&v) else { break };
            let Some(&next) = cands
                .iter()
                .filter(|&&i| !used[i])
                .min_by_key(|&&i| (rank(first_dir(&open[i])), i))
            else {
                break;
            };
            used[next] = true;
            cur = open[next];
            lp.push(cur);
        }
        loops.push(lp);
    }
    loops
}
