//! Wrinkles v2, finding the wrinkles: the centre line of each, with its width and the direction across it.
//!
//! A wrinkle is a long, narrow valley of the log luminance. At each width of [`LADDER`] its zone allows (fractions of
//! the face, [`super::zones`]), a Hessian filter says how deep a valley there is at each pixel and which way it runs;
//! a round dark spot (a freckle, a mole, a pore) answers as much across as along and is damped ([`ROUND`]), and a line
//! running another way than its zone's lines (the forehead's run across the face, the crow's feet out from the eye's
//! corner) counts less ([`super::zones::Zone::fits`]). Each pixel keeps its best width. A pixel is on a centre line when
//! it is the deepest across the line (non-maximum suppression); centre lines are kept when they reach a strong response
//! and are long enough ([`hysteresis`]).

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, par_rows};

use super::zones::{LADDER, Zones};

/// A valley's estimated depth (log light) that starts a wrinkle, and the lower one it may continue through.
const HI: f32 = 0.028;
const LO: f32 = 0.012;
/// The shortest wrinkle, pixels (each zone sets its own, a fraction of the face).
const MIN_PX: usize = 8;
/// The second curvature over the first at which a valley counts as round (a spot, not a line; or the rim round a spot,
/// curved the other way): the response is damped by a Gaussian of the ratio with this sigma.
const ROUND: f32 = 0.4;
/// A Gaussian line's `σ² × curvature` at its best width is this share of its depth.
const PEAK: f32 = 0.385;
/// Along a line, at least this share of the way must hold a valley (from the first value, fully from the second).
const STEADY: (f32, f32) = (0.55, 0.8);
/// Two points of a line run the same way when the cosine between their directions is past the first value (fully past
/// the second).
const SAME: (f32, f32) = (0.85, 0.95);
/// A valley deeper than this (log light, about 2.7 times darker) is not a wrinkle.
const DEEPEST: f32 = 1.0;
/// A round spot: the second curvature at least this share of the first, at the widths of [`LADDER`] up to this one.
const SPOT_RATIO: f32 = 0.5;
const SPOT_WIDEST: usize = 4;
/// A round Gaussian spot's `σ² × second curvature` at its best width is about this share of its depth.
const SPOT_PEAK: f32 = 0.25;
/// A spot this deep (log light) starts to count, and fully from the second; grown by this fraction of the face.
const SPOT_FROM: f32 = 0.05;
const SPOT_FULL: f32 = 0.1;
const SPOT_GROW: f64 = 0.001;
/// The finest blur a width is looked for at, pixels.
const MIN_SIGMA: f32 = 0.8;
/// A pixel's depth is the mean along its line over this many line widths each way (and at least [`ALONG_MIN`] pixels).
const ALONG: f32 = 2.5;
const ALONG_MIN: f32 = 4.0;
/// How far a line's sides may lie: this many of its zone's widest lines (sigma), and [`REACH_PX`] pixels more.
const REACH: f64 = 3.0;
const REACH_PX: f32 = 1.5;

/// A point on a wrinkle's centre line (crop pixels, pixel centres at +0.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Centre {
    pub x: f32,
    pub y: f32,
    /// The unit direction across the line.
    pub n: (f32, f32),
    /// The line's width (Gaussian sigma), pixels.
    pub sigma: f32,
    /// How much of the line goes at 100 (the zone's share, a little less for the widest lines).
    pub keep: f32,
    /// How far out its sides may lie, pixels: three of the zone's widest lines.
    pub reach: f32,
    /// The blur across the zone's lines that evens them out, pixels ([`super::flatten`]).
    pub across: f32,
}

#[derive(Clone, Copy, Default)]
struct Best {
    depth: f32,
    theta: f32,
    width: usize,
    /// The deepest round spot here (a freckle, a mole, a pore), log light.
    spot: f32,
}

/// Wrinkles and the round spots beside them: spots (freckles, moles, age spots) are not lines, are left as they are
/// (Blemishes takes them), and are not read as skin when the lines around them are evened out.
pub(super) struct Found {
    pub centres: Vec<Centre>,
    /// 0…1: how much each pixel belongs to a round dark spot.
    pub spots: Plane,
}

