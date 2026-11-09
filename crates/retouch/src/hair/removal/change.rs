//! What a face keeps of Stray Hair v2: where its strands were and what changes them into the background behind, and how a
//! render applies it.
//!
//! The search ([`super::with_removals`]) works out, once per photo, per pixel of a head crop: a coverage (how much the
//! pixel is a strand to take away) and the change that puts the background there (the log of new over old light per
//! channel). The face keeps it ([`Removal`], in [`crate::Strands::removal`]), normalised like the masks, in 16-bit
//! cells, only for the tiles of 16 × 16 cells that hold a strand (they are thin: most of the crop has none), and also
//! reduced to 1/2, 1/4 and 1/8 (the mean coverage and the coverage-weighted mean change of each block), so a draft, in
//! which a strand is thinner than a pixel, takes out only the share of the pixel the strand covers.
//!
//! A change, not the background's colour: the render is the photo as edited (exposure, tone, white balance) and the
//! search's picture is not, and a ratio carries across the two where a colour does not (a colour put there showed as
//! pale patches on a raw).
//!
//! A render only multiplies: a pixel takes the factor `e^d` of its cell (a strand pixel, at full size), or of its block
//! (a draft pixel: the ratio of the block's light after and before, so the strand's share of the pixel is what goes), and
//! `amount` of the way there. Amount 0 keeps every bit, 100 takes the strand out; no model runs. A pixel is read at its
//! normalised place, so a zoomed window, a draft and an export make the same change.

use lightcraft_raster::{Plane, Rgb32f, par_rows};

use crate::View;
use crate::input::{Fnv, NormRect};

/// Changes are kept as `round(log light × SCALE)` in 16 bits and coverages as `round(coverage × SCALE)` (a step of 0.05 %).
const SCALE: f32 = 2048.0;
/// A tile's side, in cells.
const TILE: usize = 16;
/// Coverages under this are not kept or applied.
const TINY: f32 = 0.01;
/// Reduced copies kept besides the full one (each half the size of the one before).
const LEVELS: usize = 4;
/// The render's pixels are looked at in blocks of this side.
const BLOCK: usize = 16;
/// The most a pixel is changed, either way (log light).
const MOST: f32 = 3.0;
/// How far round a pixel its lightest and darkest neighbours are looked for, a fraction of the face (2 to 10 pixels).
const WINDOW: f64 = 0.015;
/// Points on each edge of the area that a render's box is followed at (a lens or transform warp bends them).
const EDGE_STEPS: usize = 16;

/// A cell: the change's three channels, then the coverage.
type Cell = [i16; 4];

/// One level of the change: tiles with a strand, as cells.
#[derive(Clone, Debug, Default, PartialEq)]
struct Tiled {
    width: usize,
    height: usize,
    /// Per tile (`width` and `height` divided by [`TILE`], rounded up): its place in `cells` divided by `TILE²`, or
    /// `u32::MAX` for a tile without a strand.
    tiles: Vec<u32>,
    cells: Vec<Cell>,
}

impl Tiled {
    fn tiles_size(&self) -> (usize, usize) {
        (self.width.div_ceil(TILE), self.height.div_ceil(TILE))
    }

    /// Tile (`tx`, `ty`)'s cells' place in `cells`, if it holds a strand.
    fn tile(&self, tx: usize, ty: usize) -> Option<usize> {
        let (tw, th) = self.tiles_size();
        if tx >= tw || ty >= th {
            return None;
        }
        self.tiles.get(ty * tw + tx).copied().filter(|t| *t != u32::MAX).map(|t| (t as usize).saturating_mul(TILE * TILE))
    }

    fn cell(&self, x: usize, y: usize) -> Cell {
        match self.tile(x / TILE, y / TILE) {
            Some(base) => self.cells.get(base + (y % TILE) * TILE + x % TILE).copied().unwrap_or([0; 4]),
            None => [0; 4],
        }
    }

