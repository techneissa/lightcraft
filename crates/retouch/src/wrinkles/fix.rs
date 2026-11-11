//! Wrinkles v2, what is kept per face and how the sliders use it.
//!
//! The face search ([`super::search`]) works out, once per photo, the change that takes a face's deep wrinkles away:
//! per pixel of a face crop, the natural log of the light's change per channel (new skin over old). The face keeps it
//! ([`Fix`], in `Face::wrinkles`), normalised like the masks. A render only scales it: inside each area, the area's
//! slider (0…100 → 0…1) times the area's weight ([`super::area_weight`], from the face mesh) is how much of the change
//! is applied, as a gain per channel on the pixels the render has. At 0 a pixel keeps its bits; at 100, inside the
//! area, it has the stored change. No model runs while a slider moves.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f, par_rows};

use super::heal::smoothstep;
use super::{Area, Wrinkles, area_weight, areas, clamp_px, union};
use crate::input::{Fnv, NormRect};
use crate::{Part, Scene};

/// The stored change is kept as `round(log change × SCALE)` in 16 bits (a step of 0.05 %).
const SCALE: f32 = 2048.0;
/// Rows of the area's box handled at a time (so a huge export's weight never takes the whole box at once).
const BAND_PIXELS: usize = 1 << 22;
/// Changes smaller than this (log light) are not applied.
const TINY: f32 = 1e-5;
/// The skin around a crease is read with this blur (sigma, a fraction of the face), and a lifted pixel may end up this
/// much above it (log light, about 4 %): Skin runs before Wrinkles and may have softened the crease already, and the
/// stored change was measured on the picture before it.
const REF: f64 = 0.02;
const ROOM: f32 = 0.04;

/// The change a face's wrinkles search found: a grid of `width × height` cells over `area` (normalised like the
/// face's box, in the same frame), each the log of the light's change per channel. Zero outside the grid. Kept in tiles
/// of [`TILE`] × [`TILE`] cells, only the tiles that hold a change: the grid is as fine as the search (a face 1280 px
/// across), and most of it is skin without a line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fix {
    pub area: NormRect,
    pub width: usize,
    pub height: usize,
    /// Per tile, row by row: the index of its cells in `cells` (a multiple of `TILE²`), or [`EMPTY`].
    tiles: Vec<u32>,
    /// The kept tiles' cells, each tile row by row.
    cells: Vec<[i16; 3]>,
}

/// A tile's side, cells.
const TILE: usize = 16;
/// A tile with no change.
const EMPTY: u32 = u32::MAX;

