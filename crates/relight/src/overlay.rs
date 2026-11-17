//! Pictures of the light for the ⌥-drag preview (issue #294, `fork/relight.md`): where the light reaches and how it
//! hits each surface, as geometry in normalized photo coordinates that the UI draws over the loupe.
//!
//! - [`marks`]: a grid of spots on the photo, each with the light's gain there (in stops: absolute, so they follow
//!   Amount) and the two axes of the surface it lies on, from the field's normal, as the photo shows them
//!   (foreshortened). The UI draws a cross where the gain is above 0 and a ring where Darken Surroundings darkens.
//! - [`contours`]: lines of equal gain at fixed stop levels ([`UP`], [`DOWN`]), and a line where the gain crosses 0
//!   into darkening, traced (marching squares) on the shading downsampled to at most [`GRID_EDGE`] pixels and lightly
//!   smoothed.
//!
//! Both read the [`Shading`] the render already made (once per light, at field resolution): nothing here shades.
//! Both take hostile numbers as nothing: a plane of NaN, an empty or one-pixel shading give no marks and no lines.

use std::collections::HashMap;

use crate::field::Field;
use crate::light::Shading;

/// The stop levels contour lines are drawn at where the light brightens.
pub const UP: &[f32] = &[0.1, 0.2, 0.35, 0.5, 0.7, 0.9, 1.2, 1.5, 1.8];
/// The stop levels where it darkens (the line at 0 where darkening starts is added when something darkens).
pub const DOWN: &[f32] = &[-0.25, -0.5, -0.75, -1.0, -1.5];
/// The longest side of the plane contours are traced on (pixels).
pub const GRID_EDGE: usize = 320;
/// How many 3 × 3 box blurs the plane gets before it is traced (the render's gain follows the photo's own texture at
/// depth edges; lines that follow every pixel of it are noise).
const BLURS: usize = 4;
/// Gains smaller than this (stops, either way) get no mark.
pub const MIN_STOPS: f32 = 0.02;
/// The most marks a long side holds.
pub const MAX_GRID: usize = 512;
/// The shortest an axis of a mark's surface may look (the surface edge-on still shows a line, never a point).
const MIN_AXIS: f32 = 0.2;

/// One spot of the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// Where, in normalized photo coordinates.
    pub u: f32,
    pub v: f32,
    /// The light's gain there (stops; negative where it darkens).
    pub stops: f32,
    /// The two axes of the surface under the spot, as `(x, y)` in the photo's plane (x right, y down): unit tangents
    /// of the surface, projected and foreshortened (each at least [`MIN_AXIS`] long). A camera-facing wall gives
    /// `[1, 0]` and `[0, 1]` (up to sign); a floor receding from the camera squashes the vertical one.
    pub axes: [[f32; 2]; 2],
}

/// The grid of spots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    /// The grid's columns and rows (spots at `((i + 0.5) / cols, (j + 0.5) / rows)`).
    pub cols: usize,
    pub rows: usize,
    pub marks: Vec<Mark>,
}

/// A line of equal gain, in normalized photo coordinates. A closed line ends where it began.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The gain along it (stops).
    pub level: f32,
    pub closed: bool,
    pub points: Vec<[f32; 2]>,
}

/// What [`contours`] makes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contours {
    pub lines: Vec<Line>,
    /// Whether some of the photo darkens (the line at level 0 is where darkening starts).
    pub darkens: bool,
}

/// The two in-surface axes of the surface with normal `n` as they look in the photo (see [`Mark::axes`]).
fn surface_axes(n: [f32; 3]) -> [[f32; 2]; 2] {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let n = if len.is_finite() && len > 1e-6 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 0.0, -1.0] };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // the tangent nearest the photo's x axis (the photo's y axis when the surface is edge-on to it)
    let mut t1 = [1.0 - n[0] * n[0], -n[0] * n[1], -n[0] * n[2]];
    if dot(t1, t1) < 1e-4 {
        t1 = [-n[1] * n[0], 1.0 - n[1] * n[1], -n[1] * n[2]];
    }
    let l1 = dot(t1, t1).sqrt().max(1e-9);
    t1 = [t1[0] / l1, t1[1] / l1, t1[2] / l1];
    let t2 = [n[1] * t1[2] - n[2] * t1[1], n[2] * t1[0] - n[0] * t1[2], n[0] * t1[1] - n[1] * t1[0]];
    let flat = |t: [f32; 3], fallback: [f32; 2]| {
        let (x, y) = (t[0], t[1]);
        let l = (x * x + y * y).sqrt();
        if !(l.is_finite() && l > 1e-4) {
            return fallback;
        }
        let k = l.max(MIN_AXIS) / l;
        [x * k, y * k]
    };
    [flat(t1, [MIN_AXIS, 0.0]), flat(t2, [0.0, MIN_AXIS])]
}