    /// The tiles of the dense `weight` (coverage) and log `change` that hold a strand.
    fn from_dense(weight: &Plane, change: &Rgb32f) -> Tiled {
        let (w, h) = (weight.width, weight.height);
        let q = |v: f32| if v.is_finite() { (v * SCALE).round().clamp(-32000.0, 32000.0) as i16 } else { 0 };
        let cell = |x: usize, y: usize| -> Cell {
            let k = y * w + x;
            let wt = weight.data.get(k).copied().unwrap_or(0.0);
            if wt.is_nan() || wt < TINY {
                return [0; 4];
            }
            let c = change.data.get(k).copied().unwrap_or([0.0; 3]);
            [q(c[0]), q(c[1]), q(c[2]), q(wt.min(1.0))]
        };
        let (tw, th) = (w.div_ceil(TILE), h.div_ceil(TILE));
        let mut tiles = vec![u32::MAX; tw * th];
        let mut cells: Vec<Cell> = Vec::new();
        for ty in 0..th {
            for tx in 0..tw {
                let block: Vec<Cell> = (0..TILE * TILE)
                    .map(|i| (tx * TILE + i % TILE, ty * TILE + i / TILE))
                    .map(|(x, y)| if x < w && y < h { cell(x, y) } else { [0; 4] })
                    .collect();
                if block.iter().any(|c| c[3] != 0) {
                    if let Some(t) = tiles.get_mut(ty * tw + tx) {
                        *t = u32::try_from(cells.len() / (TILE * TILE)).unwrap_or(u32::MAX);
                    }
                    cells.extend(block);
                }
            }
        }
        Tiled { width: w, height: h, tiles, cells }
    }
}

/// Half the size: each 2 × 2 block's mean light.
fn halved(light: &Rgb32f) -> Rgb32f {
    let (w, h) = (light.width.div_ceil(2), light.height.div_ceil(2));
    Rgb32f::from_fn(w, h, |x, y| {
        let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (sx, sy) = (2 * x + dx, 2 * y + dy);
            if sx < light.width && sy < light.height {
                let p = light.data.get(sy * light.width + sx).copied().unwrap_or([0.0; 3]);
                sum = std::array::from_fn(|k| sum[k] + p[k]);
                n += 1.0;
            }
        }
        sum.map(|v| v / n.max(1.0))
    })
}

/// A reduced level from the mean light before (`old`) and after (`new`) the change: its change is the ratio of the two
/// (what a draft pixel holding the block needs), and it has coverage where that is not 1.
fn level_of(old: &Rgb32f, new: &Rgb32f) -> Tiled {
    let change = Rgb32f::from_fn(old.width, old.height, |x, y| {
        let (o, n) = (old.get(x, y), new.get(x, y));
        std::array::from_fn(|k| (n[k].max(1e-6) / o[k].max(1e-6)).ln().clamp(-MOST, MOST))
    });
    let weight = change.map(|d| if d.iter().any(|v| v.abs() > 1e-3) { 1.0 } else { 0.0 });
    Tiled::from_dense(&weight, &change)
}

/// Where a face's stray strands were and what takes them out: a grid of `width × height` cells over `area`
/// (normalised like the face's box, in the same frame). Nothing is replaced outside the grid and in tiles without a
/// strand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Removal {
    pub area: NormRect,
    pub width: usize,
    pub height: usize,
    /// The face's size in cells of the grid (a render picks the reduced copy whose cells are about a pixel from it).
    pub face_cells: f64,
    /// How many strands were taken (pieces of the strand mask).
    pub count: u32,
    /// The search's time and what cutting the head's crop of it took, milliseconds (a readout; not in the fingerprint).
    pub ms: u32,
    pub crop_ms: u32,
    levels: Vec<Tiled>,
}

impl Removal {
    /// The `change` (log light per channel, new over old) where `weight` (the coverage, 0…1) is above 0, over `area`;
    /// `old` is the light (linear) the change was made for, from which the reduced levels are made. Empty (no levels)
    /// when no cell has a weight.
    pub fn new(area: NormRect, weight: &Plane, change: &Rgb32f, old: &Rgb32f, face_cells: f64, count: u32) -> Removal {
        let (w, h) = (weight.width, weight.height);
        let mut removal = Removal { area, width: w, height: h, face_cells, count, ..Removal::default() };
        if !area.is_valid()
            || w == 0
            || h == 0
            || weight.data.len() != w.saturating_mul(h)
            || (change.width, change.height) != (w, h)
            || (old.width, old.height) != (w, h)
        {
            return removal;
        }
        let first = Tiled::from_dense(weight, change);
        if first.cells.is_empty() {
            return removal;
        }
        removal.levels.push(first);
        // the light after the change, where a pixel is a strand (as a render at full size has it)
        let new = Rgb32f::from_fn(w, h, |x, y| {
            let (o, d) = (old.get(x, y), change.get(x, y));
            if weight.get(x, y) >= 0.5 { std::array::from_fn(|k| o[k] * d[k].exp()) } else { o }
        });
        let (mut o, mut n) = (halved(old), halved(&new));
        for _ in 1..LEVELS {
            removal.levels.push(level_of(&o, &n));
            if o.width <= TILE && o.height <= TILE {
                break;
            }
            (o, n) = (halved(&o), halved(&n));
        }
        removal
    }