impl Fix {
    /// The change `delta` (log light per channel) over `area`, trimmed to the cells that have one (and a cell of
    /// zero around them, which the bilinear reading needs). `None` when no cell has a change.
    pub fn new(area: NormRect, delta: &Rgb32f) -> Option<Fix> {
        let (w, h) = (delta.width, delta.height);
        if !area.is_valid() || w == 0 || h == 0 || delta.data.len() != w.saturating_mul(h) {
            return None;
        }
        let q = |v: f32| if v.is_finite() { (v * SCALE).round().clamp(-32000.0, 32000.0) as i16 } else { 0 };
        let has = |p: &[f32; 3]| p.iter().any(|v| q(*v) != 0 && v.abs() >= TINY);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for (i, p) in delta.data.iter().enumerate() {
            if has(p) {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
        if x0 >= x1 {
            return None;
        }
        // one cell of zero around
        let (cx0, cy0) = (x0.saturating_sub(1), y0.saturating_sub(1));
        let (cx1, cy1) = ((x1 + 1).min(w), (y1 + 1).min(h));
        let (nw, nh) = (cx1 - cx0, cy1 - cy0);
        let (tw, th) = (nw.div_ceil(TILE), nh.div_ceil(TILE));
        let mut tiles = vec![EMPTY; tw * th];
        let mut cells = Vec::new();
        for ty in 0..th {
            for tx in 0..tw {
                let at = |x: usize, y: usize| -> [i16; 3] {
                    let (gx, gy) = (cx0 + tx * TILE + x, cy0 + ty * TILE + y);
                    if gx >= cx1 || gy >= cy1 {
                        return [0; 3];
                    }
                    delta.data.get(gy * w + gx).copied().unwrap_or([0.0; 3]).map(q)
                };
                let tile: Vec<[i16; 3]> = (0..TILE * TILE).map(|i| at(i % TILE, i / TILE)).collect();
                if tile.iter().all(|c| *c == [0; 3]) {
                    continue;
                }
                if let (Some(slot), Ok(start)) = (tiles.get_mut(ty * tw + tx), u32::try_from(cells.len())) {
                    *slot = start;
                    cells.extend(tile);
                }
            }
        }
        let (aw, ah) = (area.width(), area.height());
        let sub = NormRect::new(
            area.x0 + aw * cx0 as f64 / w as f64,
            area.y0 + ah * cy0 as f64 / h as f64,
            area.x0 + aw * cx1 as f64 / w as f64,
            area.y0 + ah * cy1 as f64 / h as f64,
        );
        Some(Fix { area: sub, width: nw, height: nh, tiles, cells })
    }

    /// The cell at (`x`, `y`) of the grid; zero in a tile with no change or past the grid.
    fn cell(&self, x: usize, y: usize) -> [i16; 3] {
        let tw = self.width.div_ceil(TILE);
        let Some(&start) = self.tiles.get((y / TILE) * tw + x / TILE) else { return [0; 3] };
        if start == EMPTY || x >= self.width || y >= self.height {
            return [0; 3];
        }
        self.cells.get(start as usize + (y % TILE) * TILE + x % TILE).copied().unwrap_or([0; 3])
    }

    /// Whether the search found nothing to change.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// About what it takes in memory.
    pub fn bytes(&self) -> usize {
        self.cells.len().saturating_mul(6).saturating_add(self.tiles.len().saturating_mul(4))
    }

    /// The change at normalised point (`nx`, `ny`), read bilinearly between cell centres; 0 outside the grid.
    pub fn sample(&self, nx: f64, ny: f64) -> [f32; 3] {
        let (w, h) = (self.width, self.height);
        if w == 0 || h == 0 || self.tiles.len() != w.div_ceil(TILE).saturating_mul(h.div_ceil(TILE)) || !self.area.is_valid() {
            return [0.0; 3];
        }
        let (sx, sy) = ((nx - self.area.x0) / self.area.width() * w as f64, (ny - self.area.y0) / self.area.height() * h as f64);
        // (also false for a point that is not a number)
        if !((0.0..=w as f64).contains(&sx) && (0.0..=h as f64).contains(&sy)) {
            return [0.0; 3];
        }
        let fx = (sx - 0.5).clamp(0.0, (w - 1) as f64);
        let fy = (sy - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |x: usize, y: usize| self.cell(x, y);
        // no lift past the mask: where the cell the point lies in has no change, the reading between cells has none
        // either (it would only be the neighbour's change leaking half a cell over the mask's edge)
        let (cx, cy) = (((sx as usize).min(w - 1)), ((sy as usize).min(h - 1)));
        if at(cx, cy) == [0; 3] {
            return [0.0; 3];
        }
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|k| {
            let v = |q: [i16; 3]| f32::from(q[k]) / SCALE;
            let top = v(a) + (v(b) - v(a)) * tx;
            let bottom = v(c) + (v(d) - v(c)) * tx;
            top + (bottom - top) * ty
        })
    }

    /// Mix everything here into the input's fingerprint.
    pub(crate) fn hash(&self, h: &mut Fnv) {
        for v in [self.area.x0, self.area.y0, self.area.x1, self.area.y1] {
            h.u64(v.to_bits());
        }
        h.u64(self.width as u64);
        h.u64(self.height as u64);
        for t in &self.tiles {
            h.u64(u64::from(*t));
        }
        for c in &self.cells {
            for v in c {
                h.u64(u64::from(*v as u16));
            }
        }
    }
}

impl Wrinkles {
    /// The sliders applied to the change a face's search found ([`Fix`]): in each area, `slider × area weight` of the
    /// change, as a gain per channel. Nothing happens for a face without its Facial Skin mask.
    pub(super) fn apply_fix(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: usize, fix: &Fix) {
        if fix.is_empty() || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(ctx) = scene.faces.get(face) else { return };
        let Some(skin) = scene.mask_box(face, Part::FacialSkin) else { return };
        // what the change may touch: the masks' boxes (and the image)
        let limit = scene.mask_box(face, Part::BodySkin).map_or(skin, |b| union(skin, b));
        // each area with a slider above 0: its regions, its pixels
        let mut planned: Vec<(f32, Vec<areas::Region>, [usize; 4])> = Vec::new();
        for area in Area::ALL {
            let k = share(area.amount(self));
            if k <= 0.0 {
                continue;
            }
            let regions = areas::regions(ctx, area);
            let Some(r) = regions.iter().map(areas::Region::bounds).reduce(|u, r| [u[0].min(r[0]), u[1].min(r[1]), u[2].max(r[2]), u[3].max(r[3])])
            else {
                continue;
            };
            let b = [
                clamp_px(r[0], limit[0], limit[2]),
                clamp_px(r[1], limit[1], limit[3]),
                clamp_px(r[2] + 1.0, limit[0], limit[2]),
                clamp_px(r[3] + 1.0, limit[1], limit[3]),
            ];
            if b[0] < b[2] && b[1] < b[3] {
                planned.push((k, regions, b));
            }
        }
        let Some(all) = planned.iter().map(|p| p.2).reduce(union) else { return };
        let corner = [ctx.bounds[0].floor(), ctx.bounds[1].floor()];
        let step = ctx.px(super::LATTICE);
        let view = scene.view();
        let sigma = (REF * ctx.size).max(1.0) as f32;
        let room = (3.0 * sigma).ceil() as usize + 1;
        let w = all[2] - all[0];
        let rows = (BAND_PIXELS / w.max(1)).max(1);
        let mut y = all[1];
        while y < all[3] {
            let band = [all[0], y, all[2], (y + rows).min(all[3])];
            y = band[3];
            // the strongest of the areas' shares at each pixel (areas can overlap)
            let mut weight = Plane::new(w, band[3] - band[1]);
            for (k, regions, b) in &planned {
                let inner = [b[0].max(band[0]), b[1].max(band[1]), b[2].min(band[2]), b[3].min(band[3])];
                if inner[0] >= inner[2] || inner[1] >= inner[3] {
                    continue;
                }
                let a = area_weight(regions, inner, corner, step);
                let aw = inner[2] - inner[0];
                for (row, arow) in a.data.chunks(aw.max(1)).enumerate() {
                    let at = (inner[1] - band[1] + row) * w + (inner[0] - band[0]);
                    if let Some(dst) = weight.data.get_mut(at..at + aw) {
                        dst.iter_mut().zip(arow).for_each(|(d, v)| *d = d.max(*v * *k));
                    }
                }
            }
            let band_h = band[3] - band[1];
            let iw = img.width;
            let (x0, y0) = (band[0], band[1]);
            // the lift each pixel is asked for, over the band and what the reference blur reads around it
            let ext = [band[0].saturating_sub(room), band[1].saturating_sub(room), (band[2] + room).min(img.width), (band[3] + room).min(img.height)];
            let ew = ext[2] - ext[0];
            let mut lift = Rgb32f::new(ew, ext[3] - ext[1]);
            par_rows(&mut lift.data, ew.max(1), |ry, row| {
                for (rx, o) in row.iter_mut().enumerate() {
                    let (nx, ny) = view.to_norm((ext[0] + rx) as f64 + 0.5, (ext[1] + ry) as f64 + 0.5);
                    *o = fix.sample(nx, ny);
                }
            });
            let level = reference(img, &lift, ext, sigma);
            par_rows(&mut img.data, iw.max(1), |py, row| {
                let Some(by) = py.checked_sub(y0).filter(|by| *by < band_h) else { return };
                let (Some(px), Some(wrow)) = (row.get_mut(x0..x0 + w), weight.data.get(by * w..(by + 1) * w)) else { return };
                for (dx, (p, k)) in px.iter_mut().zip(wrow).enumerate() {
                    if *k <= 0.0 {
                        continue;
                    }
                    let at = (py - ext[1]) * ew + (x0 + dx - ext[0]);
                    let (Some(d), Some(l)) = (lift.data.get(at), level.data.get(at)) else { continue };
                    if d.iter().all(|v| v.abs() < TINY) {
                        continue;
                    }
                    // what Skin (which runs first) softened is not lifted again: never past the skin around, and a
                    // little more
                    let want = *k * luma(*d);
                    let t = if want > 0.0 { ((l[0].max(0.0) + ROOM) / want).min(1.0) } else { 1.0 };
                    let new: [f32; 3] = std::array::from_fn(|c| p[c] * (d[c] * *k * t).exp());
                    if new.iter().all(|v| v.is_finite()) {
                        *p = new;
                    }
                }
            });
        }
    }
}

/// The luminance of a log change (Rec.2020 weights: the app's working space).
fn luma(d: [f32; 3]) -> f32 {
    0.2627 * d[0] + 0.6780 * d[1] + 0.0593 * d[2]
}

/// For each pixel of the box `ext` of `img`: how far the log luminance of the skin around it lies above the pixel's own
/// (first value; the others are 0). The blur (Gaussian `sigma` pixels) reads only the pixels
/// the stored change leaves alone (a lifted crease is not skin around), so a crease is compared with its neighbours.
fn reference(img: &Rgb32f, lift: &Rgb32f, ext: [usize; 4], sigma: f32) -> Rgb32f {
    let (ew, eh) = (ext[2] - ext[0], ext[3] - ext[1]);
    let mut own = Plane::new(ew, eh);
    par_rows(&mut own.data, ew.max(1), |ry, row| {
        for (rx, o) in row.iter_mut().enumerate() {
            let p = img.data.get((ext[1] + ry) * img.width + ext[0] + rx).copied().unwrap_or([0.0; 3]);
            *o = (0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]).max(1e-4).ln();
        }
    });
    let alone = lift.map(|d| 1.0 - smoothstep(0.01, 0.03, luma(d).abs()));
    let num = gaussian(&own.zip_map(&alone, |y, q| y * q), sigma);
    let den = gaussian(&alone, sigma);
    Rgb32f::from_fn(ew, eh, |x, y| {
        let (y_own, d) = (own.get(x, y), den.get(x, y));
        let around = if d > 0.05 { num.get(x, y) / d } else { y_own };
        [around - y_own, 0.0, 0.0]
    })
}

/// A slider (0…100) as the share of the stored change applied (a value that is not a number is 0).
fn share(v: f64) -> f32 {
    if v.is_finite() { (v.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change over 100 × 93 cells, zero over a block in the middle and outside a ring: every cell reads back as it was
    /// stored, and the tiles of the empty middle keep nothing.
    #[test]
    fn the_tiles_give_back_every_cell_and_keep_only_the_changed_ones() {
        let (w, h) = (100usize, 93usize);
        let delta = Rgb32f::from_fn(w, h, |x, y| {
            if (2..98).contains(&x) && (1..92).contains(&y) && !((20..80).contains(&x) && (20..75).contains(&y)) {
                [0.004 * (x as f32 - 50.0), 0.003 * y as f32, -0.02]
            } else {
                [0.0; 3]
            }
        });
        let fix = Fix::new(NormRect::new(0.0, 0.0, 1.0, 1.0), &delta).expect("a change");
        assert!(fix.cells.len() < fix.tiles.len() * TILE * TILE, "the empty tiles in the middle keep nothing");
        for y in 0..h {
            for x in 0..w {
                let got = fix.sample((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
                let want = delta.data[y * w + x];
                for c in 0..3 {
                    assert!((got[c] - want[c]).abs() <= 0.5 / SCALE + 1e-6, "cell {x},{y} channel {c}: {} ≠ {}", got[c], want[c]);
                }
            }
        }
    }
}
