//! Blurs of the background on grids of blocks of the analysis image (the whole frame at a fixed size).
//!
//! A blur of `R` pixels runs on blocks of about `R / 3` pixels (the image weighted by how much of each pixel is
//! background, summed per block), with a disc of `R` (a lens's out-of-focus spot) or a Gaussian, and is read back
//! bilinearly at any point of the image (a render pixel mapped onto it), so every render and every window of one
//! reads the same values.

use lightcraft_raster::{Image, par_rows};

/// Blocks of `s` × `s` render pixels: per block the weighted colour sum (`rgb · w`) and the weight (`w`), each
/// divided by `s²` (a block the image only partly covers counts for what it covers).
#[derive(Clone, Debug)]
pub(crate) struct Grid {
    /// Block size, in render pixels.
    pub s: usize,
    /// The global index of the first block column and row.
    pub b0: (i64, i64),
    pub cells: Image<[f32; 4]>,
}

/// A colour and its weight at one pixel (the weight 0 to leave it out).
pub(crate) type Weighted = ([f32; 3], f32);

impl Grid {
    /// Pool `f` over a `w × h` image whose top-left pixel is `origin` in the render, in blocks of `s` pixels.
    pub fn pool(w: usize, h: usize, origin: (i64, i64), s: usize, f: &(impl Fn(usize, usize) -> Weighted + Sync)) -> Grid {
        let s = s.max(1);
        let si = s as i64;
        let (gx0, gy0) = (origin.0.div_euclid(si), origin.1.div_euclid(si));
        let (last_x, last_y) = ((origin.0 + w as i64 - 1).div_euclid(si), (origin.1 + h as i64 - 1).div_euclid(si));
        if w == 0 || h == 0 {
            return Grid { s, b0: (gx0, gy0), cells: Image::new(0, 0) };
        }
        let (gw, gh) = ((last_x - gx0 + 1) as usize, (last_y - gy0 + 1) as usize);
        let mut cells = Image::<[f32; 4]>::new(gw, gh);
        let norm = 1.0 / (s * s) as f32;
        par_rows(&mut cells.data, gw, |by, row| {
            // the image rows of block row `by`
            let top = (gy0 + by as i64) * si - origin.1;
            let (y0, y1) = (top.max(0) as usize, ((top + si).max(0) as usize).min(h));
            for y in y0..y1 {
                for x in 0..w {
                    let bx = ((origin.0 + x as i64).div_euclid(si) - gx0) as usize;
                    let (c, wt) = f(x, y);
                    if wt > 0.0
                        && let Some(cell) = row.get_mut(bx)
                    {
                        cell[0] += c[0] * wt;
                        cell[1] += c[1] * wt;
                        cell[2] += c[2] * wt;
                        cell[3] += wt;
                    }
                }
            }
            for cell in row.iter_mut() {
                *cell = cell.map(|v| v * norm);
            }
        });
        Grid { s, b0: (gx0, gy0), cells }
    }

