//! The grids Stray Hair works on: cells a fixed share of the face, anchored to the face's box (never to the image's
//! corner, which a zoomed window moves), over the pixels the search covers.

use crate::{FaceCtx, Part, Scene};

use super::region::{BAND, CAP_BELOW, CAP_SIDE, CAP_TOP};

/// Most cells a grid holds (a grid is about a million cells; this only guards against a damaged face size).
const MAX_CELLS: f64 = 8.0e6;

/// Cells over a box of pixels: cell (`i`, `j`)'s centre is at `origin + (i0 + i + 0.5, j0 + j + 0.5) × cell`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Grid {
    /// The anchor: the face box's top-left corner, in pixels.
    pub ox: f64,
    pub oy: f64,
    /// A cell's side, in pixels.
    pub cell: f64,
    /// The face's size, in pixels.
    pub face: f64,
    pub i0: isize,
    pub j0: isize,
    pub w: usize,
    pub h: usize,
    /// The pixels the search covers: `[x0, y0, x1, y1)`, inside the image.
    pub px: [usize; 4],
}

impl Grid {
    /// The fine grid of face `face`: [`super::CELLS_PER_FACE`] cells across the face (no finer than a pixel), over
    /// the face's box and the Hair and Facial Skin masks' areas grown by [`BAND`], kept within the caps around the
    /// face. `None` for a face that is not in the image or has neither mask.
    pub fn for_face(scene: &Scene<'_>, face: usize) -> Option<Grid> {
        let ctx = scene.faces.get(face)?;
        let f = ctx.size;
        if !(f.is_finite() && f > 0.0) {
            return None;
        }
        let masks: Vec<[usize; 4]> = [Part::Hair, Part::FacialSkin].iter().filter_map(|p| scene.mask_box(face, *p)).collect();
        if masks.is_empty() {
            return None;
        }
        let b = ctx.bounds;
        let mut r = b;
        for m in &masks {
            r = [r[0].min(m[0] as f64), r[1].min(m[1] as f64), r[2].max(m[2] as f64), r[3].max(m[3] as f64)];
        }
        let band = BAND * f;
        let r = [
            (r[0] - band).max(b[0] - CAP_SIDE * f).max(0.0),
            (r[1] - band).max(b[1] - CAP_TOP * f).max(0.0),
            (r[2] + band).min(b[2] + CAP_SIDE * f).min(scene.width as f64),
            (r[3] + band).min(b[3] + CAP_BELOW * f).min(scene.height as f64),
        ];
        if !r.iter().all(|v| v.is_finite()) || r[0] >= r[2] || r[1] >= r[3] {
            return None;
        }
        let px = [r[0].floor() as usize, r[1].floor() as usize, r[2].ceil() as usize, r[3].ceil() as usize];
        let mut cell = (f / super::CELLS_PER_FACE).max(1.0);
        let cells = (r[2] - r[0]) * (r[3] - r[1]) / (cell * cell);
        if cells > MAX_CELLS {
            cell *= (cells / MAX_CELLS).sqrt();
        }
        Some(Grid::over(ctx, b, cell, px))
    }

    fn over(ctx: &FaceCtx<'_>, b: [f64; 4], cell: f64, px: [usize; 4]) -> Grid {
        let (ox, oy) = (b[0], b[1]);
        let i0 = ((px[0] as f64 - ox) / cell).floor() as isize;
        let j0 = ((px[1] as f64 - oy) / cell).floor() as isize;
        let i1 = ((px[2] as f64 - ox) / cell).ceil() as isize;
        let j1 = ((px[3] as f64 - oy) / cell).ceil() as isize;
        Grid { ox, oy, cell, face: ctx.size, i0, j0, w: (i1 - i0).max(1) as usize, h: (j1 - j0).max(1) as usize, px }
    }

    /// The same pixels, anchor and face with cells `k` times as wide.
    pub fn coarser(&self, k: f64) -> Grid {
        let cell = self.cell * k.max(1.0);
        let (ox, oy, px) = (self.ox, self.oy, self.px);
        let i0 = ((px[0] as f64 - ox) / cell).floor() as isize;
        let j0 = ((px[1] as f64 - oy) / cell).floor() as isize;
        let i1 = ((px[2] as f64 - ox) / cell).ceil() as isize;
        let j1 = ((px[3] as f64 - oy) / cell).ceil() as isize;
        Grid { cell, i0, j0, w: (i1 - i0).max(1) as usize, h: (j1 - j0).max(1) as usize, ..self.clone() }
    }

    pub fn len(&self) -> usize {
        self.w * self.h
    }

    /// `fraction` of the face's size, in cells.
    pub fn cells(&self, fraction: f64) -> f64 {
        fraction * self.face / self.cell
    }

    /// Cell (`i`, `j`)'s centre, in pixels.
    pub fn centre(&self, i: usize, j: usize) -> (f64, f64) {
        (self.ox + ((self.i0 + i as isize) as f64 + 0.5) * self.cell, self.oy + ((self.j0 + j as isize) as f64 + 0.5) * self.cell)
    }

    /// Pixel position (`x`, `y`) in cells (cell centres at whole numbers).
    pub fn to_cell(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.ox) / self.cell - 0.5 - self.i0 as f64, (y - self.oy) / self.cell - 0.5 - self.j0 as f64)
    }

    /// Values `v` of grid `from` (same pixels and anchor) read at this grid's cells.
    pub fn resample_from(&self, from: &Grid, v: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; self.len()];
        for j in 0..self.h {
            for i in 0..self.w {
                let (x, y) = self.centre(i, j);
                let (fx, fy) = from.to_cell(x, y);
                if let Some(o) = out.get_mut(j * self.w + i) {
                    *o = bilinear(v, from.w, from.h, fx, fy);
                }
            }
        }
        out
    }
}

/// `v` (`w × h`) at cell position (`fx`, `fy`), bilinear, held at the edges; 0 for a malformed grid.
pub(super) fn bilinear(v: &[f32], w: usize, h: usize, fx: f64, fy: f64) -> f32 {
    if w == 0 || h == 0 || v.len() != w * h || !fx.is_finite() || !fy.is_finite() {
        return 0.0;
    }
    let fx = fx.clamp(0.0, (w - 1) as f64);
    let fy = fy.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let at = |x: usize, y: usize| v.get(y * w + x).copied().unwrap_or(0.0);
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bottom - top) * ty
}

/// 0 below `a`, 1 above `b`, smooth between.
pub(super) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b.partial_cmp(&a) != Some(std::cmp::Ordering::Greater) {
        return if x >= b { 1.0 } else { 0.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
