//! Refine strokes → grids over the photo (normalised, the edit frame), made once per change of the strokes and kept
//! in a small cache, so a render (draft, loupe window or export) only samples them.
//!
//! A grid is a [`PartMask`] (a plane over a normalised area, 0 outside it) whose cells are a sixth of the smallest
//! brush radius: it does not depend on the render's size, so every render samples the same values.

use std::cell::Cell;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_raster::Plane;

use super::{RefineMode, RefineStroke};
use crate::input::{Fnv, NormRect, PartMask};

/// Largest grid side (cells).
const GRID_MAX: usize = 1024;
/// Grid cells across the smallest brush radius.
const CELLS_PER_RADIUS: f64 = 6.0;
/// Most dabs one stroke lays (a hostile stroke spanning the photo with the smallest brush stops there).
const MAX_DABS: usize = 200_000;
/// Memory the cache of grids may hold.
const CACHE_BYTES: usize = 64 << 20;
/// Grids the cache holds at most.
const CACHE_ENTRIES: usize = 32;

thread_local! {
    static MADE: Cell<usize> = const { Cell::new(0) };
}

/// How many grids were rasterised on this thread (a test counts that a render reuses them).
pub fn grids_made() -> usize {
    MADE.with(Cell::get)
}

/// Which grid: how much of the feature is taken away ([`Kind::Cut`], raised by Reduce), or where its masks are
/// widened ([`Kind::Add`], raised by Add).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Cut,
    Add,
}

/// The photo's shape in the edit frame: long-edge units per normalised unit, across and down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    pub kx: f64,
    pub ky: f64,
}

impl Shape {
    /// From the frame's width over its height.
    pub fn from_aspect(a: f64) -> Option<Shape> {
        if !(a.is_finite() && a > 0.0) {
            return None;
        }
        Some(if a >= 1.0 { Shape { kx: 1.0, ky: 1.0 / a } } else { Shape { kx: a, ky: 1.0 } })
    }

    /// The aspect, rounded so two renders of one photo (whose pixel maps differ by rounding) share a grid.
    fn key(self) -> u64 {
        let a = (self.kx / self.ky.max(1e-12) * 1e6).round();
        a.to_bits()
    }
}

/// A stroke `raises` the grid of `kind` (Reduce the cut, Add the add) or lowers it (Restore both, Add the cut).
fn raises(kind: Kind, m: RefineMode) -> bool {
    matches!((kind, m), (Kind::Cut, RefineMode::Reduce) | (Kind::Add, RefineMode::Add))
}

fn lowers(kind: Kind, m: RefineMode) -> bool {
    matches!((kind, m), (_, RefineMode::Restore) | (Kind::Cut, RefineMode::Add))
}

/// The grid of `kind` for `strokes` (only the Add strokes `keep` says yes to raise an Add grid), with its hash;
/// `None` when no stroke raises it.
pub(crate) fn grid(strokes: &[RefineStroke], keep: &dyn Fn(usize) -> bool, kind: Kind, shape: Shape) -> Option<(u64, Arc<PartMask>)> {
    let used: Vec<(usize, &RefineStroke)> =
        strokes.iter().enumerate().filter(|(i, s)| (raises(kind, s.mode) && (kind == Kind::Cut || keep(*i))) || lowers(kind, s.mode)).collect();
    let raising: Vec<&RefineStroke> = used.iter().filter(|(_, s)| raises(kind, s.mode)).map(|(_, s)| *s).collect();
    if raising.is_empty() {
        return None;
    }
    let mut h = Fnv::new();
    h.u64(kind as u64);
    h.u64(shape.key());
    for (_, s) in &used {
        s.hash_into(&mut h);
    }
    let key = h.0;
    if let Some(g) = cached(key) {
        return Some((key, g));
    }
    let g = Arc::new(rasterise(&used, &raising, kind, shape)?);
    store(key, g.clone());
    Some((key, g))
}

static CACHE: Mutex<Vec<(u64, Arc<PartMask>)>> = Mutex::new(Vec::new());

fn cached(key: u64) -> Option<Arc<PartMask>> {
    let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    let i = c.iter().position(|(k, _)| *k == key)?;
    // (the most recently used goes last)
    let e = c.remove(i);
    let g = e.1.clone();
    c.push(e);
    Some(g)
}

fn store(key: u64, g: Arc<PartMask>) {
    let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    c.retain(|(k, _)| *k != key);
    c.push((key, g));
    let bytes = |c: &[(u64, Arc<PartMask>)]| c.iter().map(|(_, g)| g.plane.data.len() * 4).sum::<usize>();
    while c.len() > 1 && (c.len() > CACHE_ENTRIES || bytes(&c) > CACHE_BYTES) {
        c.remove(0);
    }
}

/// The normalised box a stroke paints in (its points grown by its radius).
fn stroke_box(s: &RefineStroke, shape: Shape) -> Option<NormRect> {
    let ([x0, y0, x1, y1], r) = super::points_box(std::iter::once(s))?;
    let (rx, ry) = (r / shape.kx, r / shape.ky);
    Some(NormRect::new(x0 - rx, y0 - ry, x1 + rx, y1 + ry))
}

