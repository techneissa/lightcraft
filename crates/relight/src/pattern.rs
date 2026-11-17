//! The patterns a light shines through (issue #316, `fork/relight.md`), drawn here from numbers; nothing is read. Each
//! gives, at a point of the pattern (cells, with an edge half-width in cells, from [`crate::projector`]), the share of
//! the light that passes straight through (the gaps) and the share that passes through the gobo's own material (a
//! leaf, a slat's bounce, the window's fill), which the light gives a tint of its own.
//!
//! - **Blinds:** slats one cell apart; the opening (how far the slats are tilted open) sets the share of each gap. Slats
//!   are not quite even: their spacing and gap vary a little, one in eight sags in the middle and one in twenty-five is
//!   missing. Edges are the exact box blur of the slats (any width; a wide one tends to the gaps' share).
//! - **Window panes:** a grid of panes about two cells wide that fills the window, with mullions between them (the
//!   opening thins them), glass that passes a little less than air and varies slowly.
//! - **Leaves:** clumps of leaves on short stems, with open gaps between clumps; each leaf has its own length, width,
//!   tip and a wobbly edge. Baked once into a tileable texture of signed distances (to the nearest leaf and the second
//!   nearest, so two layers shade more than one), which takes any edge width per pixel. Leaves let some light through.
//! - **Dapple:** images of the sun through small gaps in a canopy: round spots whose radius grows with the distance past
//!   the canopy (the blur), dimmer as they grow (the gap's light spreads), adding where they overlap, in clusters.
//! - **No grid:** the leaves take a slow, smooth warp of their coordinates; every pattern takes slow changes of
//!   brightness; the dapple's gaps are scattered and thin out in clusters.

use crate::effects::Pattern;
use crate::leaves::Leaves;
use crate::projector::{Opening, Sample};

/// What reaches a point of the pattern: `light` straight through, `through` the gobo's own material (tinted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Passed {
    pub light: f32,
    pub through: f32,
}

