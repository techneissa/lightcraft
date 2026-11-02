//! "SAM 3 + BiRefNet Edges": SAM 3 decides what the subject is, BiRefNet draws its edge.
//!
//! Away from SAM 3's edge its decision stands (the interior stays in, the surroundings stay out, so
//! BiRefNet's mistakes elsewhere in the photo never reach the mask). Within a band around the edge
//! BiRefNet's finer cut-out takes over: strands of hair or fur SAM 3's coarse mask missed come in,
//! background it let through between them goes out. The two blend smoothly across the band's edge.

use crate::Matte;

/// Half-width of the band where BiRefNet decides, as a fraction of the matte's side (3 % ≈ 31 px at
/// 1024): wide enough for SAM 3's mask edge (its grid is 288 cells) and for loose hair.
pub const BAND: f32 = 0.03;
/// Logit magnitude of SAM 3's decision outside the band.
const STRONG: f32 = 8.0;

/// SAM 3's `side × side` logits resampled bilinearly (half-pixel centres) to `n × n`.
fn upsample(logits: &[f32], side: usize, n: usize) -> Vec<f32> {
    let mut out = vec![-STRONG; n * n];
    if side == 0 || logits.len() < side * side || n == 0 {
        return out;
    }
    let at = |x: usize, y: usize| logits.get(y.min(side - 1) * side + x.min(side - 1)).copied().unwrap_or(-STRONG);
    let scale = side as f32 / n as f32;
    for y in 0..n {
        let fy = ((y as f32 + 0.5) * scale - 0.5).max(0.0);
        let (y0, ty) = (fy as usize, fy.fract());
        for x in 0..n {
            let fx = ((x as f32 + 0.5) * scale - 0.5).max(0.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            if let Some(o) = out.get_mut(y * n + x) {
                *o = top * (1.0 - ty) + bottom * ty;
            }
        }
    }
    out
}

/// Chamfer distance (3-4 weights, in thirds of a pixel) of every pixel to the nearest pixel where
/// `inside` differs from its own value.
fn distance_to_edge(inside: &[bool], n: usize) -> Vec<u32> {
    const BIG: u32 = u32::MAX / 4;
    let mut d: Vec<u32> = vec![BIG; n * n];
    // seed: pixels next to a pixel of the other class are one step from the edge
    for y in 0..n {
        for x in 0..n {
            let me = inside.get(y * n + x).copied().unwrap_or(false);
            let differs = |xx: usize, yy: usize| inside.get(yy * n + xx).is_some_and(|v| *v != me);
            let edge =
                (x > 0 && differs(x - 1, y)) || (x + 1 < n && differs(x + 1, y)) || (y > 0 && differs(x, y - 1)) || (y + 1 < n && differs(x, y + 1));
            if edge && let Some(v) = d.get_mut(y * n + x) {
                *v = 0;
            }
        }
    }
    let get = |d: &[u32], x: isize, y: isize| -> u32 {
        if x < 0 || y < 0 || x >= n as isize || y >= n as isize { BIG } else { d.get(y as usize * n + x as usize).copied().unwrap_or(BIG) }
    };
    for y in 0..n as isize {
        for x in 0..n as isize {
            let best =
                [get(&d, x - 1, y) + 3, get(&d, x, y - 1) + 3, get(&d, x - 1, y - 1) + 4, get(&d, x + 1, y - 1) + 4].into_iter().min().unwrap_or(BIG);
            if let Some(v) = d.get_mut(y as usize * n + x as usize) {
                *v = (*v).min(best);
            }
        }
    }
    for y in (0..n as isize).rev() {
        for x in (0..n as isize).rev() {
            let best =
                [get(&d, x + 1, y) + 3, get(&d, x, y + 1) + 3, get(&d, x + 1, y + 1) + 4, get(&d, x - 1, y + 1) + 4].into_iter().min().unwrap_or(BIG);
            if let Some(v) = d.get_mut(y as usize * n + x as usize) {
                *v = (*v).min(best);
            }
        }
    }
    d
}

/// The combined logits (`bire.side²`) of SAM 3's `sam_side²` logits and BiRefNet's matte.
pub fn refine_edges(sam: &[f32], sam_side: usize, bire: &Matte) -> Vec<f32> {
    let n = bire.side;
    let s = upsample(sam, sam_side, n);
    if bire.logits.len() < n * n {
        return s;
    }
    let inside: Vec<bool> = s.iter().map(|v| *v > 0.0).collect();
    let dist = distance_to_edge(&inside, n);
    let band = (BAND * n as f32).max(1.0);
    s.iter()
        .zip(&bire.logits)
        .zip(&dist)
        .zip(&inside)
        .map(|(((_, b), d), inside)| {
            // 1 within half the band of SAM 3's edge, 0 beyond the band, smooth between
            let px = *d as f32 / 3.0;
            let t = ((px - band / 2.0) / (band / 2.0)).clamp(0.0, 1.0);
            let w = 1.0 - t * t * (3.0 - 2.0 * t);
            let sam = if *inside { STRONG } else { -STRONG };
            w * b.clamp(-STRONG * 2.0, STRONG * 2.0) + (1.0 - w) * sam
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disc of `radius` centred at (cx, cy) on an `n × n` grid, as ±`v` logits.
    fn disc(n: usize, cx: f32, cy: f32, radius: f32, v: f32) -> Vec<f32> {
        (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32 + 0.5, (i / n) as f32 + 0.5);
                if ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < radius { v } else { -v }
            })
            .collect()
    }

    #[test]
    fn sam_decides_away_from_its_edge_and_birefnet_at_the_edge() {
        let n = 200;
        // SAM 3's disc is a little too big; BiRefNet's is exact, and finds another disc far away
        let sam = disc(32, 16.0, 16.0, 11.0, 6.0);
        let mut b = disc(n, 100.0, 100.0, 62.5, 10.0);
        let other = disc(n, 20.0, 20.0, 12.0, 10.0);
        for (v, o) in b.iter_mut().zip(&other) {
            if *o > 0.0 {
                *v = 10.0;
            }
        }
        let out = refine_edges(&sam, 32, &Matte { side: n, logits: b });
        let at = |x: usize, y: usize| out[y * n + x];
        assert!(at(100, 100) > 4.0, "the interior stays in");
        assert!(at(100, 150) > 4.0, "50 px from the centre: inside the exact disc");
        // 66 px from the centre: SAM 3 says in, BiRefNet says out, and it is within the band of SAM's edge
        assert!(at(100, 100 + 66) < 0.0, "BiRefNet's edge wins in the band: {}", at(100, 166));
        assert!(at(190, 190) < -4.0, "far outside stays out");
        assert!(at(20, 20) < -4.0, "BiRefNet's other object, far from SAM 3's edge, never comes in");
    }

    #[test]
    fn bad_sizes_do_not_panic() {
        let m = Matte { side: 8, logits: vec![1.0; 64] };
        assert_eq!(refine_edges(&[], 0, &m).len(), 64);
        assert_eq!(refine_edges(&[1.0; 4], 2, &Matte { side: 8, logits: vec![] }).len(), 64);
        assert!(refine_edges(&[1.0; 4], 2, &Matte { side: 0, logits: vec![] }).is_empty());
    }
}