/// The centre lines of the wrinkles in the log luminance `l` of a face crop whose face is `size` pixels across, inside the
/// `zones` and where `allowed` (0…1) is above a half.
pub(super) fn centres(l: &Plane, size: f64, zones: &Zones, allowed: &Plane) -> Found {
    let (w, h) = (l.width, l.height);
    if allowed.data.len() != l.data.len() || zones.index.len() != l.data.len() || w < 3 || h < 3 || !(size.is_finite() && size > 0.0) {
        return Found { centres: Vec::new(), spots: Plane::new(w, h) };
    }
    let mut best = vec![Best::default(); w * h];
    let mut last = 0.0f32;
    for (i, frac) in LADDER.iter().enumerate() {
        if !zones.list.iter().any(|z| (z.widths.0..=z.widths.1).contains(&i)) {
            continue;
        }
        let sigma = ((frac * size) as f32).max(MIN_SIGMA);
        if sigma <= last {
            continue;
        }
        last = sigma;
        let g = gaussian(l, sigma);
        let s2 = sigma * sigma;
        par_rows(&mut best, w, |y, row| {
            for (x, b) in row.iter_mut().enumerate() {
                let Some(zone) = zones.at(x, y) else { continue };
                if !(zone.widths.0..=zone.widths.1).contains(&i) || allowed.data.get(y * w + x).is_none_or(|a| *a < 0.5) {
                    continue;
                }
                let at = |dx: isize, dy: isize| g.get_clamped(x as isize + dx, y as isize + dy);
                let c = at(0, 0);
                let (a, d) = (at(1, 0) - 2.0 * c + at(-1, 0), at(0, 1) - 2.0 * c + at(0, -1));
                let bxy = (at(1, 1) - at(-1, 1) - at(1, -1) + at(-1, -1)) / 4.0;
                let (mean, spread) = ((a + d) / 2.0, (((a - d) / 2.0).powi(2) + bxy * bxy).sqrt());
                let (l1, l2) = (mean + spread, mean - spread);
                if !(l1 > 0.0 && l1.is_finite()) {
                    continue;
                }
                let ratio = l2 / l1;
                if i <= SPOT_WIDEST && ratio > SPOT_RATIO {
                    b.spot = b.spot.max(s2 * l2 / SPOT_PEAK);
                }
                let peak = s2 * l1 / PEAK;
                // far deeper than a wrinkle: a frame's wire, a lash line, a fold of the lid
                // (and no deeper than what a finer width found here: the damping and the fit only take from it, so the
                // direction and the fit are worked out only for a pixel this width may win; issue #253)
                let skip = super::skip_idle();
                if peak > DEEPEST || (skip && peak <= b.depth) {
                    continue;
                }
                let round = (-(ratio * ratio) / (2.0 * ROUND * ROUND)).exp();
                let most = peak * round;
                if skip && most <= b.depth {
                    continue;
                }
                // the direction across the line is the first curvature's: θ = ½·atan2(2b, a − d)
                let theta = 0.5 * (2.0 * bxy).atan2(a - d);
                let t = (-f64::from(theta.sin()), f64::from(theta.cos()));
                let fit = zone.fits((x as f64 + 0.5, y as f64 + 0.5), t);
                let depth = most * fit;
                if depth > b.depth {
                    *b = Best { depth, theta, width: i, spot: b.spot };
                }
            }
        });
    }
    // a line goes on along itself, the skin's grain does not: each pixel's depth is the mean along its line
    let raw = Plane { width: w, height: h, data: best.iter().map(|b| b.depth).collect() };
    let mut depth = Plane::new(w, h);
    par_rows(&mut depth.data, w, |y, row| {
        for (x, d) in row.iter_mut().enumerate() {
            let Some(b) = best.get(y * w + x) else { continue };
            if b.depth < LO {
                continue;
            }
            let sigma = ((LADDER.get(b.width).copied().unwrap_or(0.0) * size) as f32).max(MIN_SIGMA);
            let half = (ALONG * sigma).max(ALONG_MIN).round() as i32;
            let (tx, ty) = (-b.theta.sin(), b.theta.cos());
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (mut sum, mut on) = (0.0f32, 0usize);
            for k in -half..=half {
                let (qx, qy) = (px + k as f32 * tx, py + k as f32 * ty);
                // a point counts when its own valley runs the same way (dense dots have valleys every way)
                // (held at the crop's edge, as `bilinear` holds it: past the right edge the next row is not this one)
                let (cx, cy) = (qx.clamp(0.0, (w - 1) as f32) as usize, qy.clamp(0.0, (h - 1) as f32) as usize);
                let near = best.get(cy * w + cx);
                let same = near.map_or(0.0, |o| super::heal::smoothstep(SAME.0, SAME.1, (o.theta - b.theta).cos().abs()));
                let v = bilinear(&raw, qx, qy) * same;
                sum += v;
                on += usize::from(v > 0.5 * LO);
            }
            // a row of dots (freckles in a line) is not a line: most of the way along, the valley must be there
            let steady = on as f32 / (2 * half + 1) as f32;
            *d = sum / (2 * half + 1) as f32 * super::heal::smoothstep(STEADY.0, STEADY.1, steady);
        }
    });
    // centre lines: the deepest point across each line
    let crest = Plane::from_fn(w, h, |x, y| {
        let (Some(b), Some(&v)) = (best.get(y * w + x), depth.data.get(y * w + x)) else { return 0.0 };
        if v < LO {
            return 0.0;
        }
        let (nx, ny) = (b.theta.cos(), b.theta.sin());
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let side = |k: f32| bilinear(&depth, px + k * nx, py + k * ny);
        if v >= side(1.0) && v >= side(-1.0) { v } else { 0.0 }
    });
    // the shortest line, by zone: a forehead line runs a good part of the way across it, a crow's foot is shorter
    let min_px = |i: usize| zones.at(i % w, i / w).map_or(usize::MAX, |z| ((z.min_len * size) as usize).max(MIN_PX));
    let kept = hysteresis(&crest, LO, HI, min_px);
    // the spots, grown a little so their soft edges go with them
    let spots = Plane { width: w, height: h, data: best.iter().map(|b| super::heal::smoothstep(SPOT_FROM, SPOT_FULL, b.spot)).collect() };
    let grow = ((SPOT_GROW * size).ceil() as usize).max(1);
    let spots = gaussian(&lightcraft_raster::morph::max_filter(&spots, grow), 1.0);
    let mut out: Vec<(f32, Centre)> = Vec::new();
    for (i, k) in kept.data.iter().enumerate() {
        if *k <= 0.0 {
            continue;
        }
        let (x, y) = (i % w, i / w);
        let (Some(b), Some(zone)) = (best.get(i), zones.at(x, y)) else { continue };
        let frac = LADDER.get(b.width).copied().unwrap_or(0.0);
        let wide = smooth(0.006, 0.0135, frac);
        out.push((
            depth.data.get(i).copied().unwrap_or(0.0),
            Centre {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
                n: (b.theta.cos(), b.theta.sin()),
                sigma: ((frac * size) as f32).max(MIN_SIGMA),
                keep: zone.keep * (1.0 - WIDE_KEEP * wide as f32),
                reach: (REACH * LADDER.get(zone.widths.1).copied().unwrap_or(0.0) * size) as f32 + REACH_PX,
                across: ((LADDER.get(zone.widths.1).copied().unwrap_or(0.0) * size) as f32 * super::flatten::ACROSS)
                    .max(super::flatten::ACROSS_LINE * ((frac * size) as f32).max(MIN_SIGMA)),
            },
        ));
    }
    Found { centres: strongest(out, MAX_CENTRES), spots }
}

