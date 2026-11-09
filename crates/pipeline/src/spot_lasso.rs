//! Fork feature (`fork/brush-area.md`): the area a brushed Remove, Heal or Clone spot covers, as one shape.
//!
//! A brushed spot covers the union of discs of its radius along its stroke. This module measures that union on a
//! grid ([`cover`]) so the Remove tool can draw it as ONE outline (and, while painting, one filled shape), and turns a
//! stroke that comes back to where it started into a lasso: the spot then covers the inside of the loop too
//! ([`lasso_area`], stored as a shaped spot, `fork/object-remove.md`).
//!
//! Everything works in "long units" (the long edge of the picture is 1, so a radius is a disc), with points taken
//! from the uncropped picture's normalized coordinates and the picture's size. Strokes are hostile input (a pointer
//! path, a command, an agent): points that are not numbers are dropped, far-off ones are clamped, the path is thinned
//! to [`MAX_POINTS`] and the grid never exceeds the side asked for.

use lightcraft_develop::SegMask;
use lightcraft_geom::Point;

use crate::find::shape::{Mask, components, distance_sq, simplify, trace_outline};

/// Most stroke points measured; a longer stroke is thinned evenly (first and last kept).
pub const MAX_POINTS: usize = 2048;
/// A stroke is a loop when its end is within this many radii of its start.
pub const CLOSE_RADII: f64 = 2.0;
/// The shortest stroke that can be a loop, in radii of path.
const MIN_LOOP_LENGTH: f64 = 6.0;
/// The least the loop encloses, in squared radii (a stroke that goes out and comes back encloses nothing).
const MIN_LOOP_AREA: f64 = 4.0;
/// Largest grid side [`outline`] and [`lasso_area`] measure on.
pub const OUTLINE_SIDE: usize = 640;
/// Grid cells to a radius at least, so a small spot is still a smooth disc.
const CELLS_PER_RADIUS: f64 = 10.0;
/// Coordinates (long units) the stroke is clamped to.
const FAR: f64 = 4.0;

/// The union of discs along a stroke (plus the inside of a loop) on a grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Cover {
    /// Where cell (0, 0) starts, in long units.
    pub x0: f64,
    pub y0: f64,
    /// A cell's side, long units.
    pub cell: f64,
    pub mask: Mask,
}

impl Cover {
    /// Grid point `(x, y)` (cell units, a cell's middle is `.5`) in long units.
    pub fn at(&self, x: f64, y: f64) -> Point {
        Point::new(self.x0 + x * self.cell, self.y0 + y * self.cell)
    }
}

/// The stroke's usable points: numbers only, clamped, thinned to [`MAX_POINTS`].
fn tidy(points: &[Point]) -> Vec<Point> {
    let ok: Vec<Point> =
        points.iter().filter(|p| p.x.is_finite() && p.y.is_finite()).map(|p| Point::new(p.x.clamp(-FAR, FAR), p.y.clamp(-FAR, FAR))).collect();
    let n = ok.len();
    if n <= MAX_POINTS {
        return ok;
    }
    (0..MAX_POINTS).filter_map(|k| ok.get(k * (n - 1) / (MAX_POINTS - 1)).copied()).collect()
}

fn radius_ok(r: f64) -> f64 {
    if r.is_finite() { r.clamp(1e-4, 0.5) } else { 0.02 }
}

/// Whether the stroke `points` (long units) with brush radius `r` is a loop that encloses something: long enough, its
/// end within [`CLOSE_RADII`] radii of its start, and the polygon it makes holds more than the stroke's own width.
pub fn is_loop(points: &[Point], r: f64) -> bool {
    let (pts, r) = (tidy(points), radius_ok(r));
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else { return false };
    if pts.len() < 8 || (first.x - last.x).hypot(first.y - last.y) > CLOSE_RADII * r {
        return false;
    }
    let length: f64 = pts.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
    length >= MIN_LOOP_LENGTH * r && shoelace(&pts).abs() >= MIN_LOOP_AREA * r * r
}

/// The signed area of the polygon through `pts` (closed).
fn shoelace(pts: &[Point]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| (pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y) / 2.0).sum()
}

