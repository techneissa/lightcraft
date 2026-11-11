//! The colour guided filter of [`snap`], run tile by tile over the band around the coarse edge only.

use rayon::prelude::*;

use crate::{Alpha, Rgb, Stopped};

/// A pixel counts as on the edge when its coverage is strictly between these.
const LO: f32 = 0.02;
const HI: f32 = 0.98;
/// The filter's regularization, in squared 8-bit-colour units over 255² (a colour step of about 2.5 levels counts
/// half): small, so the coverage follows the photo's colour edges closely (He et al. use 1e-6 to 1e-4 for mattes).
const EPS: f64 = 1e-4;
/// Side of the tiles the picture is filtered in (each with a margin of twice the radius).
const TILE: usize = 256;

/// Snap `alpha`'s edge to the photo `img` (the same size): within `radius` pixels of every pixel whose coverage is
/// between 2 % and 98 %, the coverage becomes the colour guided filter of itself steered by the photo; everywhere else
/// it stays exactly as it was. Returns how many pixels it changed; `Stopped` (and `alpha` as it was) when `stop` says
/// so between tiles. A picture whose sizes don't match is left alone.
pub fn snap(img: &Rgb<'_>, alpha: &mut Alpha, radius: usize, stop: &(dyn Fn() -> bool + Sync)) -> Result<usize, Stopped> {
    if !img.is_valid() || !alpha.is_valid() || (img.w, img.h) != (alpha.w, alpha.h) || radius == 0 {
        return Ok(0);
    }
    let (w, h) = (img.w, img.h);
    let r = radius.min(w.max(h));
    let tiles: Vec<(usize, usize)> = (0..h.div_ceil(TILE)).flat_map(|ty| (0..w.div_ceil(TILE)).map(move |tx| (tx * TILE, ty * TILE))).collect();
    let src = &alpha.data;
    let done: Vec<Option<Result<(usize, usize, usize, Vec<f32>), Stopped>>> = tiles
        .par_iter()
        .map(|&(x0, y0)| {
            if stop() {
                return Some(Err(Stopped));
            }
            let (x1, y1) = ((x0 + TILE).min(w), (y0 + TILE).min(h));
            // (nothing on the edge within reach of this tile: it keeps its coverage)
            let reach = (x0.saturating_sub(r), y0.saturating_sub(r), (x1 + r).min(w), (y1 + r).min(h));
            if !any_edge(src, w, reach) {
                return None;
            }
            Some(Ok((x0, y0, x1 - x0, tile(img, src, (x0, y0, x1, y1), r))))
        })
        .collect();
    let mut changed = 0;
    let mut out = alpha.data.clone();
    for d in done.into_iter().flatten() {
        let (x0, y0, tw, vals) = d?;
        for (j, row) in vals.chunks(tw.max(1)).enumerate() {
            let at = (y0 + j) * w + x0;
            if let Some(dst) = out.get_mut(at..at + row.len()) {
                for (o, v) in dst.iter_mut().zip(row) {
                    if (*o - *v).abs() > 1e-6 {
                        changed += 1;
                    }
                    *o = *v;
                }
            }
        }
    }
    alpha.data = out;
    Ok(changed)
}

fn on_edge(a: f32) -> bool {
    a > LO && a < HI
}