    /// Whether nothing is taken.
    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    /// About what it takes in memory.
    pub fn bytes(&self) -> usize {
        self.levels.iter().map(|l| l.cells.len().saturating_mul(8).saturating_add(l.tiles.len().saturating_mul(4))).fold(0, usize::saturating_add)
    }

    /// The coverage and the change at normalised point (`nx`, `ny`) in reduced copy `level` (0: full), read bilinearly
    /// between cell centres (the change weighted by the cells' coverages, so a cell with none adds nothing); `None`
    /// where there is no coverage.
    fn sample(&self, level: usize, nx: f64, ny: f64) -> Option<(f32, [f32; 3])> {
        let l = self.levels.get(level)?;
        let (w, h) = (l.width, l.height);
        if w == 0 || h == 0 || !self.area.is_valid() {
            return None;
        }
        let reduce = (1usize << level) as f64;
        let sx = (nx - self.area.x0) / self.area.width() * self.width as f64 / reduce;
        let sy = (ny - self.area.y0) / self.area.height() * self.height as f64 / reduce;
        // (also false for a point that is not a number)
        if !((0.0..=w as f64).contains(&sx) && (0.0..=h as f64).contains(&sy)) {
            return None;
        }
        let fx = (sx - 0.5).clamp(0.0, (w - 1) as f64);
        let fy = (sy - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let mut coverage = 0.0f32;
        let mut sum = [0.0f32; 3];
        for ((x, y), b) in [((x0, y0), (1.0 - tx) * (1.0 - ty)), ((x1, y0), tx * (1.0 - ty)), ((x0, y1), (1.0 - tx) * ty), ((x1, y1), tx * ty)] {
            let c = l.cell(x, y);
            let cw = f32::from(c[3]) / SCALE * b;
            coverage += cw;
            sum = std::array::from_fn(|k| sum[k] + f32::from(c[k]) / SCALE * cw);
        }
        (coverage >= TINY).then(|| (coverage.min(1.0), sum.map(|v| v / coverage)))
    }

    /// The reduced copy whose cells are about a pixel of a render in which the face is `face_px` pixels across.
    fn level_for(&self, face_px: f64) -> usize {
        let ratio = if face_px.is_finite() && face_px > 0.0 { self.face_cells / face_px } else { 1.0 };
        if !ratio.is_finite() || ratio < 1.5 {
            return 0;
        }
        (ratio.log2().floor().max(0.0) as usize).min(self.levels.len().saturating_sub(1))
    }

    /// The pixels of a `width × height` image the grid covers as `view` maps it: `[x0, y0, x1, y1)`; `None` when none.
    fn pixel_box(&self, view: &dyn View, width: usize, height: usize) -> Option<[usize; 4]> {
        let a = self.area;
        if !a.is_valid() || width == 0 || height == 0 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for i in 0..=EDGE_STEPS {
            let t = i as f64 / EDGE_STEPS as f64;
            let (u, v) = (a.x0 + t * a.width(), a.y0 + t * a.height());
            for (nx, ny) in [(u, a.y0), (u, a.y1), (a.x0, v), (a.x1, v)] {
                if let Some((px, py)) = view.to_px(nx, ny).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                    (x0, y0, x1, y1) = (x0.min(px), y0.min(py), x1.max(px), y1.max(py));
                }
            }
        }
        if x0 > x1 || y0 > y1 {
            return None;
        }
        let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
        let b = [clamp(x0.floor() - 1.0, width), clamp(y0.floor() - 1.0, height), clamp(x1.ceil() + 1.0, width), clamp(y1.ceil() + 1.0, height)];
        (b[0] < b[2] && b[1] < b[3]).then_some(b)
    }