fn union(a: Option<NormRect>, b: NormRect) -> NormRect {
    match a {
        None => b,
        Some(a) => NormRect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)),
    }
}

/// The grid: over the boxes of the raising strokes, every used stroke stamped in order.
fn rasterise(used: &[(usize, &RefineStroke)], raising: &[&RefineStroke], kind: Kind, shape: Shape) -> Option<PartMask> {
    MADE.with(|c| c.set(c.get().wrapping_add(1)));
    let area = raising.iter().filter_map(|s| stroke_box(s, shape)).fold(None, |a, b| Some(union(a, b)))?;
    if !area.is_valid() {
        return None;
    }
    let (ew, eh) = (area.width() * shape.kx, area.height() * shape.ky);
    let r_min = used.iter().map(|(_, s)| s.size).fold(f64::INFINITY, f64::min);
    let cell = (r_min / CELLS_PER_RADIUS).max(ew.max(eh) / GRID_MAX as f64);
    if !(cell.is_finite() && cell > 0.0) {
        return None;
    }
    let side = |e: f64| ((e / cell).ceil() as usize).clamp(1, GRID_MAX);
    let (w, h) = (side(ew), side(eh));
    let mut out = Plane::new(w, h);
    let g = Grid { area, w, h, shape };
    for (_, s) in used {
        let raise = raises(kind, s.mode);
        stamp(s, &g, &mut out, raise);
    }
    Some(PartMask { plane: out, area })
}

/// Where the grid's cells are.
struct Grid {
    area: NormRect,
    w: usize,
    h: usize,
    shape: Shape,
}

impl Grid {
    /// Cell `i`'s centre across, in long-edge units.
    fn cx(&self, i: usize) -> f64 {
        (self.area.x0 + (i as f64 + 0.5) * self.area.width() / self.w as f64) * self.shape.kx
    }

    fn cy(&self, j: usize) -> f64 {
        (self.area.y0 + (j as f64 + 0.5) * self.area.height() / self.h as f64) * self.shape.ky
    }

    /// The cells whose centres lie within `[lo, hi]` (long-edge units) across.
    fn cols(&self, lo: f64, hi: f64) -> (usize, usize) {
        let unit = self.area.width() * self.shape.kx / self.w as f64;
        let x0 = self.area.x0 * self.shape.kx;
        span(lo, hi, x0, unit, self.w)
    }

    fn rows(&self, lo: f64, hi: f64) -> (usize, usize) {
        let unit = self.area.height() * self.shape.ky / self.h as f64;
        let y0 = self.area.y0 * self.shape.ky;
        span(lo, hi, y0, unit, self.h)
    }
}

/// `[a, b)`: the cells of `n` (each `unit` long from `origin`) whose centres may lie in `[lo, hi]`.
fn span(lo: f64, hi: f64, origin: f64, unit: f64, n: usize) -> (usize, usize) {
    let f = |v: f64| ((v - origin) / unit).clamp(0.0, n as f64);
    let (a, b) = (f(lo).floor() as usize, (f(hi).ceil() as usize + 1).min(n));
    (a.min(b), b)
}

/// One dab: centre (long-edge units), radius, hard radius and flow.
struct Dab {
    x: f64,
    y: f64,
    r: f64,
    hard: f64,
    flow: f32,
}

/// The dabs of a stroke, a quarter of its radius apart; a point where the pointer did not move lays none.
fn dabs(s: &RefineStroke, shape: Shape) -> Vec<Dab> {
    let (r, feather, flow) = (s.size, (s.feather / 100.0).clamp(0.0, 1.0), (s.flow / 100.0).clamp(0.0, 1.0) as f32);
    let dab = |x: f64, y: f64, p: f32| {
        let rd = r * f64::from(p.max(0.1));
        Dab { x, y, r: rd, hard: rd * (1.0 - feather), flow: flow * p }
    };
    let mut out = Vec::new();
    let mut prev: Option<(f64, f64, f32)> = None;
    for (i, (nx, ny)) in s.points.iter().enumerate() {
        if out.len() >= MAX_DABS {
            break;
        }
        let (x, y, p) = (nx * shape.kx, ny * shape.ky, s.pressure_at(i));
        match prev {
            None => out.push(dab(x, y, p)),
            Some((px, py, pp)) => {
                let d = ((x - px).powi(2) + (y - py).powi(2)).sqrt();
                if !(d.is_finite() && d > 0.0) {
                    continue;
                }
                let n = ((d / (r * 0.25).max(1e-6)).ceil() as usize).min(MAX_DABS.saturating_sub(out.len())).max(1);
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    out.push(dab(px + (x - px) * t, py + (y - py) * t, pp + (p - pp) * t as f32));
                }
            }
        }
        prev = Some((x, y, p));
    }
    out
}

