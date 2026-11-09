//! Wrinkles v2, finding the wrinkles: a soft 0…1 mask, thin along each line but grown to the line's width plus its shaded
//! flanks, inside the part of the face a search allows ([`super::search`]: skin, in an area, not an ear, a nostril, the
//! face's edge or the hairline).
//!
//! A wrinkle is a valley of the log luminance. At four widths ([`SIGMAS`], fractions of the face) a Hessian ridge filter
//! says how much of a valley there is at each pixel; only the crest of each response counts, so two neighbouring lines
//! stay apart; a stretch of crest is kept when it connects to a strong response and is long enough
//! ([`hysteresis`]); and each width's lines are grown a little ([`GROW`] line widths past the crest) so the flanks go
//! too. Lines as wide as their spacing would merge into one flat patch, so the growth is small. The union is softened
//! ([`SOFT`]) and cut by the allowed part.

use lightcraft_raster::Plane;
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::max_filter;

use super::heal::smoothstep;

/// The line widths looked for (the Gaussian sigma of a line), fractions of the face.
pub(super) const SIGMAS: [f64; 4] = [0.0025, 0.005, 0.01, 0.018];
/// A response (log light depth) that starts a wrinkle, and the lower one it may continue through.
const HI: f32 = 0.05;
const LO: f32 = 0.025;
/// The shortest wrinkle, a fraction of the face: a stretch of response of fewer pixels than its square is dropped.
const MIN_LEN: f64 = 0.025;
/// A width keeps a pixel when its response there is at least this share of the best width's.
const OWN: f32 = 0.85;
/// How far a mask grows past a line's crest, in line widths (a width is 2.4 sigma).
const GROW: f64 = 0.35;
/// The mask's edge softness (sigma), a fraction of the face.
pub(super) const SOFT: f64 = 0.0035;

/// The wrinkle mask of the log luminance `l` of a face crop whose face is `size` pixels across, where `allowed`
/// (0…1) lets it. All planes are the crop's size.
pub(super) fn wrinkle_mask(l: &Plane, size: f64, allowed: &Plane) -> Plane {
    let (w, h) = (l.width, l.height);
    let mut union = Plane::new(w, h);
    if allowed.data.len() != l.data.len() || w == 0 || h == 0 {
        return union;
    }
    // a response outside the allowed part is dropped before stretches are connected: an ear's fold must not
    // lead a wrinkle into the face
    let inside = allowed.map(|a| if a > 0.5 { 1.0 } else { 0.0 });
    let min_px = ((MIN_LEN * size) * (MIN_LEN * size)).max(4.0) as usize;
    // each width's response, and at each pixel the strongest of them: a thin line also answers at the wider widths, and
    // would be grown by those into a broad stripe, so a width keeps only the pixels where it is (nearly) the best
    let sigmas: Vec<f32> = SIGMAS.iter().map(|s| (s * size).max(1.0) as f32).collect();
    let responses: Vec<Plane> = sigmas.iter().map(|sigma| valley_response(l, *sigma).zip_map(&inside, |r, i| r * i)).collect();
    let best = responses.iter().fold(Plane::new(w, h), |b, r| b.zip_map(r, f32::max));
    for (response, sigma) in responses.iter().zip(&sigmas) {
        let owned = response.zip_map(&best, |r, b| if r >= OWN * b { r } else { 0.0 });
        let core = hysteresis(&crest(&owned, *sigma), LO, HI, min_px);
        let grown = max_filter(&core, (GROW * 2.4 * f64::from(*sigma)).ceil() as usize + 1);
        union.data.iter_mut().zip(&grown.data).for_each(|(u, g)| *u = u.max(*g));
    }
    let soft = gaussian(&union, (SOFT * size).max(0.5) as f32).map(|v| smoothstep(0.15, 0.85, v));
    soft.zip_map(allowed, |m, a| m * a)
}

