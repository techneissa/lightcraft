//! The leaves pattern's texture (issue #316): clumps of leaves on short stems, drawn from numbers and baked once into a
//! tileable texture of signed distances (cells; negative inside a leaf): to the nearest leaf or stem and to the second
//! nearest, so where two leaves overlap the light passes through two. A distance takes any edge width when it is read
//! ([`crate::pattern`]), so one texture serves every Softness, distance and opening.
//!
//! A clump sits in most cells of a two-cell grid (the rest are gaps of open sky); from its middle grow three to five
//! stems with three or four leaves each, alternating sides, and one at the tip. Each leaf has its own length, width, tip
//! (pointed or round) and a slight wobble along its edge.

use std::sync::{Arc, OnceLock};

use rayon::prelude::*;

use crate::pattern::{hash, rand};

/// Cells the texture repeats over.
const TILE: usize = 16;
/// Texels per cell.
const TEXELS: usize = 24;
/// Cells per clump of the clumps' grid (it divides [`TILE`]).
const CLUMP: usize = 2;
/// The farthest a clump's leaves reach from its middle, in cells (so a texel needs only the 3 × 3 clumps round it).
const REACH: f32 = 2.0;
/// The distance a texel keeps at most (cells): beyond it, as far as any edge width reads.
const FAR: f32 = 1.5;

#[derive(Clone, Copy, Debug)]
struct Leaf {
    base: [f32; 2],
    dir: [f32; 2],
    len: f32,
    half_width: f32,
    /// How pointed the tip is (a larger power: rounder near the base, sharper at the tip).
    tip: f32,
    wobble: f32,
    phase: f32,
}

#[derive(Clone, Copy, Debug)]
struct Stem {
    a: [f32; 2],
    b: [f32; 2],
}

/// One clump: its middle, leaves and stems, in the clump's own cell's coordinates (cells).
#[derive(Clone, Debug, Default)]
struct Clump {
    centre: [f32; 2],
    leaves: Vec<Leaf>,
    stems: Vec<Stem>,
}

impl Leaf {
    /// The signed distance (about) from `p` to the leaf: negative inside.
    fn distance(&self, p: [f32; 2]) -> f32 {
        let q = [p[0] - self.base[0], p[1] - self.base[1]];
        let x = q[0] * self.dir[0] + q[1] * self.dir[1];
        let y = (-q[0] * self.dir[1] + q[1] * self.dir[0]).abs();
        if x <= 0.0 {
            return x.hypot(y);
        }
        if x >= self.len {
            return (x - self.len).hypot(y);
        }
        let t = x / self.len;
        // (a leaf's outline: t^0.6 (1 − t)^tip, its widest point brought to the half-width, with a wobble)
        let peak_t = 0.6 / (0.6 + self.tip);
        let peak = peak_t.powf(0.6) * (1.0 - peak_t).powf(self.tip);
        let profile = self.half_width * t.powf(0.6) * (1.0 - t).powf(self.tip) / peak.max(1e-3);
        let profile = profile * (1.0 + self.wobble * (11.0 * t + self.phase).sin());
        // (the outline's slope makes the across-distance longer than the true one; good enough for edges)
        y - profile
    }
}

impl Stem {
    fn distance(&self, p: [f32; 2]) -> f32 {
        let (ab, ap) = ([self.b[0] - self.a[0], self.b[1] - self.a[1]], [p[0] - self.a[0], p[1] - self.a[1]]);
        let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-9)).clamp(0.0, 1.0);
        (ap[0] - t * ab[0]).hypot(ap[1] - t * ab[1]) - 0.025
    }
}

/// The clump of clump cell `(i, j)` (wrapped to the tile), or an empty one for a gap.
fn clump(i: usize, j: usize) -> Clump {
    let h = hash(i as i32, j as i32, 41);
    if rand(h) < 0.12 {
        return Clump::default();
    }
    let c = CLUMP as f32;
    let centre = [(i as f32 + 0.3 + 0.4 * rand(h.rotate_left(3))) * c, (j as f32 + 0.3 + 0.4 * rand(h.rotate_left(7))) * c];
    let stems_n = 3 + (h >> 4) as usize % 3;
    let turn0 = rand(h.rotate_left(11)) * std::f32::consts::TAU;
    let mut out = Clump { centre, ..Clump::default() };
    for s in 0..stems_n {
        let hs = hash(h as i32, s as i32, 43);
        let a = turn0 + s as f32 * std::f32::consts::TAU / stems_n as f32 + 0.8 * (rand(hs) - 0.5);
        let dir = [a.cos(), a.sin()];
        let length = 0.5 + 0.45 * rand(hs.rotate_left(5));
        let tip = [centre[0] + dir[0] * length, centre[1] + dir[1] * length];
        out.stems.push(Stem { a: centre, b: tip });
        let leaves_n = 3 + (hs >> 6) as usize % 2;
        for k in 0..=leaves_n {
            let hl = hash(hs as i32, k as i32, 47);
            let (at, side) = if k == leaves_n { (1.0, 0.0) } else { (0.2 + 0.75 * k as f32 / leaves_n as f32, if k % 2 == 0 { 1.0 } else { -1.0 }) };
            let la = a + side * (0.5 + 0.45 * rand(hl));
            let len = 0.5 + 0.35 * rand(hl.rotate_left(3));
            out.leaves.push(Leaf {
                base: [centre[0] + dir[0] * length * at, centre[1] + dir[1] * length * at],
                dir: [la.cos(), la.sin()],
                len,
                half_width: 0.5 * len * (0.32 + 0.18 * rand(hl.rotate_left(7))),
                tip: 0.6 + 0.7 * rand(hl.rotate_left(11)),
                wobble: 0.04 + 0.06 * rand(hl.rotate_left(13)),
                phase: rand(hl.rotate_left(17)) * std::f32::consts::TAU,
            });
        }
    }
    out
}

