//! The reduced grid Skin Tone measures colour on: cells a fixed fraction of the face, laid over the pixels a face's
//! masks can touch, so the work per slider move is a few thousand cells plus one light pass over the pixels.

use lightcraft_raster::Image;
use lightcraft_raster::resample::Pixel;

/// Most cells a grid may have (a face far larger than the picture, or a huge body mask, gets coarser cells).
const MAX_CELLS: f64 = (1 << 18) as f64;

/// A grid of `gw × gh` cells of `cell` pixels over the image region `[x0, x0 + w) × [y0, y0 + h)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Grid {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    pub cell: f64,
    pub gw: usize,
    pub gh: usize,
}

impl Grid {
    /// A grid over `region` (`[x0, y0, x1, y1)`) with cells of `face_size / per_face` pixels (at least one).
    pub fn new(region: [usize; 4], face_size: f64, per_face: f64) -> Option<Grid> {
        let [x0, y0, x1, y1] = region;
        let (w, h) = (x1.checked_sub(x0)?, y1.checked_sub(y0)?);
        if w == 0 || h == 0 || !face_size.is_finite() || face_size <= 0.0 {
            return None;
        }
        let mut cell = (face_size / per_face).max(1.0);
        let count = |c: f64| (w as f64 / c).ceil() * (h as f64 / c).ceil();
        if count(cell) > MAX_CELLS {
            cell *= (count(cell) / MAX_CELLS).sqrt() * 1.01;
        }
        let gw = ((w as f64 / cell).ceil() as usize).max(1);
        let gh = ((h as f64 / cell).ceil() as usize).max(1);
        Some(Grid { x0, y0, w, h, cell, gw, gh })
    }

    /// `fraction` of the face's size in cells.
    pub fn sigma(&self, fraction: f64, face_size: f64) -> f32 {
        (fraction * face_size / self.cell) as f32
    }

    /// The column (or row) of cells region pixel `p` falls in.
    pub fn cell_of(&self, p: usize, cells: usize) -> usize {
        ((p as f64 / self.cell) as usize).min(cells.saturating_sub(1))
    }

    /// The first region row of grid row `gy` (rows `start(gy)..start(gy + 1)` fall in it).
    pub fn start(&self, gy: usize) -> usize {
        ((gy as f64 * self.cell).ceil() as usize).min(self.h)
    }

    /// The image pixel at the centre of cell (`gx`, `gy`).
    pub fn centre(&self, gx: usize, gy: usize) -> (usize, usize) {
        let at = |g: usize, len: usize| (((g as f64 + 0.5) * self.cell) as usize).min(len.saturating_sub(1));
        (self.x0 + at(gx, self.w), self.y0 + at(gy, self.h))
    }

    /// `g` (one value per cell) at region pixel (`px`, `py`), bilinear between cell centres.
    pub fn at<T: Pixel>(&self, g: &Image<T>, px: usize, py: usize) -> T {
        let axis = |p: usize, n: usize| {
            let f = ((p as f64 + 0.5) / self.cell - 0.5).clamp(0.0, n.saturating_sub(1) as f64);
            let i = f.floor() as usize;
            (i, (i + 1).min(n.saturating_sub(1)), (f - i as f64) as f32)
        };
        let ((x0, x1, tx), (y0, y1, ty)) = (axis(px, g.width), axis(py, g.height));
        let v = |x: usize, y: usize| g.data.get(y * g.width + x).copied().unwrap_or_else(T::zero);
        T::zero().madd(v(x0, y0), (1.0 - tx) * (1.0 - ty)).madd(v(x1, y0), tx * (1.0 - ty)).madd(v(x0, y1), (1.0 - tx) * ty).madd(v(x1, y1), tx * ty)
    }
}

/// The smallest box holding both (`[x0, y0, x1, y1)`).
pub(super) fn union(a: Option<[usize; 4]>, b: Option<[usize; 4]>) -> Option<[usize; 4]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]),
        (a, b) => a.or(b),
    }
}

/// 0 below `lo`, 1 above `hi`, smooth between.
pub(super) fn smoothstep(lo: f32, hi: f32, v: f32) -> f32 {
    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    if t.is_finite() { t * t * (3.0 - 2.0 * t) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_follow_the_face_and_stay_bounded() {
        let g = Grid::new([10, 20, 410, 320], 256.0, 128.0).unwrap();
        assert_eq!((g.cell, g.gw, g.gh), (2.0, 200, 150));
        // a tiny face: cells are never under one pixel
        assert_eq!(Grid::new([0, 0, 50, 40], 10.0, 128.0).unwrap().cell, 1.0);
        // a huge region gets coarser cells
        let big = Grid::new([0, 0, 100_000, 100_000], 100_000.0, 1.0e6).unwrap();
        assert!((big.gw * big.gh) as f64 <= MAX_CELLS);
        assert!(Grid::new([5, 5, 5, 9], 100.0, 128.0).is_none());
        assert!(Grid::new([0, 0, 9, 9], f64::NAN, 128.0).is_none());
    }

    #[test]
    fn every_region_row_falls_in_one_grid_row() {
        let g = Grid::new([0, 0, 97, 61], 300.0, 128.0).unwrap();
        let rows: usize = (0..g.gh).map(|gy| g.start(gy + 1) - g.start(gy)).sum();
        assert_eq!(rows, g.h);
        for py in 0..g.h {
            let gy = g.cell_of(py, g.gh);
            assert!((g.start(gy)..g.start(gy + 1)).contains(&py), "row {py} in grid row {gy}");
        }
    }

    #[test]
    fn bilinear_reads_cell_centres_exactly() {
        let g = Grid::new([0, 0, 8, 8], 8.0, 2.0).unwrap();
        let img = Image::<f32>::from_fn(2, 2, |x, y| (x + 2 * y) as f32);
        assert_eq!(g.cell, 4.0);
        // cell centres are at 2 and 6 (pixel 5's centre is 5.5: 7/8 of the way); the top rows hold the first row
        assert!((g.at(&img, 5, 1) - 0.875).abs() < 1e-6);
        assert!((g.at(&img, 0, 0) - 0.0).abs() < 1e-6);
        assert!((g.at(&img, 7, 7) - 3.0).abs() < 1e-6);
    }
}
