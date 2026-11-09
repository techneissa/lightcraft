//! Fork feature (`fork/brush-area.md`): the area a brushed Remove, Heal or Clone spot covers, as one shape.
//!
//! A brushed spot covers the union of discs of its radius along its stroke. This module measures that union on a
//! grid ([`shape`]) so the Remove tool can draw it as ONE outline (and, while painting, one filled shape), and turns a
//! stroke that comes back to where it started into a lasso: the spot then covers the inside of the loop too
//! ([`lasso_area`], stored as a shaped spot, `fork/object-remove.md`).
//!
//! Everything works in "long units" (the long edge of the picture is 1, so a radius is a disc), with points taken
//! from the uncropped picture's normalized coordinates and the picture's size. Strokes are hostile input (a pointer
//! path, a command, an agent): points that are not numbers are dropped, far-off ones are clamped, the path is thinned
//! to [`MAX_POINTS`] and the grid never exceeds the side asked for.
//!
//! **When a stroke is a lasso.** It must end within [`CLOSE_RADII`] radii of its start, be at least
//! [`MIN_LOOP_LENGTH`] radii long, and the loop must hold, beyond what the ribbon itself covers, at least
//! [`MIN_HOLE_DISCS`] brush discs of area. The last test is measured on the grid, not from a signed area, so a
//! figure 8 (whose signed area cancels) counts, each lobe filled, while a stroke out and back, a sliver, or a small
//! circle around a blemish (a hole of less than three discs) stays a stroke. The inside is filled by the non-zero
//! rule.

use lightcraft_develop::SegMask;
use lightcraft_geom::Point;

use crate::find::shape::{Mask, components, distance_sq, simplify, trace_outline};

/// Most stroke points measured; a longer stroke is thinned evenly (first and last kept).
pub const MAX_POINTS: usize = 2048;
/// A stroke is a loop when its end is within this many radii of its start.
pub const CLOSE_RADII: f64 = 2.0;
/// The shortest stroke that can be a loop, in radii of path.
pub const MIN_LOOP_LENGTH: f64 = 6.0;
/// A loop is a lasso when the inside the ribbon does not itself cover is at least this many brush discs (π r²).
pub const MIN_HOLE_DISCS: f64 = 3.0;
/// Largest share of the photo a lasso may cover (as Remove This Area).
pub const MAX_SHARE: f64 = 0.6;
/// Grid side the outline of a spot is measured on.
pub const OUTLINE_SIDE: usize = 640;
/// Cells across a stored lasso's loop. The stored mask adds a margin of a tenth on each side where the picture allows
/// it, so it is at most 1,008 cells across (the most a stored shape may have is 1,024).
pub const LASSO_SIDE: usize = 840;
/// Grid side the loop test measures on.
const TEST_SIDE: usize = 160;
/// Grid cells to a radius at least, so a small spot is still a smooth disc.
const CELLS_PER_RADIUS: f64 = 10.0;
/// Coordinates (long units) the stroke is clamped to.
const FAR: f64 = 4.0;

/// A grid of cells over part of the picture.
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

/// What a stroke covers: the union of discs along it, with the inside of the loop when it is a lasso.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub cover: Cover,
    pub lasso: bool,
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

/// The grid over the box of `pts` grown by `pad`, cut to `clip` (`[x0, y0, x1, y1]`, long units), with cells of at
/// least `floor` and at most `max_side` of them across.
fn grid(pts: &[Point], pad: f64, max_side: usize, floor: f64, clip: Option<[f64; 4]>) -> Option<Cover> {
    if pts.is_empty() || max_side < 4 {
        return None;
    }
    let (mut ax, mut ay, mut bx, mut by) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        (ax, ay, bx, by) = (ax.min(p.x), ay.min(p.y), bx.max(p.x), by.max(p.y));
    }
    let pad = pad + 1e-9;
    (ax, ay, bx, by) = (ax - pad, ay - pad, bx + pad, by + pad);
    if let Some(c) = clip {
        (ax, ay, bx, by) = (ax.max(c[0]), ay.max(c[1]), bx.min(c[2]), by.min(c[3]));
    }
    if !(bx > ax && by > ay) {
        return None;
    }
    let cell = ((bx - ax).max(by - ay) / max_side as f64).max(floor).max(1e-7);
    let (w, h) = (((bx - ax) / cell).ceil() as usize + 1, ((by - ay) / cell).ceil() as usize + 1);
    if w > max_side + 2 || h > max_side + 2 {
        return None;
    }
    Some(Cover { x0: ax, y0: ay, cell, mask: Mask::new(w, h) })
}

