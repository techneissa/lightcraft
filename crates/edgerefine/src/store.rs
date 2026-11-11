//! [`store`]: refined coverage as the grids a mask keeps.

use rayon::prelude::*;

use crate::{Alpha, MAX_SIDE, MAX_TILES, logit};

/// A pixel belongs to the edge the finer grids must hold when its coverage is strictly between these.
const LO: f32 = 0.004;
const HI: f32 = 0.996;
/// Pixels of room around the edge in each finer grid.
const PAD: usize = 2;
/// The smallest finer grid.
const MIN_SIDE: usize = 16;

/// A `side × side` grid of logits over the part `rect` of the picture (normalized `[x0, y0, x1, y1]`; row `y`, column
/// `x` covers `((x + 0.5) / side, (y + 0.5) / side)` of it), as `lightcraft_develop::SegMask` stores one.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub rect: [f64; 4],
    pub side: usize,
    pub logits: Vec<f32>,
}

/// The coverage `alpha` as a grid of `side × side` cells over the pixel rectangle `px` (`[x0, y0, x1, y1]`, within the
/// picture): each cell the mean coverage of the pixels whose centres it holds (one pixel at least), or the bilinear
/// coverage at its centre when its cells are narrower than a pixel. `None` when the sizes don't add up.
pub fn tile(alpha: &Alpha, px: [f64; 4], side: usize) -> Option<Grid> {
    let [x0, y0, x1, y1] = px;
    let ok = [x0, y0, x1, y1].iter().all(|v| v.is_finite()) && x1 > x0 && y1 > y0;
    if !alpha.is_valid() || !ok || side == 0 || side > MAX_SIDE {
        return None;
    }
    let (w, h) = (alpha.w, alpha.h);
    let (cw, ch) = ((x1 - x0) / side as f64, (y1 - y0) / side as f64);
    let at = |x: usize, y: usize| alpha.data.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or(0.0);
    let mut logits = vec![0.0f32; side * side];
    logits.par_chunks_mut(side).enumerate().for_each(|(j, row)| {
        for (i, l) in row.iter_mut().enumerate() {
            let (xa, xb) = (x0 + i as f64 * cw, x0 + (i + 1) as f64 * cw);
            let (ya, yb) = (y0 + j as f64 * ch, y0 + (j + 1) as f64 * ch);
            let a = if cw >= 1.0 && ch >= 1.0 {
                let span = |a: f64, b: f64, n: usize| {
                    let lo = (a.round().max(0.0) as usize).min(n - 1);
                    (lo, (b.round().max(0.0) as usize).clamp(lo + 1, n))
                };
                let ((px0, px1), (py0, py1)) = (span(xa, xb, w), span(ya, yb, h));
                let mut s = 0.0f64;
                for y in py0..py1 {
                    for x in px0..px1 {
                        s += f64::from(at(x, y));
                    }
                }
                (s / ((px1 - px0) * (py1 - py0)) as f64) as f32
            } else {
                // (cells finer than a pixel: the bilinear coverage at the centre)
                let (fx, fy) = (((xa + xb) / 2.0 - 0.5).clamp(0.0, (w - 1) as f64), ((ya + yb) / 2.0 - 0.5).clamp(0.0, (h - 1) as f64));
                let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
                let (tx, ty) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
                let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
                let bot = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
                top * (1.0 - ty) + bot * ty
            };
            *l = logit(a);
        }
    });
    Some(Grid { rect: [x0 / w as f64, y0 / h as f64, x1 / w as f64, y1 / h as f64], side, logits })
}

/// `alpha` (over the whole picture) as the grids a mask keeps: one `coarse_side²` grid over the whole picture, and up
/// to [`MAX_TILES`] finer ones (each at most [`MAX_SIDE`]²) whose rectangles hold every edge pixel, so the edge is
/// sampled from them at their finer cells. The edge's bounding box is cut in halves along each side longer than
/// [`MAX_SIDE`] pixels; each part shrinks to the edge pixels it holds. `None` when the sizes don't add up.
pub fn store(alpha: &Alpha, coarse_side: usize) -> Option<(Grid, Vec<Grid>)> {
    let (w, h) = (alpha.w, alpha.h);
    let coarse = tile(alpha, [0.0, 0.0, w as f64, h as f64], coarse_side.clamp(1, MAX_SIDE))?;
    // the edge's bounding box
    let mut bb: Option<[usize; 4]> = None;
    for (i, a) in alpha.data.iter().enumerate() {
        if *a > LO && *a < HI {
            let (x, y) = (i % w, i / w);
            bb = Some(bb.map_or([x, y, x + 1, y + 1], |b| [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)]));
        }
    }
    let Some([bx0, by0, bx1, by1]) = bb else { return Some((coarse, Vec::new())) };
    let kx = if bx1 - bx0 > MAX_SIDE { 2 } else { 1 };
    let ky = if by1 - by0 > MAX_SIDE { 2 } else { 1 };
    let (mx, my) = (bx0 + (bx1 - bx0) / kx, by0 + (by1 - by0) / ky);
    // each part's own edge box
    let mut parts: Vec<Option<[usize; 4]>> = vec![None; kx * ky];
    for y in by0..by1 {
        let row = alpha.data.get(y * w + bx0..y * w + bx1).unwrap_or(&[]);
        for (dx, a) in row.iter().enumerate() {
            if *a > LO && *a < HI {
                let x = bx0 + dx;
                let k = usize::from(kx == 2 && x >= mx) + kx * usize::from(ky == 2 && y >= my);
                if let Some(p) = parts.get_mut(k) {
                    *p = Some(p.map_or([x, y, x + 1, y + 1], |b| [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)]));
                }
            }
        }
    }
    let tiles = parts
        .into_iter()
        .flatten()
        .take(MAX_TILES)
        .filter_map(|[x0, y0, x1, y1]| {
            let r = [x0.saturating_sub(PAD), y0.saturating_sub(PAD), (x1 + PAD).min(w), (y1 + PAD).min(h)];
            let side = (r[2] - r[0]).max(r[3] - r[1]).clamp(MIN_SIDE, MAX_SIDE);
            tile(alpha, r.map(|v| v as f64), side)
        })
        .collect();
    Some((coarse, tiles))
}
