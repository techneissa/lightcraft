//! [`decide`]: which of two selections decides where — a coarse model's (what the subject is) or a matte's (where its
//! edge runs) — on the matte's square grid.

use crate::gaps;

/// The coarse selection's logit away from its edge.
const STRONG: f32 = 8.0;
/// How far around a gap the matte also decides, in cells (its own soft edge round the gap).
const GAP_GROW: u32 = 2;

/// The selection on the matte's `n × n` grid (logits, over the photo stretched to a square): the coarse selection
/// `base` (`base_side²` logits, enlarged bilinearly) decides away from its edge; within `band` (a fraction of the grid's
/// side) of its edge the matte decides, with a smooth hand-over; and, with `gap_cap`, inside every gap [`gaps`] finds
/// (sure background to the matte, each at most that fraction of the selection) and two cells around it. Sizes that
/// don't add up give `base` enlarged.
///
/// With `keep_base` (a coarse selection that is itself a fine cut-out, as BiRefNet's is) the matte only corrects it
/// in the band: where both agree the sharper of the two stands (a matte's softer edge never blurs a crisp one), and
/// where they disagree the matte adds or takes away only as far as it is sure (a strand the cut-out missed and a
/// blob of background it kept are corrected; a glow round a finger and a defocused hand the matte half doubts are
/// not).
pub fn decide(base: &[f32], base_side: usize, matte: &[f32], n: usize, band: f32, gap_cap: Option<f32>, keep_base: bool) -> Vec<f32> {
    let s = upsample(base, base_side, n);
    if matte.len() != n * n || n == 0 {
        return s;
    }
    let inside: Vec<bool> = s.iter().map(|v| *v > 0.0).collect();
    let dist = distance_to_edge(&inside, n, n);
    let band = (band * n as f32).max(1.0);
    let gap = match gap_cap {
        Some(cap) => {
            let g = gaps(&inside, matte, n, n, cap);
            // (a gap's own soft edge, which the matte draws too)
            let d = distance_to_edge(&g, n, n);
            g.iter().zip(&d).map(|(g, d)| *g || *d <= 3 * GAP_GROW).collect()
        }
        None => vec![false; n * n],
    };
    s.iter()
        .zip(matte)
        .zip(&dist)
        .zip(&inside)
        .zip(&gap)
        .map(|((((b, m), d), inside), gap)| {
            // 1 within half the band of the coarse edge (or in a gap), 0 beyond the band, smooth between
            let px = *d as f32 / 3.0;
            let t = ((px - band / 2.0) / (band / 2.0)).clamp(0.0, 1.0);
            let w = if *gap { 1.0 } else { 1.0 - t * t * (3.0 - 2.0 * t) };
            let coarse = if *inside { STRONG } else { -STRONG };
            let m = if m.is_finite() { m.clamp(-2.0 * STRONG, 2.0 * STRONG) } else { coarse };
            let m = if keep_base { fuse(b.clamp(-2.0 * STRONG, 2.0 * STRONG), m) } else { m };
            w * m + (1.0 - w) * coarse
        })
        .collect()
}

/// A fine coarse logit `b` corrected by a matte's `m` ([`decide`]'s `keep_base`).
fn fuse(b: f32, m: f32) -> f32 {
    if (b > 0.0) == (m > 0.0) {
        return if b.abs() > m.abs() { b } else { m };
    }
    if b > 0.0 {
        // (the matte takes away: as far as it is sure, from a logit of -2 to -5)
        let t = ((-m - 2.0) / 3.0).clamp(0.0, 1.0);
        let t = t * t * (3.0 - 2.0 * t);
        return t * m + (1.0 - t) * b;
    }
    // (the matte adds: as far as it is sure, from a logit of 0.5 to 3, so a strand comes in and a glow round a
    // finger does not)
    let t = ((m - 0.5) / 2.5).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    t * m + (1.0 - t) * b
}

/// `side × side` logits resampled bilinearly (half-pixel centres) to `n × n`.
pub fn upsample(logits: &[f32], side: usize, n: usize) -> Vec<f32> {
    let mut out = vec![-STRONG; n * n];
    if side == 0 || logits.len() < side * side || n == 0 {
        return out;
    }
    let at = |x: usize, y: usize| logits.get(y.min(side - 1) * side + x.min(side - 1)).copied().unwrap_or(-STRONG);
    let scale = side as f32 / n as f32;
    for (y, row) in out.chunks_mut(n).enumerate() {
        let fy = ((y as f32 + 0.5) * scale - 0.5).max(0.0);
        let (y0, ty) = (fy as usize, fy.fract());
        for (x, o) in row.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) * scale - 0.5).max(0.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            *o = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}

/// Chamfer distance (3-4 weights, in thirds of a cell) of every cell of a `w × h` grid to the nearest cell where
/// `inside` differs from its own value (far everywhere when it never does).
pub fn distance_to_edge(inside: &[bool], w: usize, h: usize) -> Vec<u32> {
    const BIG: u32 = u32::MAX / 4;
    let mut d: Vec<u32> = vec![BIG; w * h];
    let get_in = |x: usize, y: usize| inside.get(y * w + x).copied().unwrap_or(false);
    for y in 0..h {
        for x in 0..w {
            let me = get_in(x, y);
            let edge = (x > 0 && get_in(x - 1, y) != me)
                || (x + 1 < w && get_in(x + 1, y) != me)
                || (y > 0 && get_in(x, y - 1) != me)
                || (y + 1 < h && get_in(x, y + 1) != me);
            if edge && let Some(v) = d.get_mut(y * w + x) {
                *v = 0;
            }
        }
    }
    let get = |d: &[u32], x: isize, y: isize| -> u32 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize { BIG } else { d.get(y as usize * w + x as usize).copied().unwrap_or(BIG) }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let best =
                [get(&d, x - 1, y) + 3, get(&d, x, y - 1) + 3, get(&d, x - 1, y - 1) + 4, get(&d, x + 1, y - 1) + 4].into_iter().min().unwrap_or(BIG);
            if let Some(v) = d.get_mut(y as usize * w + x as usize) {
                *v = (*v).min(best);
            }
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let best =
                [get(&d, x + 1, y) + 3, get(&d, x, y + 1) + 3, get(&d, x + 1, y + 1) + 4, get(&d, x - 1, y + 1) + 4].into_iter().min().unwrap_or(BIG);
            if let Some(v) = d.get_mut(y as usize * w + x as usize) {
                *v = (*v).min(best);
            }
        }
    }
    d
}