/// The part of the segment `a`–`b` inside the box `[-1, w + 1] × [-1, h + 1]` (cell units), if any (Liang–Barsky), so
/// a stroke point far outside the grid cannot stretch the sampling of what crosses it.
fn clip_segment(a: (f64, f64), b: (f64, f64), w: usize, h: usize) -> Option<((f64, f64), (f64, f64))> {
    let (d, lo, hi) = ((b.0 - a.0, b.1 - a.1), -1.0, (w as f64 + 1.0, h as f64 + 1.0));
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d.0, a.0 - lo), (d.0, hi.0 - a.0), (-d.1, a.1 - lo), (d.1, hi.1 - a.1)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some(((a.0 + d.0 * t0, a.1 + d.1 * t0), (a.0 + d.0 * t1, a.1 + d.1 * t1)))
}

/// The cells within `r` (long units) of the polyline `pts`.
fn ribbon(c: &Cover, pts: &[Point], r: f64) -> Vec<bool> {
    let (w, h) = (c.mask.w, c.mask.h);
    let cell_of = |p: Point| ((p.x - c.x0) / c.cell, (p.y - c.y0) / c.cell);
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
        if let Some(q) = pts.get(i + 1)
            && let Some((a, b)) = clip_segment(a, cell_of(*q), w, h)
        {
            let steps = ((b.0 - a.0).hypot(b.1 - a.1) / 0.7).ceil().min(8.0 * (w + h) as f64) as usize;
            for k in 1..steps {
                let t = k as f64 / steps as f64;
                mark((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
        }
    }
    let r2 = ((r / c.cell) * (r / c.cell)) as f32;
    let dist = distance_sq(w, h, |i| on.get(i).copied().unwrap_or(false));
    if dist.len() != w * h {
        return vec![false; w * h];
    }
    dist.iter().map(|d| *d <= r2).collect()
}

/// The cells whose middle is inside the closed polygon `poly` (cell units; non-zero winding, so a loop drawn twice or
/// with a twist still fills).
fn fill_polygon(poly: &[(f64, f64)], w: usize, h: usize) -> Vec<bool> {
    let mut bits = vec![false; w * h];
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
    bits
}

/// Clear the cells of `bits` (on the grid of `c`) whose middle is outside `clip`.
fn clip_bits(c: &Cover, bits: &mut [bool], clip: Option<[f64; 4]>) {
    let Some(k) = clip else { return };
    let w = c.mask.w;
    for (i, b) in bits.iter_mut().enumerate() {
        let p = c.at((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
        if p.x < k[0] || p.x > k[2] || p.y < k[1] || p.y > k[3] {
            *b = false;
        }
    }
}

/// Whether the (tidied) stroke closes on itself: enough points, long enough, its end near its start.
fn closes(pts: &[Point], r: f64) -> bool {
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else { return false };
    if pts.len() < 8 || (first.x - last.x).hypot(first.y - last.y) > CLOSE_RADII * r {
        return false;
    }
    let length: f64 = pts.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum();
    length >= MIN_LOOP_LENGTH * r
}

/// The cells inside the loop through `pts` on the grid `c` (closed by a straight line from its end), cut to `clip`.
fn polygon_bits(c: &Cover, pts: &[Point], clip: Option<[f64; 4]>) -> Vec<bool> {
    let poly: Vec<(f64, f64)> = pts.iter().map(|p| ((p.x - c.x0) / c.cell, (p.y - c.y0) / c.cell)).collect();
    let mut bits = fill_polygon(&poly, c.mask.w, c.mask.h);
    clip_bits(c, &mut bits, clip);
    bits
}

/// What `points` (long units) with brush radius `r` cover, on a grid of at most `max_side` cells a side, cut to `clip`
/// (`[x0, y0, x1, y1]`: the picture). With `lasso_ok`, a stroke that is a lasso (see the module) also covers its
/// inside. `None` for a stroke with no usable point or none on the picture.
pub fn shape(points: &[Point], r: f64, max_side: usize, clip: Option<[f64; 4]>, lasso_ok: bool) -> Option<Shape> {
    let (pts, r) = (tidy(points), radius_ok(r));
    let mut cover = grid(&pts, r, max_side, r / CELLS_PER_RADIUS, clip)?;
    let mut bits = ribbon(&cover, &pts, r);
    clip_bits(&cover, &mut bits, clip);
    let mut lasso = false;
    if lasso_ok && closes(&pts, r) {
        let inside = polygon_bits(&cover, &pts, clip);
        let hole = inside.iter().zip(&bits).filter(|(i, rb)| **i && !**rb).count() as f64;
        if hole * cover.cell * cover.cell >= MIN_HOLE_DISCS * std::f64::consts::PI * r * r {
            lasso = true;
            for (b, i) in bits.iter_mut().zip(&inside) {
                *b |= *i;
            }
        }
    }
    cover.mask.bits = bits;
    Some(Shape { cover, lasso })
}

/// Whether the stroke `points` (long units) with brush radius `r` is a lasso on the picture `clip`.
pub fn is_loop(points: &[Point], r: f64, clip: Option<[f64; 4]>) -> bool {
    shape(points, r, TEST_SIDE, clip, true).is_some_and(|s| s.lasso)
}

/// The outline of what a stroke covers (no lasso): one ring per separate piece (at most `pieces`), in long units.
pub fn outline(points: &[Point], r: f64, pieces: usize) -> Vec<Vec<Point>> {
    let Some(s) = shape(points, r, OUTLINE_SIDE, None, false) else { return Vec::new() };
    rings(&s.cover, pieces)
}

/// The outer outlines of a [`Cover`], in long units.
pub fn rings(c: &Cover, pieces: usize) -> Vec<Vec<Point>> {
    components(&c.mask, pieces).iter().map(|k| simplify(&trace_outline(&k.mask), 0.6, true).iter().map(|p| c.at(p.0, p.1)).collect()).collect()
}

/// What a stroke makes on a picture.
#[derive(Clone, Debug, PartialEq)]
pub enum Lasso {
    /// Not a lasso: the stroke stays a stroke.
    No,
    /// A lasso that would cover more than [`MAX_SHARE`] of the picture.
    TooBig,
    /// The stored shape of the loop's inside (the discs along the stroke are not in it: the spot reaches one brush
    /// radius past it, as a shaped spot does).
    Area(SegMask),
}

/// The stored shape of the lasso that the stroke `points` (the picture's normalized coordinates) with brush radius
/// `r` (long edge) makes on a picture `ow × oh` pixels. The loop is clipped to the picture, and measured on
/// [`LASSO_SIDE`] cells across (so a full-frame export is not soft), whatever the loop's size.
pub fn lasso_area(points: &[Point], ow: f64, oh: f64, r: f64) -> Lasso {
    if !(ow.is_finite() && oh.is_finite() && ow > 0.0 && oh > 0.0) {
        return Lasso::No;
    }
    let long = ow.max(oh);
    let (sx, sy) = (ow / long, oh / long);
    let clip = Some([0.0, 0.0, sx, sy]);
    let in_long: Vec<Point> = points.iter().map(|p| Point::new(p.x * sx, p.y * sy)).collect();
    let r = radius_ok(r);
    if !is_loop(&in_long, r, clip) {
        return Lasso::No;
    }
    let pts = tidy(&in_long);
    let Some(mut c) = grid(&pts, 0.0, LASSO_SIDE, 0.0, clip) else { return Lasso::No };
    c.mask.bits = polygon_bits(&c, &pts, clip);
    let share = c.mask.count() as f64 * c.cell * c.cell / (sx * sy);
    if share > MAX_SHARE {
        return Lasso::TooBig;
    }
    let cov: Vec<f32> = c.mask.bits.iter().map(|b| if *b { 1.0 } else { 0.0 }).collect();
    let seg = crate::spot_area::area_from_coverage(
        &cov,
        c.mask.w,
        c.mask.h,
        |p| {
            let l = c.at(p.x, p.y);
            Point::new(l.x / sx, l.y / sy)
        },
        |n| Point::new((n.x * sx - c.x0) / c.cell, (n.y * sy - c.y0) / c.cell),
    );
    // (the grid reaches at most half a cell past the picture's edge: the stored box stays on the picture)
    let Some(mut seg) = seg else { return Lasso::No };
    if let Some(r) = seg.rect {
        let r = r.map(|v| v.clamp(0.0, 1.0));
        if !(r[2] > r[0] && r[3] > r[1]) {
            return Lasso::No;
        }
        seg.rect = Some(r);
    }
    Lasso::Area(seg)
}

#[cfg(test)]
#[path = "tests_spot_lasso.rs"]
mod tests;