/// How much of the light a slat's shadow still gets (bounce off the slats and the sill).
const SLAT_PASS: f32 = 0.06;
/// How much a mullion's shadow still gets (the sky's fill round it).
const BAR_PASS: f32 = 0.05;
/// How much light one leaf lets through (two let through its square).
pub(crate) const LEAF_PASS: f32 = 0.18;
/// The light under a canopy that passes through its leaves, between the dapple's spots.
const CANOPY_PASS: f32 = 0.12;
/// The dapple's gaps per cell of the pattern, each way.
const DAPPLE_DENSITY: f32 = 2.2;
/// A pane's glass.
const GLASS: f32 = 0.93;

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn hash(ix: i32, iy: i32, salt: u32) -> u32 {
    let mut h = (ix as u32).wrapping_mul(0x9E37_79B1) ^ (iy as u32).wrapping_mul(0x85EB_CA77) ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// A number in 0..1 from a hash.
pub(crate) fn rand(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

/// Smooth value noise in −1..1, one lattice point per unit.
pub(crate) fn noise(x: f32, y: f32, salt: u32) -> f32 {
    let (x, y) = (x.clamp(-1e6, 1e6), y.clamp(-1e6, 1e6));
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i32, fy as i32);
    let (tx, ty) = (x - fx, y - fy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let at = |dx: i32, dy: i32| 2.0 * rand(hash(ix.wrapping_add(dx), iy.wrapping_add(dy), salt)) - 1.0;
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

/// The length of `[a0, a1] ∩ [b0, b1]`.
fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// How much of the opening covers `(u, v)` with edges `w` cells wide (1 inside, 0 outside).
pub(crate) fn aperture(op: &Opening, u: f32, v: f32, w: f32) -> f32 {
    match *op {
        Opening::None => 1.0,
        Opening::Rect { hx, hy } => {
            // (the exact box blur of the window along each side)
            let side = |c: f32, half: f32| overlap(c - w, c + w, -half, half) / (2.0 * w);
            side(u, hx) * side(v, hy)
        }
        Opening::Round { r, feather } => 1.0 - smoothstep(r - feather - w, r + w, u.hypot(v)),
    }
}

/// The blinds at `(u, v)` (cells; `hx` the window's half-width for the sag), edges `w`, open `open` (0..1).
fn blinds(u: f32, v: f32, w: f32, hx: f32, top: f32, open: f32) -> f32 {
    let gap = 0.15 + 0.7 * open;
    // (from the window's top, so the first slat starts at the frame; slats never quite straight)
    let y = v + top + 0.02 * noise(0.15 * u, 0.15 * v, 3);
    let w = w.clamp(1e-4, 4.0);
    let (lo, hi) = (y - w, y + w);
    let mut lit = 0.0;
    let first = lo.floor() as i64;
    for k in first..=(hi.floor() as i64).min(first + 9) {
        let h = hash(k as i32, 0, 11);
        let kf = k as f32;
        if rand(h) < 0.04 {
            // a missing slat: its whole period is open
            lit += overlap(lo, hi, kf, kf + 1.0);
            continue;
        }
        let across = (u / hx.max(1.0)).clamp(-1.0, 1.0);
        let sag = if rand(h.rotate_left(5)) < 0.12 { 0.14 * (rand(h.rotate_left(9)) - 0.3) * (1.0 - across * across) } else { 0.0 };
        let centre = kf + 0.5 + 0.06 * (rand(h.rotate_left(17)) - 0.5) + sag;
        let half = 0.5 * gap * (1.0 + 0.1 * (rand(h.rotate_left(23)) - 0.5));
        lit += overlap(lo, hi, centre - half, centre + half);
    }
    lit / (2.0 * w)
}

/// One axis of the panes: 1 on glass, 0 on a mullion. With a window (`half`), its panes fill it; without, they repeat.
fn mullions(c: f32, half: Option<f32>, w: f32, bar: f32) -> f32 {
    let w = w.clamp(1e-4, 4.0);
    let (pitch, start, count) = match half {
        Some(half) => {
            let n = (2.0 * half / 2.0).round().max(1.0);
            (2.0 * half / n, -half, Some(n as i64))
        }
        None => (2.0, 0.0, None),
    };
    let pitch = pitch.max(0.2);
    let (lo, hi) = (c - w - bar, c + w + bar);
    let mut covered = 0.0;
    let first = ((lo - start) / pitch).floor() as i64;
    for i in first..=(((hi - start) / pitch).ceil() as i64).min(first + 12) {
        // (the window's own frame is the opening's edge, not a mullion)
        if let Some(n) = count
            && (i <= 0 || i >= n)
        {
            continue;
        }
        let x = start + i as f32 * pitch;
        covered += overlap(c - w, c + w, x - bar, x + bar);
    }
    (1.0 - covered / (2.0 * w)).clamp(0.0, 1.0)
}

/// Dapple at `(u, v)`: spots of radius `gap + sun` from gaps one to a cell (some left out, in clusters).
fn dapple(u: f32, v: f32, sun: f32, open: f32) -> f32 {
    let (ix, iy) = (u.floor() as i32, v.floor() as i32);
    let mut sum = 0.0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix.wrapping_add(dx), iy.wrapping_add(dy));
            let h = hash(cx, cy, 31);
            // (clusters: the canopy is thicker in places)
            let cluster = 0.5 + 0.5 * noise(0.17 * cx as f32, 0.17 * cy as f32, 9);
            if rand(h) > (0.35 + 0.6 * open) * (0.45 + 0.55 * cluster) {
                continue;
            }
            let centre = [cx as f32 + 0.15 + 0.7 * rand(h.rotate_left(7)), cy as f32 + 0.15 + 0.7 * rand(h.rotate_left(13))];
            let gap = 0.16 + 0.22 * rand(h.rotate_left(19));
            let r = gap + sun;
            let d = (u - centre[0]).hypot(v - centre[1]);
            if d > r + 0.5 {
                continue;
            }
            // (the gap's light spreads over the sun's image: a bigger spot is dimmer)
            let bright = (0.6 + 0.4 * rand(h.rotate_left(25))) * (gap / r).powf(1.4).min(1.0) * 2.0;
            let edge = (0.3 * sun + 0.02).min(0.5 * r);
            sum += bright * (1.0 - smoothstep(r - edge, r + edge, d));
        }
    }
    sum.min(1.0)
}