    /// Convolved with a disc of `r` blocks (its rim anti-aliased over one block), normalised: a lens's blur.
    pub fn disc(&self, r: f32) -> Grid {
        let r = if r.is_finite() { r.clamp(0.0, 64.0) } else { 0.0 };
        let n = r.ceil() as i64 + 1;
        let mut taps: Vec<(i64, i64, f32)> = Vec::new();
        for dy in -n..=n {
            for dx in -n..=n {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                let wt = (r + 0.5 - d).clamp(0.0, 1.0);
                if wt > 0.0 {
                    taps.push((dx, dy, wt));
                }
            }
        }
        let total: f32 = taps.iter().map(|t| t.2).sum();
        let k = if total > 0.0 { 1.0 / total } else { 1.0 };
        let (w, h) = (self.cells.width, self.cells.height);
        let mut out = Image::<[f32; 4]>::new(w, h);
        let src = &self.cells;
        par_rows(&mut out.data, w.max(1), |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let mut acc = [0.0f32; 4];
                for &(dx, dy, wt) in &taps {
                    let (sx, sy) = (x as i64 + dx, y as i64 + dy);
                    if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
                        continue;
                    }
                    if let Some(v) = src.data.get(sy as usize * w + sx as usize) {
                        acc.iter_mut().zip(v).for_each(|(a, v)| *a += v * wt);
                    }
                }
                *o = acc.map(|v| v * k);
            }
        });
        Grid { s: self.s, b0: self.b0, cells: out }
    }

    /// Blurred by a Gaussian of `sigma` blocks.
    pub fn gauss(&self, sigma: f32) -> Grid {
        if !(sigma.is_finite() && sigma > 0.0) || self.cells.data.is_empty() {
            return self.clone();
        }
        Grid { s: self.s, b0: self.b0, cells: lightcraft_raster::blur::gaussian(&self.cells, sigma) }
    }

    /// Where image point (`u`, `v`) (pixel centres at whole numbers) reads the grid: four cells and their bilinear
    /// weights.
    pub fn taps(&self, u: f32, v: f32) -> Option<([usize; 4], [f32; 4])> {
        let (w, h) = (self.cells.width, self.cells.height);
        if w == 0 || h == 0 || !u.is_finite() || !v.is_finite() {
            return None;
        }
        let s = self.s as f32;
        let cu = ((u + 0.5) / s - 0.5 - self.b0.0 as f32).clamp(0.0, (w - 1) as f32);
        let cv = ((v + 0.5) / s - 0.5 - self.b0.1 as f32).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (cu as usize, cv as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (cu - x0 as f32, cv - y0 as f32);
        Some(([y0 * w + x0, y0 * w + x1, y1 * w + x0, y1 * w + x1], [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty]))
    }

    /// The colour at image point (`u`, `v`), when enough background is near it (`min` of the weight).
    pub fn read(&self, u: f32, v: f32, min: f32) -> Option<[f32; 3]> {
        let (at, wt) = self.taps(u, v)?;
        let mut p = [0.0f32; 4];
        for (i, k) in at.iter().zip(wt) {
            let v = self.cells.data.get(*i)?;
            p.iter_mut().zip(v).for_each(|(a, v)| *a += v * k);
        }
        (p[3] > min).then(|| [p[0] / p[3], p[1] / p[3], p[2] / p[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window's grid pools the same pixels into the same blocks as the whole image's.
    #[test]
    fn a_window_pools_into_the_whole_images_blocks() {
        let f = |ox: i64, oy: i64| move |x: usize, y: usize| ([(x as i64 + ox) as f32, (y as i64 + oy) as f32, 1.0], 1.0);
        let whole = Grid::pool(40, 30, (0, 0), 4, &f(0, 0));
        let part = Grid::pool(20, 12, (8, 12), 4, &f(8, 12));
        assert_eq!(part.b0, (2, 3));
        for y in 0..part.cells.height {
            for x in 0..part.cells.width {
                assert_eq!(part.cells.get(x, y), whole.cells.get(x + 2, y + 3));
            }
        }
        // a window that starts inside a block
        let odd = Grid::pool(10, 10, (5, 7), 4, &f(5, 7));
        assert_eq!(odd.b0, (1, 1));
        let (a, b) = (odd.read(9.0, 10.0, 0.0), whole.read(9.0, 10.0, 0.0));
        assert!(a.is_some() && b.is_some());
    }

    #[test]
    fn a_disc_keeps_a_flat_field_flat_and_reads_nothing_where_nothing_was_pooled() {
        let g = Grid::pool(32, 32, (0, 0), 2, &|x, _| ([0.5, 0.25, 0.125], if x < 16 { 1.0 } else { 0.0 }));
        let d = g.disc(3.0);
        let p = d.read(4.0, 16.0, 1e-3).unwrap();
        assert!((p[0] - 0.5).abs() < 1e-5 && (p[2] - 0.125).abs() < 1e-5);
        assert!(d.read(31.0, 16.0, 1e-3).is_none());
        assert!(Grid::pool(0, 0, (0, 0), 3, &|_, _| ([0.0; 3], 1.0)).read(0.0, 0.0, 0.0).is_none());
        assert!(d.read(f32::NAN, 1.0, 0.0).is_none());
    }
}