/// The most centre points a face keeps: the most lined of the test portraits have about 22,000 at the search's size, so a
/// face never comes near it, but a picture that is all lines (a fabric, a screen's moiré) cannot make the evening slow.
const MAX_CENTRES: usize = 80_000;

/// The `most` deepest of `found` (depth, centre), in the order they were found.
fn strongest(mut found: Vec<(f32, Centre)>, most: usize) -> Vec<Centre> {
    if found.len() > most {
        let mut by_depth: Vec<(f32, usize)> = found.iter().enumerate().map(|(i, (d, _))| (*d, i)).collect();
        by_depth.select_nth_unstable_by(most, |a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut keep = vec![false; found.len()];
        for (_, i) in by_depth.iter().take(most) {
            if let Some(k) = keep.get_mut(*i) {
                *k = true;
            }
        }
        let mut k = keep.into_iter();
        found.retain(|_| k.next().unwrap_or(false));
    }
    found.into_iter().map(|(_, c)| c).collect()
}

/// The share of the widest lines (past 1.35 % of the face) left: a deep crease is softened a little less than a fine one.
const WIDE_KEEP: f32 = 0.15;

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `p` read bilinearly at (`x`, `y`) (pixel centres at +0.5), the edge held.
pub(super) fn bilinear(p: &Plane, x: f32, y: f32) -> f32 {
    let (w, h) = (p.width, p.height);
    if w == 0 || h == 0 || !(x.is_finite() && y.is_finite()) {
        return 0.0;
    }
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: usize, y: usize| p.data.get(y * w + x).copied().unwrap_or(0.0);
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bottom - top) * ty
}

