//! The repair, every render: per pixel a strand covers, the background or skin beside the strand as the render has
//! it, read on both sides across the strand and averaged along it.
//!
//! The strands ([`Strands`]) are kept in the photo's normalised frame; each strand cell's centre is mapped into the
//! image through the view, and its tile's local scale (at the tile's centre) maps the directions, the distances and
//! the pixels the cell holds, so a lens or transform warp, a crop, a draft or an export all repair the same strands.
//! The work is per strand cell, never per pixel of the face: a slider drag and a Skin drag stay within a few
//! milliseconds.

use lightcraft_raster::{Rgb32f, par_rows};

use super::grid::smoothstep;
use crate::View;
use crate::input::Strands;

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
/// The least weight kept with a strand cell (0…255): about [`MIN_WEIGHT`].
pub(super) const MIN_STORED: u8 = 5;
/// The distances across the strand a side is read at, in multiples of [`Strands::side`].
const DISTANCES: [f64; 3] = [1.0, 1.5, 2.0];
/// Samples along the strand on each side, either way of the point (their median is taken), and their spacing in
/// strand blurs.
const ALONG: isize = 1;
pub(super) const ALONG_STEP: f64 = 2.5;
/// A pixel changes only by how much it differs from its fill, in cube-root luminance: not at all at the first, wholly
/// at the second. The skin's or the sky's fine texture beside a strand stays.
const GATE: (f32, f32) = (0.004, 0.03);
/// The most a pixel moves away from the strand's polarity (a halo is lighter than the background by a few hundredths
/// of cube-root luminance): wholly up to the first, not at all past the second.
const HALO: (f32, f32) = (0.04, 0.08);

fn luma(p: [f32; 3]) -> f32 {
    (0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]).max(0.0).cbrt()
}

/// The fills of the pixels `strands` covers in `img` (mapped through `view`), in no set order; read from `img` as it
/// is, so blend them only once all are made.
pub(super) fn fills(img: &Rgb32f, view: &dyn View, strands: &Strands) -> Vec<Fill> {
    let (tw, th) = strands.tiles_size();
    let occupied: Vec<(usize, usize)> = (0..th)
        .flat_map(|ty| (0..tw).map(move |tx| (tx, ty)))
        .filter(|(tx, ty)| strands.tiles.get(ty * tw + tx).is_some_and(|t| *t != u32::MAX))
        .collect();
    let m = block(view, strands);
    let mut per: Vec<Vec<Fill>> = vec![Vec::new(); occupied.len()];
    par_rows(&mut per, 1, |k, slot| {
        if let (Some(out), Some(&(tx, ty))) = (slot.first_mut(), occupied.get(k)) {
            tile(img, view, strands, (tx, ty), m, out);
        }
    });
    per.into_iter().flatten().collect()
}

/// How many cells a side the render works on as one: 1, or, where a cell is smaller than a pixel (a draft of a photo
/// searched at a larger size), the fewest that make a block at least a pixel wide, so every pixel on a strand has a
/// block of it as its nearest. One size for the whole face (measured at the strands' centre), and blocks anchored to
/// the grid, so a window and the whole frame use the same blocks.
fn block(view: &dyn View, s: &Strands) -> usize {
    if s.width == 0 || s.height == 0 || !s.area.is_valid() {
        return 1;
    }
    let (cw, ch) = (s.area.width() / s.width as f64, s.area.height() / s.height as f64);
    let (mx, my) = s.area.centre();
    let (Some(p0), Some(pu), Some(pv)) = (view.to_px(mx, my), view.to_px(mx + cw, my), view.to_px(mx, my + ch)) else { return 1 };
    let px = ((pu.0 - p0.0) * (pv.1 - p0.1) - (pv.0 - p0.0) * (pu.1 - p0.1)).abs().sqrt();
    if px.is_finite() && px > 0.0 { (1.0 / px).ceil().clamp(1.0, MAX_BLOCK as f64) as usize } else { 1 }
}

/// The most cells a side a block holds (a render this much smaller than the search is too small to show strands).
const MAX_BLOCK: usize = 8;

