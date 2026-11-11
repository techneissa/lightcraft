//! Mask positions through a lens or perspective warp, from a coarse grid (issue #212).
//!
//! A linear or radial mask reads each output pixel's position on the photo before the warp
//! ([`Frame::out_to_mask`]). Sending every pixel through the warp in `f64` was most of a masked render's CPU on a
//! photo whose lens correction moves pixels. The warp is smooth, so [`PosGrid`] evaluates it exactly every [`CELL`]
//! output pixels and interpolates bilinearly in between.
//!
//! Each cell measures its own interpolation error. A mask ([`Ramp`]) takes the interpolated positions in a cell where
//! that error moves its alpha by at most [`ALPHA_TOL`] given how steep the mask is, or where its alpha is the same
//! (0 or 1) all over the cell. Other cells (the edge of a hard mask, a perspective near its horizon, a Face Reshape's
//! areas) take the exact per-pixel path, so the result is never further from the exact one than that.
//!
//! The grid is anchored to the whole output: a window of a zoomed view has the grid points of the whole render, so
//! its pixels equal the same pixels of the whole frame.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_geom::{Point, Vec2};
use lightcraft_raster::Plane;

use crate::geometry::{Frame, MaskMap};

/// Output pixels per grid cell. A power of two: the GPU finds a pixel's cell and its place in it with the same
/// integer arithmetic.
pub const CELL: usize = 16;

/// The most interpolated positions may move a mask's alpha: a quarter of one 8-bit level.
pub const ALPHA_TOL: f64 = 0.25 / 255.0;

/// The measured error times this bounds a cell's error. Bilinear interpolation reproduces the warp's constant,
/// linear and `x·y` terms; its error from the `x²` and `y²` terms peaks at the edge midpoints and the centre, which
/// is where it is measured. Higher terms add about `CELL³` times the warp's third derivative: far below this
/// margin for a lens or perspective warp, which bends over hundreds of pixels.
const SAFETY: f64 = 2.0;

/// Grids kept for the frames last evaluated: every part of every mask of a render shares one.
const KEPT: usize = 4;

/// Each output pixel's mask position (in long-edge units), interpolated from the warp's positions every
/// [`CELL`] pixels. See the module documentation.
#[derive(Debug)]
pub struct PosGrid {
    /// Cells across. Grid point `(i, j)` sits at whole-output pixel position `((i0 + i)·CELL, (j0 + j)·CELL)`.
    cols: usize,
    /// This output's offset in the whole output (a window's), and the first grid column and row it touches.
    ox: usize,
    oy: usize,
    i0: usize,
    j0: usize,
    /// `(cols + 1) × (rows + 1)` source positions (px, before a Face Reshape), row by row.
    nodes: Vec<Point>,
    /// Per cell, row by row.
    cells: Vec<Cell>,
    /// Long edge of the oriented photo (source px per long-edge unit).
    long: f64,
    /// The exact mapping.
    map: MaskMap,
    ow: f64,
    oh: f64,
}

/// What a grid cell knows of its positions (long-edge units).
#[derive(Clone, Copy, Debug, Default)]
struct Cell {
    /// A bound on how far an interpolated position is from the exact one; infinite where every pixel is mapped exactly.
    err: f64,
    /// A disc holding every position of the cell, exact or interpolated: its centre and radius (infinite with `err`).
    centre: Point,
    reach: f64,
}

/// How a mask evaluates a cell ([`PosGrid::fill`]).
#[derive(Clone, Copy, Debug)]
enum Way {
    /// Every pixel through the exact warp.
    Exact,
    /// Every pixel's position interpolated.
    Interpolated,
    /// The mask is the same over the whole cell: its value at this position (long-edge units).
    Flat(Point),
}

/// What a linear or radial mask's alpha follows, to choose the cells it interpolates: a value `u` of the position (a
/// linear's `t`, a radial's normalized radius) that changes by at most [`Ramp::rate`] per long-edge unit, and the band
/// of `u` where the alpha ramps, by smoothstep. Outside the band the alpha is 0 or 1.
#[derive(Clone, Copy, Debug)]
pub enum Ramp {
    /// `t = (p − a)·d / |d|²`, ramping over `0..1`.
    Linear { a: Point, d: Vec2 },
    /// The radius of `p − c` turned by `(cos, sin)` and scaled by `1 / (rx, ry)`, ramping over `1 − band..1`.
    Radial { c: Point, cos: f64, sin: f64, rx: f64, ry: f64, band: f64 },
}