    /// Apply `amount` (0…1) of the change to `img` (as `view` maps the photo into it); the face is `face_px` pixels
    /// across in it. A pixel with no weight keeps its bits, and so does every pixel at amount 0.
    pub fn apply(&self, img: &mut Rgb32f, view: &dyn View, face_px: f64, amount: f32) {
        if self.is_empty() || amount.is_nan() || amount <= 0.0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let amount = amount.min(1.0);
        let Some([bx0, by0, bx1, by1]) = self.pixel_box(view, img.width, img.height) else { return };
        let level = self.level_for(face_px);
        let Some(l) = self.levels.get(level) else { return };
        // which blocks of the render's pixels the tiles with a strand (and a tile's width round them, for the bilinear
        // reads) can touch: the pixels in the others cost nothing
        let reduce = (1usize << level) as f64;
        let (ax, ay) = (self.area.x0, self.area.y0);
        let (kx, ky) = (self.width as f64 / self.area.width() / reduce, self.height as f64 / self.area.height() / reduce);
        let (tw, th) = l.tiles_size();
        let (bw, bh) = ((bx1 - bx0).div_ceil(BLOCK), (by1 - by0).div_ceil(BLOCK));
        let mut occupied = vec![false; bw * bh];
        let to_cell = |c: f64, k: f64, a: f64| a + c / k;
        for ty in 0..th {
            for tx in 0..tw {
                if l.tile(tx, ty).is_none() {
                    continue;
                }
                let (c0, c1) =
                    (((tx * TILE) as f64 - TILE as f64, (ty * TILE) as f64 - TILE as f64), (((tx + 2) * TILE) as f64, ((ty + 2) * TILE) as f64));
                let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
                for (cx, cy) in [(c0.0, c0.1), (c1.0, c0.1), (c1.0, c1.1), (c0.0, c1.1)] {
                    if let Some((px, py)) = view.to_px(to_cell(cx, kx, ax), to_cell(cy, ky, ay)).filter(|(x, y)| x.is_finite() && y.is_finite()) {
                        (x0, y0, x1, y1) = (x0.min(px), y0.min(py), x1.max(px), y1.max(py));
                    }
                }
                if x0 > x1 || y0 > y1 {
                    continue;
                }
                let block = |v: f64, from: usize, n: usize| ((v - from as f64) / BLOCK as f64).floor().clamp(0.0, n as f64 - 1.0) as usize;
                let (u0, u1) = (block(x0 - 1.0, bx0, bw), block(x1 + 1.0, bx0, bw));
                let (v0, v1) = (block(y0 - 1.0, by0, bh), block(y1 + 1.0, by0, bh));
                for v in v0..=v1 {
                    for u in u0..=u1 {
                        if let Some(o) = occupied.get_mut(v * bw + u) {
                            *o = true;
                        }
                    }
                }
            }
        }
        let iw = img.width;
        // (what a pixel may become is read from the picture as it was: a copy of the rows the change can touch)
        let src: Vec<[f32; 3]> = img.data.get(by0 * iw..by1 * iw).map(<[_]>::to_vec).unwrap_or_default();
        let rad = ((WINDOW * face_px).round().max(0.0) as usize).clamp(2, 10);
        par_rows(&mut img.data, iw.max(1), |py, row| {
            if py < by0 || py >= by1 {
                return;
            }
            let blocks = occupied.get(((py - by0) / BLOCK) * bw..).unwrap_or(&[]);
            for u in 0..bw {
                if !blocks.get(u).copied().unwrap_or(false) {
                    continue;
                }
                let (x0, x1) = (bx0 + u * BLOCK, (bx0 + (u + 1) * BLOCK).min(bx1));
                let Some(px) = row.get_mut(x0..x1) else { continue };
                for (dx, p) in px.iter_mut().enumerate() {
                    let (nx, ny) = view.to_norm((x0 + dx) as f64 + 0.5, py as f64 + 0.5);
                    let Some((coverage, d)) = self.sample(level, nx, ny) else { continue };
                    // (at full size a pixel is a strand or it is not: the read between cells only blurs the edge)
                    if level == 0 && coverage < 0.35 {
                        continue;
                    }
                    // (`amount` of the way, in light: the slider is linear in what is left of the strand)
                    let c = if level == 0 { smoothstep(0.35, 0.65, coverage) } else { coverage };
                    // (a ratio worked out for the picture the search saw, not the edited one, can take a pixel far off: it
                    // never goes past the lightest and darkest pixels beside it)
                    let (lo, hi) = around(&src, iw, (by0, by1), (x0 + dx, py), rad);
                    let new: [f32; 3] = std::array::from_fn(|k| {
                        let full = (p[k] * (d[k] * c).exp()).clamp(lo[k].min(p[k]), hi[k].max(p[k]));
                        p[k] + amount * (full - p[k])
                    });
                    if new.iter().all(|v| v.is_finite()) {
                        *p = new;
                    }
                }
            }
        });
    }