/// The baked texture.
pub(crate) struct Leaves {
    n: usize,
    near: Vec<f32>,
    second: Vec<f32>,
}

static BAKED: OnceLock<Arc<Leaves>> = OnceLock::new();

impl Leaves {
    /// The texture, baked the first time it is asked for (about 2.4 MB).
    pub(crate) fn get() -> Arc<Leaves> {
        BAKED.get_or_init(|| Arc::new(Leaves::bake())).clone()
    }

    fn bake() -> Leaves {
        let grid = TILE / CLUMP;
        let clumps: Vec<Clump> = (0..grid * grid).map(|k| clump(k % grid, k / grid)).collect();
        let n = TILE * TEXELS;
        let rows: Vec<(Vec<f32>, Vec<f32>)> = (0..n)
            .into_par_iter()
            .map(|y| {
                let mut near = vec![FAR; n];
                let mut second = vec![FAR; n];
                for x in 0..n {
                    let p = [(x as f32 + 0.5) / TEXELS as f32, (y as f32 + 0.5) / TEXELS as f32];
                    let (ci, cj) = ((p[0] / CLUMP as f32) as i64, (p[1] / CLUMP as f32) as i64);
                    let (mut d1, mut d2) = (FAR, FAR);
                    for dj in -1..=1i64 {
                        for di in -1..=1i64 {
                            let (gi, gj) = (ci + di, cj + dj);
                            let Some(c) = clumps.get(gj.rem_euclid(grid as i64) as usize * grid + gi.rem_euclid(grid as i64) as usize) else {
                                continue;
                            };
                            // (the clump drawn where this neighbour lies, past the tile's edge when it wraps)
                            let shift =
                                [(gi - gi.rem_euclid(grid as i64)) as f32 * CLUMP as f32, (gj - gj.rem_euclid(grid as i64)) as f32 * CLUMP as f32];
                            let q = [p[0] - shift[0], p[1] - shift[1]];
                            if (q[0] - c.centre[0]).hypot(q[1] - c.centre[1]) > REACH + FAR {
                                continue;
                            }
                            for d in c.leaves.iter().map(|l| l.distance(q)).chain(c.stems.iter().map(|s| s.distance(q))) {
                                if d < d1 {
                                    (d1, d2) = (d, d1);
                                } else if d < d2 {
                                    d2 = d;
                                }
                            }
                        }
                    }
                    if let (Some(a), Some(b)) = (near.get_mut(x), second.get_mut(x)) {
                        (*a, *b) = (d1.clamp(-FAR, FAR), d2.clamp(-FAR, FAR));
                    }
                }
                (near, second)
            })
            .collect();
        let (mut near, mut second) = (Vec::with_capacity(n * n), Vec::with_capacity(n * n));
        for (a, b) in rows {
            near.extend(a);
            second.extend(b);
        }
        Leaves { n, near, second }
    }

    /// The distances to the nearest leaf and the second nearest at `(u, v)` (cells), bilinear and repeating.
    pub(crate) fn distances(&self, u: f32, v: f32) -> (f32, f32) {
        let n = self.n as i64;
        let period = TILE as f32;
        let wrapped = |c: f32| {
            let c = if c.is_finite() { c.clamp(-1e6, 1e6) } else { 0.0 };
            (c - period * (c / period).floor()) * TEXELS as f32 - 0.5
        };
        let (fx, fy) = (wrapped(u), wrapped(v));
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let read = |tex: &[f32]| {
            let at = |x: f32, y: f32| {
                let (xi, yi) = ((x as i64).rem_euclid(n.max(1)) as usize, (y as i64).rem_euclid(n.max(1)) as usize);
                tex.get(yi * self.n + xi).copied().unwrap_or(FAR)
            };
            let top = at(x0, y0) + (at(x0 + 1.0, y0) - at(x0, y0)) * tx;
            let bottom = at(x0, y0 + 1.0) + (at(x0 + 1.0, y0 + 1.0) - at(x0, y0 + 1.0)) * tx;
            top + (bottom - top) * ty
        };
        (read(&self.near), read(&self.second))
    }
}