impl Ramp {
    /// The ramp of a linear gradient from `a` to `a + d` (long-edge units; `d` may be zero).
    pub fn linear(a: Point, d: Vec2) -> Ramp {
        Ramp::Linear { a, d }
    }

    /// The ramp of a radial gradient: centre `c`, semi-axes `rx, ry` (long-edge units), turned by `angle` degrees,
    /// `feather` 0..1 of its radius.
    pub fn radial(c: Point, rx: f64, ry: f64, angle: f64, feather: f64) -> Ramp {
        let (sin, cos) = (-angle.to_radians()).sin_cos();
        Ramp::Radial { c, cos, sin, rx: rx.max(1e-6), ry: ry.max(1e-6), band: feather.clamp(0.0, 1.0).max(0.01) }
    }

    /// `u` at position `p`.
    pub fn value(&self, p: Point) -> f64 {
        match *self {
            Ramp::Linear { a, d } => (p - a).dot(d) / d.dot(d).max(1e-12),
            Ramp::Radial { c, cos, sin, rx, ry, .. } => {
                let d = p - c;
                let (x, y) = (d.x * cos - d.y * sin, d.x * sin + d.y * cos);
                ((x / rx).powi(2) + (y / ry).powi(2)).sqrt()
            }
        }
    }

    /// The most `u` changes per long-edge unit.
    fn rate(&self) -> f64 {
        match *self {
            Ramp::Linear { d, .. } => d.dot(d).sqrt() / d.dot(d).max(1e-12),
            Ramp::Radial { rx, ry, .. } => 1.0 / rx.min(ry),
        }
    }

    /// The band of `u` the alpha ramps over.
    fn band(&self) -> (f64, f64) {
        match *self {
            Ramp::Linear { .. } => (0.0, 1.0),
            Ramp::Radial { band, .. } => (1.0 - band, 1.0),
        }
    }

    /// The most the alpha changes per long-edge unit: smoothstep's 1.5 over the band.
    pub fn slope(&self) -> f64 {
        let (lo, hi) = self.band();
        1.5 * self.rate() / (hi - lo)
    }

    /// Whether the alpha is the same all over the disc of radius `r` about `c`: `u` stays out of the band (with a
    /// margin for the `f32` the alpha is computed in).
    fn flat(&self, c: Point, r: f64) -> bool {
        let (u, reach, (lo, hi)) = (self.value(c), r * self.rate(), self.band());
        u + reach <= lo - 1e-6 || u - reach >= hi + 1e-6
    }
}

/// The kept grids: frame, output size, grid.
type Kept = Vec<(Frame, usize, usize, Arc<PosGrid>)>;
static GRIDS: Mutex<Kept> = Mutex::new(Vec::new());

/// The position grid of `frame`'s `w × h` output, when its masks go through a warp ([`Frame::mask_warp`]); `None`
/// when they don't (the affine positions are cheap and exact). Kept for the last few frames.
pub fn pos_grid(frame: &Frame, w: usize, h: usize) -> Option<Arc<PosGrid>> {
    frame.mask_warp()?;
    #[cfg(test)]
    if EXACT.get() {
        return None;
    }
    if w == 0 || h == 0 {
        return None;
    }
    let lock = || GRIDS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(g) = lock().iter().find(|(f, gw, gh, _)| (*gw, *gh) == (w, h) && f == frame).map(|e| e.3.clone()) {
        return Some(g);
    }
    let grid = Arc::new(PosGrid::build(frame, w, h));
    let mut kept = lock();
    kept.retain(|(f, gw, gh, _)| !((*gw, *gh) == (w, h) && f == frame));
    kept.insert(0, (frame.clone(), w, h, grid.clone()));
    kept.truncate(KEPT);
    Some(grid)
}

/// The largest of `a` and `b`, where a NaN counts as infinite (an error that can't be measured is never small).
fn worse(a: f64, b: f64) -> f64 {
    if b.is_nan() { f64::INFINITY } else { a.max(b) }
}