    /// Mix everything here into the input's fingerprint.
    pub(crate) fn hash(&self, h: &mut Fnv) {
        for v in [self.area.x0, self.area.y0, self.area.x1, self.area.y1, self.face_cells] {
            h.u64(v.to_bits());
        }
        h.u64(self.width as u64);
        h.u64(self.height as u64);
        h.u64(u64::from(self.count));
        if let Some(l) = self.levels.first() {
            for t in &l.tiles {
                h.u64(u64::from(*t));
            }
            for c in &l.cells {
                for v in c {
                    h.u64(u64::from(*v as u16));
                }
            }
        }
    }
}

/// The darkest and lightest values per channel in the square of radius `rad` round (`x`, `y`) of `src` (the rows
/// `rows.0..rows.1` of a `width`-wide picture).
fn around(src: &[[f32; 3]], width: usize, rows: (usize, usize), (x, y): (usize, usize), rad: usize) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for yy in y.saturating_sub(rad).max(rows.0)..(y + rad + 1).min(rows.1) {
        let base = (yy - rows.0) * width;
        for xx in x.saturating_sub(rad)..(x + rad + 1).min(width) {
            if let Some(p) = src.get(base + xx) {
                lo = std::array::from_fn(|k| lo[k].min(p[k]));
                hi = std::array::from_fn(|k| hi[k].max(p[k]));
            }
        }
    }
    (lo, hi)
}