/// How much of a valley (dark line) of width `sigma` there is at each pixel, in log-light depth: the larger Hessian
/// eigenvalue of the blurred picture times `sigma²`, damped where the second one is as large (a dark spot).
pub(super) fn valley_response(l: &Plane, sigma: f32) -> Plane {
    let g = gaussian(l, sigma);
    let (w, h) = (g.width, g.height);
    let s2 = sigma * sigma;
    Plane::from_fn(w, h, |x, y| {
        let at = |dx: isize, dy: isize| g.get_clamped(x as isize + dx, y as isize + dy);
        let c = at(0, 0);
        let (a, d) = (at(1, 0) - 2.0 * c + at(-1, 0), at(0, 1) - 2.0 * c + at(0, -1));
        let b = (at(1, 1) - at(-1, 1) - at(1, -1) + at(-1, -1)) / 4.0;
        let (mean, spread) = ((a + d) / 2.0, (((a - d) / 2.0).powi(2) + b * b).sqrt());
        let (l1, l2) = (mean + spread, mean - spread);
        if l1 <= 0.0 || !l1.is_finite() {
            return 0.0;
        }
        let ratio = l2 / l1;
        s2 * l1 * (-(ratio * ratio) / (2.0 * 0.5 * 0.5)).exp()
    })
}

/// `r` kept only near the crest of each line: where it is at least 60 % of its largest value within a line width.
pub(super) fn crest(r: &Plane, sigma: f32) -> Plane {
    let local = max_filter(r, sigma.ceil() as usize);
    r.zip_map(&local, |v, m| if v >= 0.6 * m { v } else { 0.0 })
}

/// Pixels above `lo` that connect (8 ways) to one above `hi`, in stretches of at least `min_px` pixels.
pub(super) fn hysteresis(r: &Plane, lo: f32, hi: f32, min_px: usize) -> Plane {
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
        if strong && comp.len() >= min_px {
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
    use super::*;

    /// Skin 200 px across, dark lines (3 px wide, 25 % darker) across it at rows 60 and 100, and a dark ear fold at
    /// column 10 (which `allowed` lets out).
    fn lined() -> (Plane, Plane) {
        let (w, h) = (200, 200);
        let l = Plane::from_fn(w, h, |x, y| {
            let line = |c: f32| (-0.5 * ((y as f32 - c) / 1.5).powi(2)).exp();
            let fold = (-0.5 * ((x as f32 - 10.0) / 1.5).powi(2)).exp();
            (0.4f32 * (1.0 - 0.25 * (line(60.0).max(line(100.0)).max(fold)))).ln()
        });
        let allowed = Plane::from_fn(w, h, |x, _| if x >= 30 { 1.0 } else { 0.0 });
        (l, allowed)
    }

    #[test]
    fn lines_are_found_thin_and_what_is_not_allowed_is_not() {
        let (l, allowed) = lined();
        let mask = wrinkle_mask(&l, 200.0, &allowed);
        let at = |x: usize, y: usize| mask.data.get(y * 200 + x).copied().unwrap_or(f32::NAN);
        assert!(at(100, 60) > 0.9 && at(100, 100) > 0.9, "the lines are in the mask");
        assert!(at(100, 80) < 0.01, "the skin between them is not");
        assert!(at(100, 140) < 0.01 && at(100, 20) < 0.01, "skin without a line is not");
        assert!(at(10, 100) == 0.0 && at(10, 60) == 0.0, "the fold outside the allowed part is not, nor is its line's end");
        // thin: a few pixels each side of the crest, never a patch
        let rows: usize = (0..200).filter(|y| at(100, *y) > 0.5).count();
        assert!(rows <= 34, "two thin lines, {rows} rows");
    }

    #[test]
    fn a_picture_that_does_not_match_its_allowed_part_gives_an_empty_mask() {
        let l = Plane::filled(10, 10, 0.1);
        for allowed in [Plane::new(3, 3), Plane::new(0, 0)] {
            assert!(wrinkle_mask(&l, 10.0, &allowed).data.iter().all(|v| *v == 0.0));
        }
        assert!(wrinkle_mask(&Plane::new(0, 0), 10.0, &Plane::new(0, 0)).data.is_empty());
    }
}