/// The grid of spots with `n_long` along the photo's long side (clamped to 4..=[`MAX_GRID`]) on `field`, with the
/// gain of `shading` at each. Spots the light leaves alone (under [`MIN_STOPS`]) are left out.
pub fn marks(field: &Field, shading: &Shading, n_long: usize) -> Marks {
    let (fw, fh) = (field.width, field.height);
    if fw == 0 || fh == 0 || shading.width == 0 || shading.height == 0 {
        return Marks::default();
    }
    let n = n_long.clamp(4, MAX_GRID);
    let long = fw.max(fh);
    let scale = n as f64 / long as f64;
    let (cols, rows) = (((fw as f64 * scale).round() as usize).max(1), ((fh as f64 * scale).round() as usize).max(1));
    let mut marks = Vec::new();
    for j in 0..rows {
        let v = (j as f32 + 0.5) / rows as f32;
        for i in 0..cols {
            let u = (i as f32 + 0.5) / cols as f32;
            let stops = shading.at(u, v).stops;
            if !stops.is_finite() || stops.abs() < MIN_STOPS {
                continue;
            }
            marks.push(Mark { u, v, stops, axes: surface_axes(field.normal_at(u, v)) });
        }
    }
    Marks { cols, rows, marks }
}

/// A line traced on a plane, in the plane's pixel coordinates (a pixel's centre at its integer position).
struct Trace {
    closed: bool,
    points: Vec<[f32; 2]>,
}