/// A dab's paint at distance `dd`: full inside its hard radius, smoothly to none at its radius.
fn dab_alpha(dd: f64, r: f64, hard: f64) -> f32 {
    if dd > r {
        0.0
    } else if dd <= hard || r <= hard {
        1.0
    } else {
        let t = ((dd - hard) / (r - hard)).clamp(0.0, 1.0) as f32;
        1.0 - t * t * (3.0 - 2.0 * t)
    }
}

/// Stamp stroke `s` into `out`: its coverage builds within the stroke up to its strength, then raises `out` there
/// (the larger of the two) or lowers it (`out × (1 − coverage)`).
fn stamp(s: &RefineStroke, g: &Grid, out: &mut Plane, raise: bool) {
    let ds = dabs(s, g.shape);
    let Some(rmax) = ds.iter().map(|d| d.r).reduce(f64::max) else { return };
    let (lo_x, hi_x) = ds.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), d| (a.min(d.x), b.max(d.x)));
    let (lo_y, hi_y) = ds.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), d| (a.min(d.y), b.max(d.y)));
    let (x0, x1) = g.cols(lo_x - rmax, hi_x + rmax);
    let (y0, y1) = g.rows(lo_y - rmax, hi_y + rmax);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let dens = (s.strength / 100.0).clamp(0.0, 1.0) as f32;
    let bw = x1 - x0;
    let mut cov = vec![0.0f32; bw * (y1 - y0)];
    for d in &ds {
        let (cx0, cx1) = g.cols(d.x - d.r, d.x + d.r);
        let (cy0, cy1) = g.rows(d.y - d.r, d.y + d.r);
        for j in cy0.max(y0)..cy1.min(y1) {
            let dy = g.cy(j) - d.y;
            for i in cx0.max(x0)..cx1.min(x1) {
                let dx = g.cx(i) - d.x;
                let a = dab_alpha((dx * dx + dy * dy).sqrt(), d.r, d.hard);
                if a <= 0.0 {
                    continue;
                }
                if let Some(v) = cov.get_mut((j - y0) * bw + (i - x0)) {
                    // flow builds within a stroke up to its strength (a full dab at full flow reaches it at once)
                    *v = if a * d.flow >= 1.0 { dens } else { (*v + a * d.flow * (1.0 - *v)).min(dens) };
                }
            }
        }
    }
    for j in y0..y1 {
        for i in x0..x1 {
            let (Some(c), Some(o)) = (cov.get((j - y0) * bw + (i - x0)).copied(), out.data.get_mut(j * g.w + i)) else { continue };
            *o = if raise { o.max(c) } else { *o * (1.0 - c) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(mode: RefineMode, points: &[(f64, f64)]) -> RefineStroke {
        RefineStroke { mode, points: points.to_vec(), size: 0.05, feather: 0.0, ..RefineStroke::default() }
    }

    #[test]
    fn a_full_reduce_is_one_inside_and_restore_takes_it_back() {
        let shape = Shape::from_aspect(1.5).unwrap();
        let s = vec![stroke(RefineMode::Reduce, &[(0.5, 0.5)])];
        let (_, g) = grid(&s, &|_| true, Kind::Cut, shape).unwrap();
        assert_eq!(g.sample(0.5, 0.5), 1.0);
        assert_eq!(g.sample(0.9, 0.5), 0.0);
        let mut s2 = s.clone();
        s2.push(stroke(RefineMode::Restore, &[(0.5, 0.5)]));
        let (_, g) = grid(&s2, &|_| true, Kind::Cut, shape).unwrap();
        assert_eq!(g.sample(0.5, 0.5), 0.0);
        assert!(grid(&[stroke(RefineMode::Restore, &[(0.5, 0.5)])], &|_| true, Kind::Cut, shape).is_none(), "nothing to restore");
    }

    #[test]
    fn a_grid_is_made_once_per_change() {
        let shape = Shape::from_aspect(1.0).unwrap();
        let s = vec![stroke(RefineMode::Add, &[(0.2, 0.2), (0.3, 0.25)])];
        let before = grids_made();
        let a = grid(&s, &|_| true, Kind::Add, shape).unwrap();
        let b = grid(&s, &|_| true, Kind::Add, shape).unwrap();
        assert!(Arc::ptr_eq(&a.1, &b.1));
        assert_eq!(grids_made() - before, 1);
        assert!(grid(&s, &|_| false, Kind::Add, shape).is_none(), "a stroke kept for another face");
    }

    #[test]
    fn hostile_strokes_stay_bounded() {
        let shape = Shape::from_aspect(1.0).unwrap();
        let wild: Vec<(f64, f64)> = (0..5000).map(|i| (if i % 2 == 0 { -1.0 } else { 2.0 }, 0.5)).collect();
        let s = RefineStroke { size: 0.001, ..stroke(RefineMode::Reduce, &wild) }.sanitized().unwrap();
        let (_, g) = grid(&[s], &|_| true, Kind::Cut, shape).unwrap();
        assert!(g.plane.width <= GRID_MAX && g.plane.height <= GRID_MAX);
        assert!(Shape::from_aspect(f64::NAN).is_none() && Shape::from_aspect(0.0).is_none());
    }
}
