//! What goes in a strand's place: per pixel, the background or skin beside the strand, read on both sides across it
//! and averaged along it.

use lightcraft_raster::{Rgb32f, par_rows};

use super::detect::{SIDE, Strands};
use super::grid::{Grid, bilinear, smoothstep};

/// One pixel's repair: blend `weight × amount` of the way to `fill`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Fill {
    /// The pixel's index in the image (`y × width + x`).
    pub index: usize,
    pub weight: f32,
    pub fill: [f32; 3],
}

/// Pixels whose strand weight is below this are left alone.
const MIN_WEIGHT: f32 = 0.02;
/// The distances across the strand a side is read at, in [`SIDE`] blurs.
const DISTANCES: [f64; 3] = [1.0, 1.5, 2.0];
/// Samples along the strand on each side, either way of the pixel, and their spacing in blurs.
const ALONG: isize = 2;
const ALONG_STEP: f64 = 1.5;
/// A pixel changes only by how much it differs from its fill, in cube-root luminance: not at all at the first, wholly
/// at the second. The skin's or the sky's fine texture beside a strand stays.
const GATE: (f32, f32) = (0.004, 0.03);

fn luma(p: [f32; 3]) -> f32 {
    (0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]).max(0.0).cbrt()
}

/// The fills of the pixels `strands` covers, in the image's order.
pub(super) fn fills(img: &Rgb32f, grid: &Grid, strands: &Strands) -> Vec<Fill> {
    let [x0, y0, x1, y1] = grid.px;
    let (x1, y1) = (x1.min(img.width), y1.min(img.height));
    if x0 >= x1 || y0 >= y1 || strands.weight.len() != grid.len() {
        return Vec::new();
    }
    let side = (SIDE * strands.sigma + 1.0) * grid.cell;
    let step = ALONG_STEP * strands.sigma * grid.cell;
    let mut rows: Vec<Vec<Fill>> = vec![Vec::new(); y1 - y0];
    par_rows(&mut rows, 1, |r, row| {
        let Some(out) = row.first_mut() else { return };
        let y = y0 + r;
        for x in x0..x1 {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let (fx, fy) = grid.to_cell(px, py);
            let w = bilinear(&strands.weight, grid.w, grid.h, fx, fy);
            if w.is_nan() || w <= MIN_WEIGHT {
                continue;
            }
            let (ci, cj) = (fx.round().clamp(0.0, (grid.w - 1) as f64) as usize, fy.round().clamp(0.0, (grid.h - 1) as f64) as usize);
            let k = cj * grid.w + ci;
            let (nx, ny) = strands.across.get(k).copied().unwrap_or((1.0, 0.0));
            let pol = strands.polarity.get(k).copied().unwrap_or(0.0);
            if pol == 0.0 {
                continue;
            }
            let (nx, ny) = (f64::from(nx), f64::from(ny));
            let (tx, ty) = (-ny, nx);
            // one side, at a distance along the strand: the median (by luminance) of three distances across, so a
            // sharpening halo beside the strand or a neighbouring strand does not set the fill
            let sample = |s: f64, d: f64| -> [f32; 3] {
                let mut three = DISTANCES.map(|k| img.sample_bilinear((px + k * s * nx + d * tx) as f32, (py + k * s * ny + d * ty) as f32));
                three.sort_by(|a, b| luma(*a).total_cmp(&luma(*b)));
                three[1]
            };
            let mean = |s: f64| -> [f32; 3] {
                let mut acc = [0.0f32; 3];
                for t in -ALONG..=ALONG {
                    let p = sample(s, t as f64 * step);
                    acc = std::array::from_fn(|c| acc[c] + p[c]);
                }
                acc.map(|v| v / (2 * ALONG + 1) as f32)
            };
            let (a, b) = (mean(side), mean(-side));
            let here = img.get(x, y);
            let lp = luma(here);
            // the side more like the strand (darker, for a dark strand) counts little when the other is clearly
            // unlike it: next to a strand, one side reads the strand itself; between two strands, one reads the other
            let (la, lb) = (luma(a), luma(b));
            let strandier = if pol < 0.0 { la.min(lb) } else { la.max(lb) };
            let (wa, wb) = (smoothstep(GATE.0, GATE.1, pol * (strandier - la)) + 0.02, smoothstep(GATE.0, GATE.1, pol * (strandier - lb)) + 0.02);
            let fill: [f32; 3] = std::array::from_fn(|c| (a[c] * wa + b[c] * wb) / (wa + wb));
            // darker than the fill (the strand) or lighter (the sharpening halo beside it); the texture's small
            // differences stay
            let gate = smoothstep(GATE.0, GATE.1, (lp - luma(fill)).abs());
            let weight = w * gate;
            if weight > MIN_WEIGHT && fill.iter().all(|v| v.is_finite()) {
                out.push(Fill { index: y * img.width + x, weight: weight.min(1.0), fill });
            }
        }
    });
    rows.into_iter().flatten().collect()
}

/// Blend each fill `amount` (0…1) of the way in.
pub(super) fn blend(img: &mut Rgb32f, fills: &[Fill], amount: f32) {
    for f in fills {
        if let Some(p) = img.data.get_mut(f.index) {
            let t = (f.weight * amount).clamp(0.0, 1.0);
            *p = std::array::from_fn(|c| p[c] + (f.fill[c] - p[c]) * t);
        }
    }
}