/// 0 under `a`, 1 over `b`, smooth between.
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WholePhoto;

    /// A 64 × 48 grid over the middle of a 128 × 96 photo: one column of cells is a strand that takes 5× the light to be
    /// the background (a change of ln 5).
    fn removal() -> Removal {
        let weight = Plane::from_fn(64, 48, |x, _| if x == 30 { 1.0 } else { 0.0 });
        let change = Rgb32f::from_fn(64, 48, |x, _| if x == 30 { [5f32.ln(); 3] } else { [0.0; 3] });
        Removal::new(NormRect::new(0.25, 0.25, 0.75, 0.75), &weight, &change, &Rgb32f::filled(64, 48, [0.5; 3]), 40.0, 1)
    }

    /// The photo with the strand in pixel column 62 (grid column 30), dark, in a background of 0.5.
    fn photo() -> Rgb32f {
        Rgb32f::from_fn(128, 96, |x, _| if x == 62 { [0.1; 3] } else { [0.5; 3] })
    }

    #[test]
    fn amount_zero_keeps_every_bit_and_a_full_amount_takes_the_strand_out() {
        let r = removal();
        let view = WholePhoto { width: 128, height: 96 };
        let img = photo();
        let mut zero = img.clone();
        r.apply(&mut zero, &view, 40.0, 0.0);
        assert_eq!(zero, img);
        let mut full = img.clone();
        r.apply(&mut full, &view, 40.0, 1.0);
        let mut half = img.clone();
        r.apply(&mut half, &view, 40.0, 0.5);
        // (the render's pixels were edited since the search: everything twice as light; a ratio still takes the strand out)
        let mut edited = Rgb32f::from_fn(128, 96, |x, _| if x == 62 { [0.2; 3] } else { [1.0; 3] });
        r.apply(&mut edited, &view, 40.0, 1.0);
        let (f, h, e) = (full.get(62, 48)[1], half.get(62, 48)[1], edited.get(62, 48)[1]);
        assert!((f - 0.5).abs() < 2e-3, "{f}");
        assert!((h - 0.3).abs() < 2e-3, "{h}: halfway from 0.1 to 0.5");
        assert!((e - 1.0).abs() < 2e-3, "{e}");
        // far from the column nothing changed
        assert_eq!(full.get(5, 5), img.get(5, 5));
        assert_eq!(full.get(100, 90), img.get(100, 90));
        assert_eq!(full.get(61, 48), img.get(61, 48), "nor beside it");
    }

    #[test]
    fn a_render_finer_than_the_grid_takes_the_strand_out_and_touches_nothing_beside_it() {
        // a strand 3 cells wide in a 64-cell grid, drawn at twice the grid's size
        let weight = Plane::from_fn(64, 64, |x, _| if (31..=33).contains(&x) { 1.0 } else { 0.0 });
        let change = Rgb32f::from_fn(64, 64, |x, _| if (31..=33).contains(&x) { [5f32.ln(); 3] } else { [0.0; 3] });
        let r = Removal::new(NormRect::new(0.0, 0.0, 1.0, 1.0), &weight, &change, &Rgb32f::filled(64, 64, [0.5; 3]), 64.0, 1);
        let view = WholePhoto { width: 128, height: 128 };
        let img = Rgb32f::from_fn(128, 128, |x, _| if (62..=67).contains(&x) { [0.1; 3] } else { [0.5; 3] });
        let mut out = img.clone();
        r.apply(&mut out, &view, 400.0, 1.0);
        for x in 62..=67 {
            assert!((out.get(x, 64)[0] - 0.5).abs() < 0.02, "pixel {x}: {}", out.get(x, 64)[0]);
        }
        for x in [50, 60, 61, 68, 69, 80] {
            assert_eq!(out.get(x, 64), img.get(x, 64), "pixel {x} beside the strand");
        }
    }

    #[test]
    fn a_draft_takes_out_the_share_of_the_pixel_the_strand_covered() {
        // a one-cell-wide strand in a 64-cell grid, rendered at a quarter of the grid's size: the draft pixel holds it over
        // a quarter of its width (0.75 background of 0.6, 0.25 strand of 0.1)
        let weight = Plane::from_fn(64, 64, |x, _| if x == 33 { 1.0 } else { 0.0 });
        let change = Rgb32f::from_fn(64, 64, |x, _| if x == 33 { [6f32.ln(); 3] } else { [0.0; 3] });
        let old = Rgb32f::from_fn(64, 64, |x, _| if x == 33 { [0.1; 3] } else { [0.6; 3] });
        let r = Removal::new(NormRect::new(0.0, 0.0, 1.0, 1.0), &weight, &change, &old, 64.0, 1);
        let view = WholePhoto { width: 16, height: 16 };
        let mut img = Rgb32f::from_fn(16, 16, |x, _| if x == 8 { [0.6 * 0.75 + 0.1 * 0.25; 3] } else { [0.6; 3] });
        r.apply(&mut img, &view, 16.0, 1.0);
        let v = img.get(8, 8)[0];
        assert!((v - 0.6).abs() < 0.01, "{v}");
        assert_eq!(img.get(3, 8)[0], 0.6);
        assert_eq!(r.level_for(16.0), 2);
        assert_eq!(r.level_for(64.0), 0);
        assert_eq!(r.level_for(f64::NAN), 0);
    }

    #[test]
    fn an_empty_change_and_odd_input_do_nothing() {
        let view = WholePhoto { width: 16, height: 16 };
        let mut img = Rgb32f::filled(16, 16, [0.3; 3]);
        let none = Removal::new(NormRect::new(0.0, 0.0, 1.0, 1.0), &Plane::new(8, 8), &Rgb32f::new(8, 8), &Rgb32f::new(8, 8), 8.0, 0);
        assert!(none.is_empty());
        none.apply(&mut img, &view, 8.0, 1.0);
        assert_eq!(img, Rgb32f::filled(16, 16, [0.3; 3]));
        let r = removal();
        r.apply(&mut img, &view, f64::NAN, f32::NAN);
        r.apply(&mut img, &view, 0.0, 1.0);
        r.apply(&mut Rgb32f::new(0, 0), &view, 10.0, 1.0);
        let bad = Removal::new(
            NormRect::new(0.0, 0.0, f64::NAN, 1.0),
            &Plane::filled(8, 8, 1.0),
            &Rgb32f::filled(8, 8, [0.0; 3]),
            &Rgb32f::filled(8, 8, [0.5; 3]),
            8.0,
            1,
        );
        assert!(bad.is_empty());
        let mismatched = Removal::new(
            NormRect::new(0.0, 0.0, 1.0, 1.0),
            &Plane::filled(8, 8, 1.0),
            &Rgb32f::filled(4, 4, [0.0; 3]),
            &Rgb32f::filled(8, 8, [0.5; 3]),
            8.0,
            1,
        );
        assert!(mismatched.is_empty());
    }

    #[test]
    fn tiles_keep_only_the_cells_that_changed() {
        let r = removal();
        let first = r.levels.first().map_or(0, |l| l.cells.len());
        assert_eq!(first, 16 * 16 * 3, "one column crosses three tiles of a 64 × 48 grid");
        assert!(r.bytes() > 0);
    }
}