impl PosGrid {
    /// The grid of `frame`'s `w × h` output (both at least 1).
    fn build(frame: &Frame, w: usize, h: usize) -> PosGrid {
        let map = frame.out_to_mask(w, h);
        // a window's pixels sit at its offset in the whole output (whole pixels: `Frame::window`)
        let (ox, oy) = frame.view.map_or((0, 0), |v| (v.x.max(0.0) as usize, v.y.max(0.0) as usize));
        let (i0, j0) = (ox / CELL, oy / CELL);
        let cols = ox.saturating_add(w).div_ceil(CELL).saturating_sub(i0).max(1);
        let rows = oy.saturating_add(h).div_ceil(CELL).saturating_sub(j0).max(1);
        let c = CELL as f64;
        // output position (px) of grid coordinates (i, j), fractional between grid points
        let at = |i: f64, j: f64| Point::new((i0 as f64 + i) * c - ox as f64, (j0 as f64 + j) * c - oy as f64);
        let warped = |i: f64, j: f64| map.warped_px(at(i, j)).unwrap_or(Point::new(f64::NAN, f64::NAN));
        let stride = cols + 1;
        let mut nodes = vec![Point::new(0.0, 0.0); stride * (rows + 1)];
        crate::for_rows(&mut nodes, stride, |j, row| {
            for (i, n) in row.iter_mut().enumerate() {
                *n = warped(i as f64, j as f64);
            }
        });
        let long = frame.ow.max(frame.oh).max(1e-9);
        // a Face Reshape's areas (px): no grid there (its own grid is not smooth)
        let reshape: Vec<[f64; 4]> = map
            .warp()
            .and_then(|wp| wp.reshape.as_ref().map(|f| (wp.w, wp.h, f)))
            .map(|(pw, ph, f)| f.areas().iter().map(|a| [a[0] * pw, a[1] * ph, a[2] * pw, a[3] * ph]).collect())
            .unwrap_or_default();
        let node = |i: usize, j: usize| nodes.get(j * stride + i).copied().unwrap_or(Point::new(f64::NAN, f64::NAN));
        let mut cells = vec![Cell::default(); cols * rows];
        crate::for_rows(&mut cells, cols, |j, row| {
            for (i, e) in row.iter_mut().enumerate() {
                let (n00, n10, n01, n11) = (node(i, j), node(i + 1, j), node(i, j + 1), node(i + 1, j + 1));
                let mid = |a: Point, b: Point| Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                let (fi, fj) = (i as f64, j as f64);
                let checks = [
                    (fi + 0.5, fj, mid(n00, n10)),
                    (fi + 0.5, fj + 1.0, mid(n01, n11)),
                    (fi, fj + 0.5, mid(n00, n01)),
                    (fi + 1.0, fj + 0.5, mid(n10, n11)),
                    (fi + 0.5, fj + 0.5, mid(mid(n00, n10), mid(n01, n11))),
                ];
                let px = checks.iter().fold(0.0, |m, &(u, v, q)| worse(m, warped(u, v).dist(q))) * SAFETY;
                let (lo, hi) = [n00, n10, n01, n11].iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
                    ([lo[0].min(p.x), lo[1].min(p.y)], [hi[0].max(p.x), hi[1].max(p.y)])
                });
                // (the cell's positions lie within its corners' box and its error; a pixel more for the reshape's own
                // bilinear grid)
                let m = px + 1.0;
                let reshaped = reshape.iter().any(|a| lo[0] - m <= a[2] && hi[0] + m >= a[0] && lo[1] - m <= a[3] && hi[1] + m >= a[1]);
                let err = if reshaped || !px.is_finite() { f64::INFINITY } else { px / long };
                // (a bilinear patch lies within its corners' hull)
                let centre = mid(mid(n00, n10), mid(n01, n11));
                let hull = [n00, n10, n01, n11].iter().fold(0.0, |m, p| worse(m, p.dist(centre)));
                let reach = worse(hull / long + err, err);
                *e = Cell { err, centre: Point::new(centre.x / long, centre.y / long), reach };
            }
        });
        PosGrid { cols, ox, oy, i0, j0, nodes, cells, long, ow: frame.ow, oh: frame.oh, map }
    }

    /// Per cell, how a mask following `ramp` evaluates it, with positions `extra` (long-edge units) further off than
    /// the grid's.
    fn plan(&self, ramp: &Ramp, extra: f64) -> Vec<Way> {
        let slope = ramp.slope();
        let mut ways = vec![Way::Exact; self.cells.len()];
        crate::for_rows(&mut ways, self.cols, |j, row| {
            for (o, c) in row.iter_mut().zip(self.cells.get(j * self.cols..).unwrap_or(&[])) {
                *o = if ramp.flat(c.centre, c.reach + extra) {
                    Way::Flat(c.centre)
                } else if (c.err + extra) * slope <= ALPHA_TOL {
                    Way::Interpolated
                } else {
                    Way::Exact
                };
            }
        });
        ways
    }

    /// Whether a mask following `ramp` may take every cell's interpolated positions, evaluated with positions `extra`
    /// further off (the GPU's `f32` rounding).
    pub fn fits(&self, ramp: &Ramp, extra: f64) -> bool {
        self.plan(ramp, extra).iter().all(|w| !matches!(w, Way::Exact))
    }

    /// Fill `out` with `f` of each pixel's mask position (long-edge units) for a mask following `ramp`: interpolated
    /// in the cells where that moves its alpha by at most [`ALPHA_TOL`] (or not at all), exact in the others.
    ///
    /// A cell the mask is flat over (all 0 or all 1, at the exact positions too) is that value, without a position
    /// per pixel: most of a frame, for most masks.
    pub fn fill(&self, out: &mut Plane, ramp: &Ramp, f: impl Fn(Point) -> f32 + Sync + Send) {
        let ways = self.plan(ramp, 0.0);
        let w = out.width;
        let inv = 1.0 / self.long;
        let (ow, oh, l) = (self.ow, self.oh, self.ow.max(self.oh));
        let stride = self.cols + 1;
        let reshape = self.map.warp().filter(|wp| wp.reshape.is_some());
        let c = CELL as f64;
        crate::for_rows(&mut out.data, w, |y, row| {
            let gy = y + self.oy;
            let j = (gy / CELL).saturating_sub(self.j0);
            let fy = ((gy % CELL) as f64 + 0.5) / c;
            let (Some(top), Some(bottom)) = (self.nodes.get(j * stride..(j + 1) * stride), self.nodes.get((j + 1) * stride..(j + 2) * stride)) else {
                return;
            };
            let ways = ways.get(j * self.cols..(j + 1) * self.cols).unwrap_or(&[]);
            // the row's pixels, cell by cell (`x`: the cell's first)
            let mut x = 0;
            for (i, ((t, b), way)) in top.windows(2).zip(bottom.windows(2)).zip(ways).enumerate() {
                let end = ((self.i0 + i + 1) * CELL).saturating_sub(self.ox).min(w);
                let Some(cells) = row.get_mut(x..end) else { break };
                if let Way::Flat(centre) = way {
                    cells.fill(f(*centre));
                } else if matches!(way, Way::Interpolated)
                    && let ([t0, t1], [b0, b1]) = (t, b)
                {
                    let lerp = |a: Point, b: Point, t: f64| Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
                    let (a, b) = (lerp(*t0, *b0, fy), lerp(*t1, *b1, fy));
                    for (k, v) in cells.iter_mut().enumerate() {
                        let fx = (((self.ox + x + k) % CELL) as f64 + 0.5) / c;
                        let mut g = lerp(a, b, fx);
                        if let Some(wp) = reshape {
                            g = wp.reshaped(g);
                        }
                        *v = f(Point::new(g.x * inv, g.y * inv));
                    }
                } else {
                    for (k, v) in cells.iter_mut().enumerate() {
                        let n = self.map.apply(Point::new((x + k) as f64 + 0.5, y as f64 + 0.5));
                        *v = f(Point::new(n.x * ow / l, n.y * oh / l));
                    }
                }
                x = end;
            }
        });
    }

    /// The grid for the GPU's `shape` kernel: its points in long-edge units (`x, y` pairs, row by row) and the kernel
    /// parameters that find a pixel's cell: `[ox, oy, i0, j0, points per row, CELL]`.
    pub fn gpu_table(&self) -> (Vec<f32>, [u32; 6]) {
        let inv = 1.0 / self.long;
        let pts = self.nodes.iter().flat_map(|p| [(p.x * inv) as f32, (p.y * inv) as f32]).collect();
        let u = |v: usize| u32::try_from(v).unwrap_or(u32::MAX);
        (pts, [u(self.ox), u(self.oy), u(self.i0), u(self.j0), u(self.cols + 1), u(CELL)])
    }
}

#[cfg(test)]
thread_local! {
    static EXACT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `f` with every mask position mapped exactly, as before the grid (the reference the tests hold it to).
#[cfg(test)]
pub(crate) fn exactly<T>(f: impl FnOnce() -> T) -> T {
    EXACT.set(true);
    let r = f();
    EXACT.set(false);
    r
}

/// How many of the grid's cells a mask following `ramp` interpolates, of all of them.
#[cfg(test)]
pub(crate) fn interpolated_cells(g: &PosGrid, ramp: &Ramp) -> (usize, usize) {
    let ways = g.plan(ramp, 0.0);
    (ways.iter().filter(|w| !matches!(w, Way::Exact)).count(), ways.len())
}