/// Pixels above `lo` that connect (8 ways) to one above `hi`, in stretches of at least `min_px(first pixel's index)`
/// pixels.
pub(super) fn hysteresis(r: &Plane, lo: f32, hi: f32, min_px: impl Fn(usize) -> usize) -> Plane {
    let (w, h) = (r.width, r.height);
    let mut seen = vec![false; w.saturating_mul(h)];
    let mut out = Plane::new(w, h);
    let mut stack = Vec::new();
    let mut comp = Vec::new();
    let above = |i: usize| r.data.get(i).is_some_and(|v| *v > lo);
    for start in 0..w * h {
        if seen.get(start).copied().unwrap_or(true) || !above(start) {
            continue;
        }
        comp.clear();
        let mut strong = false;
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        stack.push(start);
        while let Some(i) = stack.pop() {
            comp.push(i);
            strong |= r.data.get(i).is_some_and(|v| *v >= hi);
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if above(j)
                    && let Some(s) = seen.get_mut(j).filter(|s| !**s)
                {
                    *s = true;
                    stack.push(j);
                }
            }
        }
        if strong && comp.len() >= min_px(start) {
            for i in &comp {
                if let Some(o) = out.data.get_mut(*i) {
                    *o = 1.0;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::zones::{Course, NONE, Zone};
    use super::*;
    use crate::wrinkles::Area;

    /// One zone over the whole picture: the forehead's widths, lines across.
    fn forehead(w: usize, h: usize) -> Zones {
        Zones {
            list: vec![Zone { area: Area::Forehead, widths: (0, 5), course: Course::Along((1.0, 0.0)), keep: 1.0, min_len: 0.02 }],
            index: vec![0; w * h],
            weight: Plane::filled(w, h, 1.0),
        }
    }

    /// Skin 400 px across with dark lines (sigma 2 px, 20 % darker) across it at rows 100 and 160, a dark round spot at
    /// (300, 260), and a dark line down it at column 60 from row 220.
    fn picture() -> Plane {
        Plane::from_fn(400, 400, |x, y| {
            let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
            let across = |c: f32| (-0.5 * ((y - c) / 2.0).powi(2)).exp();
            let spot = (-0.5 * (((x - 300.0).powi(2) + (y - 260.0).powi(2)) / 16.0)).exp();
            let down = if y > 220.0 { (-0.5 * ((x - 60.0) / 2.0).powi(2)).exp() } else { 0.0 };
            (0.4f32 * (1.0 - 0.2 * across(100.0).max(across(160.0)).max(spot).max(down))).ln()
        })
    }

    #[test]
    fn lines_across_the_forehead_are_found_and_a_spot_and_a_line_down_it_are_not() {
        let l = picture();
        let found = centres(&l, 1000.0, &forehead(400, 400), &Plane::filled(400, 400, 1.0));
        let c = &found.centres;
        let near = |x: f32, y: f32, r: f32| c.iter().filter(|p| (p.x - x).abs() < r && (p.y - y).abs() < r).count();
        assert!(near(200.0, 100.5, 2.0) > 0 && near(200.0, 160.5, 2.0) > 0, "both lines");
        let on_line = c.iter().filter(|p| (p.y - 100.5).abs() < 1.5).count();
        assert!(on_line > 300, "the line is followed along its length: {on_line}");
        assert!(c.iter().all(|p| (p.y - 100.5).abs() > 4.0 || p.n.1.abs() > 0.95), "across the line is down the face");
        assert_eq!(near(300.0, 260.0, 8.0), 0, "a round spot is not a line");
        assert_eq!(near(60.5, 300.0, 6.0), 0, "a line down the forehead is not one of its lines");
        assert_eq!(near(200.0, 130.0, 10.0), 0, "the skin between the lines");
        assert!(found.spots.get(300, 260) > 0.9, "the spot is a spot");
        assert!(found.spots.get(200, 100) < 0.1 && found.spots.get(200, 130) < 0.1, "a line and skin are not");
    }

    #[test]
    fn nothing_is_found_outside_the_allowed_part_or_the_zones() {
        let l = picture();
        let mut zones = forehead(400, 400);
        let half = Plane::from_fn(400, 400, |x, _| if x < 200 { 1.0 } else { 0.0 });
        assert!(centres(&l, 1000.0, &zones, &half).centres.iter().all(|p| p.x < 200.0));
        zones.index.iter_mut().for_each(|i| *i = NONE);
        assert!(centres(&l, 1000.0, &zones, &Plane::filled(400, 400, 1.0)).centres.is_empty());
        assert!(centres(&l, 1000.0, &zones, &Plane::new(3, 3)).centres.is_empty());
        assert!(centres(&Plane::new(0, 0), 10.0, &forehead(0, 0), &Plane::new(0, 0)).centres.is_empty());
    }

    #[test]
    fn a_face_keeps_its_deepest_centres_up_to_the_cap() {
        let at = |x: f32| Centre { x, y: 0.5, n: (0.0, 1.0), sigma: 1.0, keep: 1.0, reach: 3.0, across: 4.0 };
        let found = vec![(0.02, at(0.5)), (0.09, at(1.5)), (0.05, at(2.5)), (0.07, at(3.5))];
        let kept = strongest(found.clone(), 2);
        assert_eq!(kept.iter().map(|c| c.x).collect::<Vec<_>>(), [1.5, 3.5], "the two deepest, in order");
        assert_eq!(strongest(found, 10).len(), 4, "under the cap all stay");
        assert!(strongest(Vec::new(), 0).is_empty());
    }
}