/// The union of discs of radius `r` along `points` (long units), with the inside of the loop when `fill`, on a grid of
/// at most `max_side` cells a side. `None` for a stroke with no usable point or a grid that would be empty.
pub fn cover(points: &[Point], r: f64, fill: bool, max_side: usize) -> Option<Cover> {
    let (pts, r) = (tidy(points), radius_ok(r));
    if pts.is_empty() || max_side < 4 {
        return None;
    }
    let (mut lo, mut hi) = (Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN));
    for p in &pts {
        (lo.x, lo.y, hi.x, hi.y) = (lo.x.min(p.x), lo.y.min(p.y), hi.x.max(p.x), hi.y.max(p.y));
    }
    let (x0, y0) = (lo.x - r - 1e-9, lo.y - r - 1e-9);
    let extent = (hi.x - lo.x).max(hi.y - lo.y) + 2.0 * r + 2e-9;
    let cell = (extent / max_side as f64).max(r / CELLS_PER_RADIUS);
    let (w, h) = (((hi.x + r + 1e-9 - x0) / cell).ceil() as usize + 1, ((hi.y + r + 1e-9 - y0) / cell).ceil() as usize + 1);
    if w == 0 || h == 0 || w > max_side + 2 || h > max_side + 2 {
        return None;
    }
    let cell_of = |p: Point| ((p.x - x0) / cell, (p.y - y0) / cell);
    // the stroke itself, as cells
    let mut on = vec![false; w * h];
    let mut mark = |(x, y): (f64, f64)| {
        let (ix, iy) = (x.floor(), y.floor());
        if ix >= 0.0
            && iy >= 0.0
            && (ix as usize) < w
            && (iy as usize) < h
            && let Some(c) = on.get_mut(iy as usize * w + ix as usize)
        {
            *c = true;
        }
    };
    for (i, p) in pts.iter().enumerate() {
        let a = cell_of(*p);
        mark(a);
        if let Some(q) = pts.get(i + 1) {
            let b = cell_of(*q);
            let steps = ((b.0 - a.0).hypot(b.1 - a.1) / 0.7).ceil().min(4.0 * max_side as f64) as usize;
            for k in 1..steps {
                let t = k as f64 / steps as f64;
                mark((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
        }
    }
    let r_cells2 = ((r / cell) * (r / cell)) as f32;
    let dist = distance_sq(w, h, |i| on.get(i).copied().unwrap_or(false));
    if dist.len() != w * h {
        return None;
    }
    let mut bits: Vec<bool> = dist.iter().map(|d| *d <= r_cells2).collect();
    if fill && pts.len() >= 3 {
        fill_polygon(&pts.iter().map(|p| cell_of(*p)).collect::<Vec<_>>(), w, h, &mut bits);
    }
    Some(Cover { x0, y0, cell, mask: Mask { w, h, bits } })
}

/// Set the cells whose middle is inside the closed polygon `poly` (cell units; non-zero winding, so a loop drawn
/// twice or with a twist still fills).
fn fill_polygon(poly: &[(f64, f64)], w: usize, h: usize, bits: &mut [bool]) {
    let mut rows: Vec<Vec<(f64, i32)>> = vec![Vec::new(); h];
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if a.1 == b.1 {
            continue;
        }
        let (lo, hi) = (a.1.min(b.1), a.1.max(b.1));
        let j0 = (lo - 0.5).ceil().max(0.0) as usize;
        let j1 = ((hi - 0.5).ceil().max(0.0) as usize).min(h);
        for j in j0..j1 {
            let yc = j as f64 + 0.5;
            if (a.1 <= yc) != (b.1 <= yc) {
                let x = a.0 + (yc - a.1) / (b.1 - a.1) * (b.0 - a.0);
                if let Some(row) = rows.get_mut(j) {
                    row.push((x, if b.1 > a.1 { 1 } else { -1 }));
                }
            }
        }
    }
    for (j, row) in rows.iter_mut().enumerate() {
        row.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut wind = 0;
        for k in 0..row.len() {
            wind += row[k].1;
            let Some(next) = row.get(k + 1) else { break };
            if wind == 0 {
                continue;
            }
            let (xa, xb) = (row[k].0, next.0);
            let (i0, i1) = ((xa - 0.5).ceil().max(0.0) as usize, ((xb - 0.5).ceil().max(0.0) as usize).min(w));
            for i in i0..i1 {
                if let Some(c) = bits.get_mut(j * w + i) {
                    *c = true;
                }
            }
        }
    }
}

/// The outline of what `points` (long units) with brush radius `r` cover: one ring per separate piece (at most
/// `pieces`), in long units. With `fill`, a loop's inside is covered too.
pub fn outline(points: &[Point], r: f64, fill: bool, pieces: usize) -> Vec<Vec<Point>> {
    let Some(c) = cover(points, r, fill, OUTLINE_SIDE) else { return Vec::new() };
    rings(&c, pieces)
}

/// The outer outlines of a [`Cover`], in long units.
pub fn rings(c: &Cover, pieces: usize) -> Vec<Vec<Point>> {
    components(&c.mask, pieces).iter().map(|k| simplify(&trace_outline(&k.mask), 0.6, true).iter().map(|p| c.at(p.0, p.1)).collect()).collect()
}

/// The stored shape of a lasso: the inside of the loop `points` (the picture's normalized coordinates) with the discs
/// of radius `reach` (long units) along it, on a picture `ow × oh`. `None` when nothing is covered.
pub fn lasso_area(points: &[Point], ow: f64, oh: f64, reach: f64) -> Option<SegMask> {
    if !(ow.is_finite() && oh.is_finite() && ow > 0.0 && oh > 0.0) {
        return None;
    }
    let long = ow.max(oh);
    let (sx, sy) = (ow / long, oh / long);
    let in_long: Vec<Point> = points.iter().map(|p| Point::new(p.x * sx, p.y * sy)).collect();
    let c = cover(&in_long, reach, true, OUTLINE_SIDE)?;
    let cov: Vec<f32> = c.mask.bits.iter().map(|b| if *b { 1.0 } else { 0.0 }).collect();
    crate::spot_area::area_from_coverage(
        &cov,
        c.mask.w,
        c.mask.h,
        |p| {
            let l = c.at(p.x, p.y);
            Point::new(l.x / sx, l.y / sy)
        },
        |n| Point::new((n.x * sx - c.x0) / c.cell, (n.y * sy - c.y0) / c.cell),
    )
}

#[cfg(test)]
#[path = "tests_spot_lasso.rs"]
mod tests;