/// The lines where the `w × h` plane `g` crosses `level`, by marching squares: each crossing is placed along the
/// edge between two pixels by linear interpolation, and the pieces are joined into lines. `g` must be finite.
fn trace(g: &[f32], w: usize, h: usize, level: f32) -> Vec<Trace> {
    if w < 2 || h < 2 || g.len() != w * h {
        return Vec::new();
    }
    let at = |x: usize, y: usize| g.get(y * w + x).copied().unwrap_or(0.0);
    // an edge is named by its pixel and its direction: right (0) or down (1)
    let key = |x: usize, y: usize, down: usize| (y * w + x) * 2 + down;
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x + 1, y + 1), at(x, y + 1));
            let case = usize::from(a >= level) | usize::from(b >= level) << 1 | usize::from(c >= level) << 2 | usize::from(d >= level) << 3;
            if case == 0 || case == 15 {
                continue;
            }
            let (top, right, bottom, left) = (key(x, y, 0), key(x + 1, y, 1), key(x, y + 1, 0), key(x, y, 1));
            let centre_in = (a + b + c + d) * 0.25 >= level;
            let pairs: &[(usize, usize)] = match case {
                1 | 14 => &[(left, top)],
                2 | 13 => &[(top, right)],
                3 | 12 => &[(left, right)],
                4 | 11 => &[(right, bottom)],
                6 | 9 => &[(top, bottom)],
                7 | 8 => &[(left, bottom)],
                5 if centre_in => &[(top, right), (left, bottom)],
                5 => &[(left, top), (right, bottom)],
                _ if centre_in => &[(left, top), (right, bottom)],
                _ => &[(top, right), (left, bottom)],
            };
            segs.extend_from_slice(pairs);
        }
    }
    // where an edge's crossing lies
    let point = |k: usize| -> [f32; 2] {
        let (down, p) = (k % 2, k / 2);
        let (x, y) = (p % w, p / w);
        let (a, b) = if down == 0 { (at(x, y), at((x + 1).min(w - 1), y)) } else { (at(x, y), at(x, (y + 1).min(h - 1))) };
        let t = if (b - a).abs() > 1e-9 { ((level - a) / (b - a)).clamp(0.0, 1.0) } else { 0.5 };
        if down == 0 { [x as f32 + t, y as f32] } else { [x as f32, y as f32 + t] }
    };
    let mut at_edge: HashMap<usize, Vec<usize>> = HashMap::with_capacity(segs.len() * 2);
    for (i, (a, b)) in segs.iter().enumerate() {
        at_edge.entry(*a).or_default().push(i);
        at_edge.entry(*b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    // the keys of a chain walked on from `from`, taking each unused segment that touches its end
    let walk = |used: &mut Vec<bool>, mut end: usize| -> Vec<usize> {
        let mut keys = Vec::new();
        while let Some(next) = at_edge.get(&end).and_then(|v| v.iter().copied().find(|i| used.get(*i) == Some(&false))) {
            if let Some(u) = used.get_mut(next) {
                *u = true;
            }
            let Some(&(a, b)) = segs.get(next) else { break };
            end = if a == end { b } else { a };
            keys.push(end);
        }
        keys
    };
    let mut out = Vec::new();
    for i in 0..segs.len() {
        if used.get(i) != Some(&false) {
            continue;
        }
        let Some(&(a, b)) = segs.get(i) else { continue };
        if let Some(u) = used.get_mut(i) {
            *u = true;
        }
        let mut keys: Vec<usize> = vec![a, b];
        keys.extend(walk(&mut used, b));
        let closed = keys.len() > 3 && keys.last() == keys.first();
        if !closed {
            let mut back = walk(&mut used, a);
            back.reverse();
            back.extend(keys);
            keys = back;
        }
        out.push(Trace { closed, points: keys.into_iter().map(point).collect() });
    }
    out
}

/// One round of corner cutting: each piece of the line is replaced by its points a quarter and three quarters along
/// it (an open line keeps its ends), which rounds the pixel grid's corners away.
fn smooth(points: &[[f32; 2]], closed: bool) -> Vec<[f32; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let lerp = |p: [f32; 2], q: [f32; 2], t: f32| [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t];
    let mut out = Vec::with_capacity(points.len() * 2 + 2);
    // (a closed line's last point repeats its first: the pieces run between neighbouring points)
    for pair in points.windows(2) {
        let (p, q) = (pair[0], pair[1]);
        out.push(lerp(p, q, 0.25));
        out.push(lerp(p, q, 0.75));
    }
    match (closed, points.first(), points.last()) {
        (true, ..) => {
            if let Some(f) = out.first().copied() {
                out.push(f);
            }
        }
        (false, Some(f), Some(l)) => {
            out.insert(0, *f);
            out.push(*l);
        }
        _ => {}
    }
    out
}

/// `plane` (`w × h`) with a 3 × 3 box blur taken `rounds` times (borders clamp).
fn blur(plane: Vec<f32>, w: usize, h: usize, rounds: usize) -> Vec<f32> {
    let mut cur = plane;
    for _ in 0..rounds {
        let at = |g: &[f32], x: usize, y: usize| g.get(y * w + x).copied().unwrap_or(0.0);
        let mut rows = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                rows[y * w + x] = (at(&cur, x.saturating_sub(1), y) + at(&cur, x, y) + at(&cur, (x + 1).min(w - 1), y)) / 3.0;
            }
        }
        for y in 0..h {
            for x in 0..w {
                cur[y * w + x] = (at(&rows, x, y.saturating_sub(1)) + at(&rows, x, y) + at(&rows, x, (y + 1).min(h - 1))) / 3.0;
            }
        }
    }
    cur
}

/// The contour lines of the gain plane `g` (`w × h`, stops) at `levels`, in normalized coordinates of the plane
/// (`(x + 0.5) / w`), lightly smoothed. Not finite values count as 0; a plane of another size, or under 2 × 2, or
/// over 4 million pixels gives nothing.
pub fn contour_lines(g: &[f32], w: usize, h: usize, levels: &[f32]) -> Vec<Line> {
    if w < 2 || h < 2 || w.saturating_mul(h) > 4_000_000 || g.len() != w * h {
        return Vec::new();
    }
    let g: Vec<f32> = g.iter().map(|v| if v.is_finite() { *v } else { 0.0 }).collect();
    let (lo, hi) = g.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let mut out = Vec::new();
    for &level in levels {
        if !level.is_finite() || level < lo || level > hi {
            continue;
        }
        for t in trace(&g, w, h, level) {
            let points = smooth(&t.points, t.closed);
            let points = points.into_iter().map(|[x, y]| [(x + 0.5) / w as f32, (y + 0.5) / h as f32]).collect();
            out.push(Line { level, closed: t.closed, points });
        }
    }
    out
}

