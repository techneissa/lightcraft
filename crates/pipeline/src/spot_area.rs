//! Fork feature (`fork/object-remove.md`): Remove spots in the shape of an object or a mask
//! ([`lightcraft_develop::SpotArea`]), drawn by the same per-spot loop as stroke spots ([`crate::spots::apply`]).
//!
//! A shaped spot covers its stored outline grown by its radius (the Expand amount, `Spot::size`), with the spot's
//! feather and opacity: a pixel's alpha is `(1 − smoothstep(inner, r, d)) · opacity`, `d` its distance in output pixels
//! to the inside of the outline, as a stroke spot's is the distance to its stroke. Distances are in output pixels and
//! the radius scales with the render, so a preview and an export cover the same part of the photo.
//!
//! A shaped spot has no source: it is rebuilt from its surroundings, by the content-aware fill over its box and half
//! its size around it (Classic), or by LaMa ([`crate::spots_learned`]). The GPU renderer leaves photos with one to
//! the CPU, as it does Content-Aware spots.

use std::borrow::Cow;

use lightcraft_develop::{SegMask, Spot};
use lightcraft_geom::Point;

use crate::geometry::Frame;
use crate::masks::SegGrid;
use crate::spots::smooth;

/// How much of its size around a shaped spot the Classic fill reads, on each side.
const CONTEXT: f64 = 0.5;
/// Most pixels the distance to an outline is measured on; a bigger box measures on a coarser grid (a huge export).
const MAX_DISTANCE_PIXELS: usize = 8 << 20;

/// The corners of a shaped spot's shape (its grid's rectangle) in spot coordinates; `None` for a stroke spot and
/// for a shape whose rectangle is damaged.
pub fn corners(spot: &Spot) -> Option<[Point; 4]> {
    let r = spot.area.as_ref()?.bounds()?;
    Some([Point::new(r[0], r[1]), Point::new(r[2], r[1]), Point::new(r[2], r[3]), Point::new(r[0], r[3])])
}

/// The points whose box, with the spot's radius around it, holds everything the spot draws: its shape's corners for
/// a shaped spot, its stroke otherwise.
pub fn box_points(spot: &Spot) -> Cow<'_, [Point]> {
    match corners(spot) {
        Some(c) => Cow::Owned(c.to_vec()),
        None => Cow::Borrowed(&spot.points),
    }
}

/// Pixels the Classic fill reads around a shaped spot's box (its points' box spans `span` pixels, the radius
/// included); zero for a stroke spot.
pub fn context_pad(spot: &Spot, span: f64) -> f64 {
    if spot.area.is_none() || !span.is_finite() {
        return 0.0;
    }
    CONTEXT * span.max(0.0)
}

/// Pixels more a render window must hold around a shaped spot than around a stroke spot of radius `r` whose points
/// span `span` pixels (what [`crate::spots::layout`] adds for it); zero for a stroke spot.
pub fn window_pad(spot: &Spot, r: f64, span: f64) -> f64 {
    if spot.area.is_none() {
        return 0.0;
    }
    r + context_pad(spot, span + 2.0 * r)
}

/// A pixel box: left, top, width, height.
pub type PixelBox = (usize, usize, usize, usize);

/// A shaped spot's alpha over the box `b` of a `w × h` render of `frame` (radius `r`, full strength within `inner`,
/// times `opacity`), row by row; `None` for a stroke spot. A shape whose data is damaged selects nothing.
#[allow(clippy::too_many_arguments)]
pub fn alpha(spot: &Spot, frame: &Frame, w: usize, h: usize, b: PixelBox, r: f32, inner: f32, opacity: f32) -> Option<Vec<f32>> {
    let area = spot.area.as_ref()?;
    let (bx, by, bw, bh) = b;
    let n = bw.saturating_mul(bh);
    let none = vec![0.0f32; n];
    let Some(grid) = SegGrid::new(&area.seg) else { return Some(none) };
    if n == 0 || w == 0 || h == 0 {
        return Some(none);
    }
    // the inside, on a grid of `step` pixels (one pixel unless the box is huge)
    let step = ((n as f64 / MAX_DISTANCE_PIXELS as f64).sqrt().ceil() as usize).max(1);
    let (cw, ch) = (bw.div_ceil(step), bh.div_ceil(step));
    let to_norm = frame.out_to_norm(w, h);
    let half = step as f64 / 2.0;
    let inside: Vec<bool> = (0..cw * ch)
        .map(|i| {
            let (x, y) = ((bx + (i % cw) * step) as f64 + half, (by + (i / cw) * step) as f64 + half);
            let q = to_norm.apply(Point::new(x, y));
            grid.contains(q) && grid.logit(q) > 0.0
        })
        .collect();
    if !inside.iter().any(|v| *v) {
        return Some(none);
    }
    let dist = crate::find::shape::distance_sq(cw, ch, |i| inside.get(i).copied().unwrap_or(false));
    if dist.len() != cw * ch {
        return Some(none);
    }
    let mut out = none;
    for y in 0..bh {
        for x in 0..bw {
            let d = dist.get((y / step) * cw + x / step).map_or(f32::MAX, |d| d.sqrt() * step as f32);
            if let Some(a) = out.get_mut(y * bw + x) {
                *a = (1.0 - smooth(inner, r, d)) * opacity;
            }
        }
    }
    Some(out)
}