fn any_edge(a: &[f32], w: usize, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> bool {
    (y0..y1).any(|y| a.get(y * w + x0..y * w + x1).is_some_and(|row| row.iter().any(|v| on_edge(*v))))
}

/// The filtered coverage of the tile `(x0, y0)–(x1, y1)`, row-major: the guided filter where an edge pixel is within
/// `r`, the coverage as it was elsewhere.
fn tile(img: &Rgb<'_>, a: &[f32], (x0, y0, x1, y1): (usize, usize, usize, usize), r: usize) -> Vec<f32> {
    let w = img.w;
    // the region every window this tile needs reads: the tile and twice the radius around it (the means of a and b
    // over a window need a and b one radius out, each of which needs the sums one radius further)
    let (rx0, ry0) = (x0.saturating_sub(2 * r), y0.saturating_sub(2 * r));
    let (rx1, ry1) = ((x1 + 2 * r).min(w), (y1 + 2 * r).min(img.h));
    let (rw, rh) = (rx1 - rx0, ry1 - ry0);
    let n = rw * rh;
    let mut ch: [Vec<f64>; 3] = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    let mut p = vec![0.0f64; n];
    let mut edge = vec![0.0f64; n];
    for y in 0..rh {
        for x in 0..rw {
            let (gx, gy) = (rx0 + x, ry0 + y);
            let i = y * rw + x;
            for (c, plane) in ch.iter_mut().enumerate() {
                plane[i] = f64::from(img.data.get((gy * w + gx) * 3 + c).copied().unwrap_or(0)) / 255.0;
            }
            let v = a.get(gy * w + gx).copied().unwrap_or(0.0);
            p[i] = f64::from(v);
            edge[i] = if on_edge(v) { 1.0 } else { 0.0 };
        }
    }
    let mean = |v: &[f64]| box_mean(v, rw, rh, r);
    let prod = |a: &[f64], b: &[f64]| -> Vec<f64> { a.iter().zip(b).map(|(x, y)| x * y).collect() };
    let mi: Vec<Vec<f64>> = ch.iter().map(|c| mean(c)).collect();
    let mp = mean(&p);
    let mip: Vec<Vec<f64>> = ch.iter().map(|c| mean(&prod(c, &p))).collect();
    // the colour covariance's six distinct entries: rr, rg, rb, gg, gb, bb
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let mii: Vec<Vec<f64>> = pairs.iter().map(|&(i, j)| mean(&prod(&ch[i], &ch[j]))).collect();
    let near_edge = mean(&edge);
    let mut ca: [Vec<f64>; 3] = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    let mut cb = vec![0.0f64; n];
    for i in 0..n {
        let m = [mi[0][i], mi[1][i], mi[2][i]];
        let cov = [mip[0][i] - m[0] * mp[i], mip[1][i] - m[1] * mp[i], mip[2][i] - m[2] * mp[i]];
        let s = |k: usize, a: usize, b: usize| mii[k][i] - m[a] * m[b];
        let sigma =
            [[s(0, 0, 0) + EPS, s(1, 0, 1), s(2, 0, 2)], [s(1, 0, 1), s(3, 1, 1) + EPS, s(4, 1, 2)], [s(2, 0, 2), s(4, 1, 2), s(5, 2, 2) + EPS]];
        let k = solve(&sigma, &cov);
        for c in 0..3 {
            ca[c][i] = k[c];
        }
        cb[i] = mp[i] - k[0] * m[0] - k[1] * m[1] - k[2] * m[2];
    }
    let ma: Vec<Vec<f64>> = ca.iter().map(|c| mean(c)).collect();
    let mb = mean(&cb);
    let (tw, th) = (x1 - x0, y1 - y0);
    let mut out = Vec::with_capacity(tw * th);
    for y in 0..th {
        for x in 0..tw {
            let i = (y0 + y - ry0) * rw + (x0 + x - rx0);
            let old = p.get(i).copied().unwrap_or(0.0);
            let v = if near_edge.get(i).copied().unwrap_or(0.0) > 0.0 {
                let q = ma[0][i] * ch[0][i] + ma[1][i] * ch[1][i] + ma[2][i] * ch[2][i] + mb[i];
                if q.is_finite() { q.clamp(0.0, 1.0) } else { old }
            } else {
                old
            };
            out.push(v as f32);
        }
    }
    out
}

/// `Σ⁻¹ · v` for a symmetric positive-definite 3 × 3 `Σ` (the regularization keeps it so); zero when it is not.
fn solve(s: &[[f64; 3]; 3], v: &[f64; 3]) -> [f64; 3] {
    let c00 = s[1][1] * s[2][2] - s[1][2] * s[2][1];
    let c01 = s[1][2] * s[2][0] - s[1][0] * s[2][2];
    let c02 = s[1][0] * s[2][1] - s[1][1] * s[2][0];
    let det = s[0][0] * c00 + s[0][1] * c01 + s[0][2] * c02;
    if !det.is_finite() || det.abs() < 1e-30 {
        return [0.0; 3];
    }
    let inv = [
        [c00, s[0][2] * s[2][1] - s[0][1] * s[2][2], s[0][1] * s[1][2] - s[0][2] * s[1][1]],
        [c01, s[0][0] * s[2][2] - s[0][2] * s[2][0], s[0][2] * s[1][0] - s[0][0] * s[1][2]],
        [c02, s[0][1] * s[2][0] - s[0][0] * s[2][1], s[0][0] * s[1][1] - s[0][1] * s[1][0]],
    ];
    let mut out = [0.0; 3];
    for (r, o) in inv.iter().zip(out.iter_mut()) {
        *o = (r[0] * v[0] + r[1] * v[1] + r[2] * v[2]) / det;
    }
    out
}

/// The mean of `v` (`w × h`) over the `(2r + 1)²` window around each pixel, the window clipped to the picture.
pub(crate) fn box_mean(v: &[f64], w: usize, h: usize, r: usize) -> Vec<f64> {
    let mut tmp = vec![0.0f64; w * h];
    let mut pre = vec![0.0f64; w.max(h) + 1];
    for y in 0..h {
        for x in 0..w {
            pre[x + 1] = pre[x] + v.get(y * w + x).copied().unwrap_or(0.0);
        }
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            tmp[y * w + x] = (pre[b] - pre[a]) / (b - a) as f64;
        }
    }
    let mut out = vec![0.0f64; w * h];
    for x in 0..w {
        for y in 0..h {
            pre[y + 1] = pre[y] + tmp[y * w + x];
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            out[y * w + x] = (pre[b] - pre[a]) / (b - a) as f64;
        }
    }
    out
}