/// The lines of equal gain of `shading` at [`UP`] and [`DOWN`], and at 0 where something darkens.
pub fn contours(shading: &Shading) -> Contours {
    let (sw, sh) = (shading.width, shading.height);
    if sw == 0 || sh == 0 {
        return Contours::default();
    }
    let s = (GRID_EDGE as f64 / sw.max(sh) as f64).min(1.0);
    let (w, h) = (((sw as f64 * s).round() as usize).max(2), ((sh as f64 * s).round() as usize).max(2));
    let mut g = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            g.push(shading.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32).stops);
        }
    }
    let g = blur(g, w, h, BLURS);
    let darkens = g.iter().any(|v| v.is_finite() && *v < -MIN_STOPS);
    let mut levels: Vec<f32> = UP.to_vec();
    if darkens {
        levels.push(0.0);
        levels.extend_from_slice(DOWN);
    }
    Contours { lines: contour_lines(&g, w, h, &levels), darkens }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cone: 2 stops at the middle of a `w × h` plane, falling by 4 stops per plane width.
    fn cone(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32 - 0.5, (i / w) as f32 / h as f32 - 0.5);
                2.0 - 4.0 * (x * x + y * y).sqrt()
            })
            .collect()
    }

    /// The plane's value at the (normalized) point `p`, bilinear, in the plane `g` of `w × h`.
    fn sample(g: &[f32], w: usize, h: usize, p: [f32; 2]) -> f32 {
        let (x, y) = ((p[0] * w as f32 - 0.5).clamp(0.0, (w - 1) as f32), (p[1] * h as f32 - 0.5).clamp(0.0, (h - 1) as f32));
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let a = g[y0 * w + x0] * (1.0 - tx) + g[y0 * w + x1] * tx;
        let b = g[y1 * w + x0] * (1.0 - tx) + g[y1 * w + x1] * tx;
        a * (1.0 - ty) + b * ty
    }

    #[test]
    fn contours_run_at_their_level_closed_inside_and_to_the_edge_outside() {
        let (w, h) = (80, 60);
        let g = cone(w, h);
        let lines = contour_lines(&g, w, h, &[0.5, 1.5, -0.5, -3.0]);
        // 2 stops in the middle, 4 fewer per plane width: 0.5 and 1.5 are rings that fit in the plane
        for level in [0.5, 1.5] {
            let ring: Vec<&Line> = lines.iter().filter(|l| l.level == level).collect();
            assert_eq!(ring.len(), 1, "{level}");
            assert!(ring[0].closed && ring[0].points.first() == ring[0].points.last(), "{level}");
        }
        // −0.5 is reached only in the corners: arcs that begin and end on the plane's edge
        let arcs: Vec<&Line> = lines.iter().filter(|l| l.level == -0.5).collect();
        assert_eq!(arcs.len(), 4);
        for l in &arcs {
            assert!(!l.closed);
            for end in [l.points.first(), l.points.last()].into_iter().flatten() {
                let on_edge = end[1] < 1.5 / h as f32 || end[1] > 1.0 - 1.5 / h as f32 || end[0] < 1.5 / w as f32 || end[0] > 1.0 - 1.5 / w as f32;
                assert!(on_edge, "an open line ends on the plane's edge: {end:?}");
            }
        }
        // −3 is never reached
        assert!(lines.iter().all(|l| l.level != -3.0));
        // every point lies at its line's level (within what the plane's pixels and the smoothing allow)
        for l in &lines {
            for p in &l.points {
                let v = sample(&g, w, h, *p);
                assert!((v - l.level).abs() < 0.12, "{p:?} is at {v}, not {}", l.level);
            }
        }
    }

    #[test]
    fn hostile_planes_give_no_lines_and_no_nan() {
        assert!(contour_lines(&[], 0, 0, UP).is_empty());
        assert!(contour_lines(&[1.0], 1, 1, UP).is_empty());
        assert!(contour_lines(&[1.0, 2.0, 3.0], 2, 2, UP).is_empty(), "a plane of the wrong size");
        assert!(contour_lines(&[f32::NAN; 16], 4, 4, UP).is_empty());
        let mixed: Vec<f32> = (0..25)
            .map(|i| {
                if i % 3 == 0 {
                    f32::NAN
                } else if i % 5 == 0 {
                    f32::INFINITY
                } else {
                    i as f32 * 0.1
                }
            })
            .collect();
        let lines = contour_lines(&mixed, 5, 5, &[0.2, f32::NAN, 0.5, 1.0]);
        assert!(lines.iter().flat_map(|l| &l.points).all(|p| p[0].is_finite() && p[1].is_finite()));
        // a saddle (two opposite corners above the level) must not panic or lose a piece
        assert!(!contour_lines(&[1.0, 0.0, 0.0, 1.0], 2, 2, &[0.5]).is_empty());
    }
}