/// A stored shape from `cov`, the coverage (0..1, above a half inside) of a `w × h` picture whose pixel coordinates
/// go to spot coordinates through `to_norm` and back through `to_px`. The grid covers the inside's box and a margin
/// around it, at about the picture's resolution. `None` when nothing is inside.
pub fn area_from_coverage(cov: &[f32], w: usize, h: usize, to_norm: impl Fn(Point) -> Point, to_px: impl Fn(Point) -> Point) -> Option<SegMask> {
    if w == 0 || h == 0 || cov.len() != w.checked_mul(h)? {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, c) in cov.iter().enumerate() {
        if *c > 0.5 {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x0 > x1 || y0 > y1 {
        return None;
    }
    let span = (x1 + 1 - x0).max(y1 + 1 - y0) as f64;
    let m = (0.1 * span).max(3.0);
    let (px0, py0) = ((x0 as f64 - m).max(0.0), (y0 as f64 - m).max(0.0));
    let (px1, py1) = (((x1 + 1) as f64 + m).min(w as f64), ((y1 + 1) as f64 + m).min(h as f64));
    let c = [Point::new(px0, py0), Point::new(px1, py0), Point::new(px1, py1), Point::new(px0, py1)].map(&to_norm);
    let rect = [
        c.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
        c.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
        c.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
        c.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
    ];
    if !(rect.iter().all(|v| v.is_finite()) && rect[2] > rect[0] && rect[3] > rect[1]) {
        return None;
    }
    let side = ((px1 - px0).max(py1 - py0).ceil() as usize).clamp(8, lightcraft_develop::segmask::MAX_SIDE);
    let at = |x: i64, y: i64| -> f32 {
        let (x, y) = (x.clamp(0, w as i64 - 1) as usize, y.clamp(0, h as i64 - 1) as usize);
        cov.get(y * w + x).copied().filter(|v| v.is_finite()).unwrap_or(0.0)
    };
    let mut logits = Vec::with_capacity(side * side);
    for j in 0..side {
        for i in 0..side {
            let n = Point::new(
                rect[0] + (i as f64 + 0.5) / side as f64 * (rect[2] - rect[0]),
                rect[1] + (j as f64 + 0.5) / side as f64 * (rect[3] - rect[1]),
            );
            let p = to_px(n);
            let (fx, fy) = (p.x - 0.5, p.y - 0.5);
            let v = if fx.is_finite() && fy.is_finite() {
                let (ix, iy) = (fx.floor() as i64, fy.floor() as i64);
                let (tx, ty) = ((fx - fx.floor()) as f32, (fy - fy.floor()) as f32);
                let top = at(ix, iy) * (1.0 - tx) + at(ix + 1, iy) * tx;
                let bot = at(ix, iy + 1) * (1.0 - tx) + at(ix + 1, iy + 1) * tx;
                top * (1.0 - ty) + bot * ty
            } else {
                0.0
            };
            let v = v.clamp(1e-4, 1.0 - 1e-4);
            logits.push((v / (1.0 - v)).ln());
        }
    }
    Some(SegMask::from_logits_in(side, &logits, rect))
}

#[cfg(test)]
#[path = "tests_spot_area.rs"]
mod tests;