/// What pattern `pattern` passes at `s`, inside opening `op`, open `open` (0..1). The leaves' texture comes with it:
/// it is baked by the caller before any pixel runs, never from inside a parallel loop (a once-only lock taken there
/// deadlocks when the baking thread picks up another pixel of the same loop).
pub(crate) fn passed(pattern: Pattern, s: &Sample, op: &Opening, open: f32, leaves: Option<&Leaves>) -> Passed {
    let (u, v, w) = (s.u, s.v, s.blur);
    let gate = aperture(op, u, v, w);
    if gate <= 0.0 {
        return Passed { light: 0.0, through: 0.0 };
    }
    // (slow changes of brightness over several cells: no two cells alike)
    let slow = |k: f32| 1.0 - k * (0.5 + 0.5 * noise(0.11 * u, 0.11 * v, 17));
    let (light, through) = match pattern {
        // (the water's net is drawn from the scene's points, `crate::caustics`, not from a projector's slide)
        Pattern::None | Pattern::Caustics => (1.0, 0.0),
        Pattern::Blinds => {
            let (hx, top) = match *op {
                Opening::Rect { hx, hy } => (hx, hy),
                _ => (6.0, 0.0),
            };
            let lit = blinds(u, v, w, hx, top, open) * slow(0.06);
            (lit, (1.0 - lit) * SLAT_PASS)
        }
        Pattern::Panes => {
            let bar = 0.04 + 0.1 * (1.0 - open);
            let (hu, hv) = match *op {
                Opening::Rect { hx, hy } => (Some(hx), Some(hy)),
                _ => (None, None),
            };
            let frame = mullions(u, hu, w, bar) * mullions(v, hv, w, bar);
            let glass = GLASS * (1.0 + 0.04 * noise(0.4 * u, 0.4 * v, 5));
            (frame * glass, (1.0 - frame) * BAR_PASS)
        }
        Pattern::Leaves => {
            let (wu, wv) = (u + 0.35 * noise(0.3 * u, 0.3 * v, 1), v + 0.35 * noise(0.3 * u, 0.3 * v, 2));
            let (sd1, sd2) = leaves.map_or((1.0, 1.0), |l| l.distances(wu, wv));
            // (more open: every leaf a little smaller; a canopy sits farther from what it shades than a window's blinds:
            // a third more blur)
            let bias = (open - 0.5) * 0.3;
            let w = 1.3 * w;
            let cover = |sd: f32| 1.0 - smoothstep(-w, w, sd + bias);
            let (c1, c2) = (cover(sd1), cover(sd2).min(cover(sd1)));
            let through = (c1 - c2) * LEAF_PASS + c2 * LEAF_PASS * LEAF_PASS;
            ((1.0 - c1) * slow(0.15), through)
        }
        Pattern::Dapple => {
            // (gaps a little over twice as dense as the other patterns' cells, unwarped: a spot is an image of the round
            // sun whatever the gap's shape, and the gaps' scatter already hides any grid)
            let k = DAPPLE_DENSITY;
            // (the sun's image: its radius is the blur, a share of a gap's cell at the median distance)
            let sun = (1.4 * k * w).min(0.9);
            let lit = dapple(k * u, k * v, sun, open) * slow(0.18);
            (lit, (1.0 - lit) * CANOPY_PASS)
        }
    };
    let clean = |x: f32| if x.is_finite() { (x * gate).clamp(0.0, 1.0) } else { 0.0 };
    Passed { light: clean(light), through: clean(through) }
}

/// Every pattern's coverage in the open (1) and its through share, at many points of a big window: what a pattern
/// passes on average (tests).
#[cfg(test)]
pub(crate) fn mean_passed(pattern: Pattern, open: f32, blur: f32) -> (f32, f32) {
    use rayon::prelude::*;
    let op = Opening::Rect { hx: 1e4, hy: 1e4 };
    let leaves = Leaves::get();
    let n = 160;
    let (l, t) = (0..n * n)
        .into_par_iter()
        .map(|i| {
            let s = Sample { u: (i % n) as f32 * 0.137 + 0.01, v: (i / n) as f32 * 0.137 + 0.01, blur };
            let p = passed(pattern, &s, &op, open, Some(&leaves));
            (p.light, p.through)
        })
        .reduce(|| (0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    (l / (n * n) as f32, t / (n * n) as f32)
}