/// The fills of the pixels whose nearest block (of `m × m` cells, anchored to the grid) starts in tile `(tx, ty)`: per
/// block holding a strand, one fill read beside the strand from the block's centre, with the direction and polarity
/// of its surest cell, then each pixel the block holds, weighted by the strands' weight there (each block's surest
/// cell's) and by how much the pixel differs from the fill.
fn tile(img: &Rgb32f, view: &dyn View, s: &Strands, (tx, ty): (usize, usize), m: usize, out: &mut Vec<Fill>) {
    const TILE: usize = Strands::TILE;
    if s.width == 0 || s.height == 0 || !s.area.is_valid() || s.picture.0 == 0 || s.picture.1 == 0 {
        return;
    }
    let (cw, ch) = (s.area.width() / s.width as f64, s.area.height() / s.height as f64);
    let (cx0, cy0) = (tx * TILE, ty * TILE);
    let (cx1, cy1) = ((cx0 + TILE).min(s.width), (cy0 + TILE).min(s.height));
    let centre = |cx: f64, cy: f64| (s.area.x0 + (cx + 0.5) * cw, s.area.y0 + (cy + 0.5) * ch);
    // the tile's local scale: how a step in the photo's frame moves in the image (and back)
    let (mx, my) = centre((cx0 + cx1) as f64 / 2.0 - 0.5, (cy0 + cy1) as f64 / 2.0 - 0.5);
    let (Some(p0), Some(pu), Some(pv)) = (view.to_px(mx, my), view.to_px(mx + cw, my), view.to_px(mx, my + ch)) else { return };
    // one cell's step across and down, in the image's pixels
    let (j00, j01, j10, j11) = (pu.0 - p0.0, pv.0 - p0.0, pu.1 - p0.1, pv.1 - p0.1);
    let det = j00 * j11 - j01 * j10;
    if ![j00, j01, j10, j11, det].iter().all(|v| v.is_finite()) || det.abs() < 1e-12 {
        return;
    }
    let (i00, i01, i10, i11) = (j11 / det, -j01 / det, -j10 / det, j00 / det);
    // half a cell's extent in pixels
    let (ex, ey) = (0.5 * (j00.abs() + j01.abs()), 0.5 * (j10.abs() + j11.abs()));
    let (pw, ph) = (s.picture.0 as f64, s.picture.1 as f64);
    // block (`bx`, `by`): its surest cell's weight, direction and polarity
    let surest = |bx: isize, by: isize| -> Option<[u8; 3]> {
        if bx < 0 || by < 0 {
            return None;
        }
        let (x0, y0) = (bx as usize * m, by as usize * m);
        (0..m * m).filter_map(|i| s.cell(x0 + i % m, y0 + i / m)).max_by_key(|c| c[0])
    };
    let weight_at = |bx: f64, by: f64| -> f32 { surest(bx as isize, by as isize).map_or(0.0, |c| f32::from(c[0]) / 255.0) };
    let mf = m as f64;
    // a block's centre, in cells
    let half = (mf - 1.0) / 2.0;
    let (bx0, by0) = (cx0.div_ceil(m), cy0.div_ceil(m));
    for by in by0..cy1.div_ceil(m) {
        for bx in bx0..cx1.div_ceil(m) {
            if bx * m < cx0 || by * m < cy0 || bx * m >= cx1 || by * m >= cy1 {
                continue;
            }
            let Some([q, angle, p]) = surest(bx as isize, by as isize) else { continue };
            let pol = match p {
                1 => -1.0f32,
                2 => 1.0,
                _ => continue,
            };
            if q < MIN_STORED {
                continue;
            }
            let (bcx, bcy) = (bx as f64 * mf + half, by as f64 * mf + half);
            let (ncx, ncy) = centre(bcx, bcy);
            let Some(c) = view.to_px(ncx, ncy).filter(|(x, y)| x.is_finite() && y.is_finite()) else { continue };
            // the direction across the strand, from the searched picture's pixels to the image's
            let a = f64::from(angle) / 255.0 * std::f64::consts::PI;
            let (dx, dy) = (a.cos() / pw / cw, a.sin() / ph / ch);
            let v = (j00 * dx + j01 * dy, j10 * dx + j11 * dy);
            let scale = (v.0 * v.0 + v.1 * v.1).sqrt();
            if !(scale.is_finite() && scale > 0.0) {
                continue;
            }
            let across = Across { n: (v.0 / scale, v.1 / scale), side: s.side * scale, step: s.step * scale, pol };
            let Some(fill) = beside(img, c, &across) else { continue };
            let lf = luma(fill);
            // the pixels this block holds: those whose nearest block it is
            let (ex, ey) = (ex * mf, ey * mf);
            let (x0, x1) = ((c.0 - ex).floor().max(0.0) as usize, ((c.0 + ex).ceil().max(0.0) as usize).min(img.width));
            let (y0, y1) = ((c.1 - ey).floor().max(0.0) as usize, ((c.1 + ey).ceil().max(0.0) as usize).min(img.height));
            for y in y0..y1 {
                for x in x0..x1 {
                    let (ox, oy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
                    // in blocks
                    let (fx, fy) = (bx as f64 + (i00 * ox + i01 * oy) / mf, by as f64 + (i10 * ox + i11 * oy) / mf);
                    if fx.round() != bx as f64 || fy.round() != by as f64 {
                        continue;
                    }
                    let (ix, iy) = (fx.floor(), fy.floor());
                    let (ax, ay) = ((fx - ix) as f32, (fy - iy) as f32);
                    let top = weight_at(ix, iy) + (weight_at(ix + 1.0, iy) - weight_at(ix, iy)) * ax;
                    let bottom = weight_at(ix, iy + 1.0) + (weight_at(ix + 1.0, iy + 1.0) - weight_at(ix, iy + 1.0)) * ax;
                    let w = top + (bottom - top) * ay;
                    if w.is_nan() || w <= MIN_WEIGHT {
                        continue;
                    }
                    let index = y.saturating_mul(img.width).saturating_add(x);
                    let Some(here) = img.data.get(index) else { continue };
                    let weight = w * gate(luma(*here) - lf, pol);
                    if weight > MIN_WEIGHT {
                        out.push(Fill { index, weight: weight.min(1.0), fill });
                    }
                }
            }
        }
    }
}

/// How much a pixel `d` from its fill (cube-root luminance) moves, on a strand of polarity `pol`: as far as it is
/// like the strand (darker than the fill, for a dark strand), wholly beyond [`GATE`]; the other way (the sharpening
/// halo beside a strand) only by a halo's small step, never by more than [`HALO`] (between a strand's tip and the
/// hair mass both sides read dark: the skin there must not darken). The texture's small differences stay.
fn gate(d: f32, pol: f32) -> f32 {
    let g = smoothstep(GATE.0, GATE.1, d.abs());
    if pol * d >= 0.0 { g } else { g * (1.0 - smoothstep(HALO.0, HALO.1, d.abs())) }
}

/// A strand at a point, in the image's pixels: the unit direction across it, how far its sides are, the spacing of
/// the samples along it, and its polarity (−1 darker than its sides, +1 lighter).
struct Across {
    n: (f64, f64),
    side: f64,
    step: f64,
    pol: f32,
}

/// What is beside strand `a` at point `c` (image pixels): on each side across it, the median (by luminance) of three
/// distances, so a sharpening halo beside the strand or a neighbouring strand does not set it, and of three points
/// along the strand, so the strand itself past its tip does not; the side more like the strand counts little when the other is clearly unlike it (next to a strand, one
/// side reads the strand itself; between two strands, one reads the other). `None` for a fill that is not a number.
fn beside(img: &Rgb32f, c: (f64, f64), a: &Across) -> Option<[f32; 3]> {
    let (nx, ny) = a.n;
    let (tx, ty) = (-ny, nx);
    let lin = |p: [f32; 3]| 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2];
    let sample = |s: f64, d: f64| -> [f32; 3] {
        let mut three = DISTANCES.map(|k| img.sample_bilinear((c.0 + k * s * nx + d * tx) as f32, (c.1 + k * s * ny + d * ty) as f32));
        three.sort_by(|p, q| lin(*p).total_cmp(&lin(*q)));
        three[1]
    };
    // along the strand, the median too: past a strand's tip one sample lands on the strand
    let along = |s: f64| -> [f32; 3] {
        let mut v: Vec<[f32; 3]> = (-ALONG..=ALONG).map(|t| sample(s, t as f64 * a.step)).collect();
        v.sort_by(|p, q| lin(*p).total_cmp(&lin(*q)));
        v.get(v.len() / 2).copied().unwrap_or([0.0; 3])
    };
    let (sa, sb) = (along(a.side), along(-a.side));
    let (la, lb) = (luma(sa), luma(sb));
    let strandier = if a.pol < 0.0 { la.min(lb) } else { la.max(lb) };
    let wa = smoothstep(GATE.0, GATE.1, a.pol * (strandier - la)) + 0.02;
    let wb = smoothstep(GATE.0, GATE.1, a.pol * (strandier - lb)) + 0.02;
    let fill: [f32; 3] = std::array::from_fn(|k| (sa[k] * wa + sb[k] * wb) / (wa + wb));
    fill.iter().all(|v| v.is_finite()).then_some(fill)
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
