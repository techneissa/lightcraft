//! Spots that cover a shape: a person's mask (dabs on a grid) or a wire's path (dabs along it), grown a little.
//!
//! A spot in the develop settings is a stroke: dab centres and one radius, each pixel within the radius of a centre
//! is rebuilt (full strength inside `1 − 0.8 · feather` of the radius). So a shape is covered by choosing one radius,
//! then centres close enough that every shape pixel is inside some dab's full-strength disc.

use super::shape::{Mask, dilate, distance_sq};
use super::{SPOT_FEATHER, SpotPlan};

/// How far a mask is grown before it is covered, as a share of its grid's long edge (SAM masks are loose-ish at the
/// edge, and a hair or a strap just outside the outline is part of what the eye reads as the person).
pub const GROW: f64 = 0.005;
/// Most dabs one spot holds: the render checks every pixel of a spot's box against every dab.
pub const MAX_DABS_PER_SPOT: usize = 48;
/// Most dabs one shape gets; past it the dabs grow instead of multiplying.
pub const MAX_DABS: usize = 480;
/// Largest dab radius (a fraction of the long edge), the Remove tool's own limit.
const MAX_SIZE: f64 = 0.25;
/// Smallest dab radius in pixels of the grid.
const MIN_RADIUS_PX: f64 = 1.5;

/// The share of a dab's radius that is at full strength.
fn full_strength() -> f64 {
    1.0 - 0.8 * (SPOT_FEATHER / 100.0)
}

/// Spots covering `shape`, grown by [`GROW`]. `grid` pixels span the whole photo (`shape.w × shape.h`); the radius is
/// half the widest part of the grown shape (more is overreach, much less is hundreds of dabs), and dabs sit on a
/// grid so close that a cell's far corner is still inside its dab's full-strength disc.
pub fn cover_mask(shape: &Mask) -> Vec<SpotPlan> {
    let (w, h) = (shape.w, shape.h);
    if w == 0 || h == 0 || shape.bits.len() != w * h || shape.count() == 0 {
        return Vec::new();
    }
    let long = w.max(h) as f64;
    let grown = dilate(shape, ((GROW * long) as f32).max(1.0));
    let inside = distance_sq(w, h, |i| !grown.bits.get(i).copied().unwrap_or(false));
    let widest = inside.iter().copied().fold(0.0f32, f32::max).sqrt() as f64;
    let mut r = (0.55 * widest).max(MIN_RADIUS_PX);
    let cap = MAX_SIZE * long;
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for (i, b) in grown.bits.iter().enumerate() {
        if *b {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    // the cell centres, row by row (cells that hold a pixel of the grown shape)
    let grid = |r: f64| {
        // a cell's far corner is half its diagonal from its centre
        let g = (r * full_strength() * std::f64::consts::SQRT_2 * 0.92).max(1.0);
        let (cols, rows) = (((x1 - x0 + 1) as f64 / g).ceil() as usize + 1, ((y1 - y0 + 1) as f64 / g).ceil() as usize + 1);
        let mut hit = vec![false; cols.saturating_mul(rows)];
        for y in y0..=y1 {
            for x in x0..=x1 {
                if grown.bits[y * w + x] {
                    let (cx, cy) = (((x - x0) as f64 / g) as usize, ((y - y0) as f64 / g) as usize);
                    if let Some(c) = hit.get_mut(cy * cols + cx) {
                        *c = true;
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (i, c) in hit.iter().enumerate() {
            if *c {
                let (cx, cy) = (i % cols, i / cols);
                let px = (x0 as f64 + (cx as f64 + 0.5) * g).clamp(0.5, w as f64 - 0.5);
                let py = (y0 as f64 + (cy as f64 + 0.5) * g).clamp(0.5, h as f64 - 0.5);
                out.push((px / w as f64, py / h as f64));
            }
        }
        out
    };
    let mut dabs = grid(r);
    while dabs.len() > MAX_DABS && r < cap {
        r = (r * 1.25).min(cap);
        dabs = grid(r);
    }
    let size = (r / long).clamp(0.001, MAX_SIZE);
    dabs.chunks(MAX_DABS_PER_SPOT).map(|c| SpotPlan { points: c.to_vec(), size }).collect()
}

/// Spots covering the path `pts` (grid pixels), a line `width_px` wide, grown by [`GROW`] and a pixel: dabs along it,
/// so close that the strip between them is at full strength. `w × h` is the grid spanning the photo.
pub fn cover_path(pts: &[(f64, f64)], width_px: f64, w: usize, h: usize) -> Vec<SpotPlan> {
    if pts.len() < 2 || w == 0 || h == 0 {
        return Vec::new();
    }
    let long = w.max(h) as f64;
    let r = ((width_px / 2.0 + GROW * long + 1.0) * 1.3).clamp(MIN_RADIUS_PX, MAX_SIZE * long);
    // along the path every `step` pixels
    let step = (r * full_strength()).max(1.0);
    let mut dabs: Vec<(f64, f64)> = Vec::new();
    let mut carry = 0.0;
    dabs.push(pts[0]);
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if !len.is_finite() || len <= 0.0 {
            continue;
        }
        let mut t = step - carry;
        while t <= len && dabs.len() < MAX_DABS * 8 {
            dabs.push((a.0 + (b.0 - a.0) * t / len, a.1 + (b.1 - a.1) * t / len));
            t += step;
        }
        carry = len - (t - step);
    }
    if let (Some(last), Some(end)) = (dabs.last(), pts.last())
        && (last.0 - end.0).hypot(last.1 - end.1) > step * 0.25
    {
        dabs.push(*end);
    }
    let size = (r / long).clamp(0.001, MAX_SIZE);
    let to_norm = |p: &(f64, f64)| ((p.0 / w as f64).clamp(0.0, 1.0), (p.1 / h as f64).clamp(0.0, 1.0));
    // consecutive spots share a dab, so no seam is left between them
    let mut out = Vec::new();
    let mut i = 0;
    while i < dabs.len() {
        let end = (i + MAX_DABS_PER_SPOT).min(dabs.len());
        out.push(SpotPlan { points: dabs[i..end].iter().map(to_norm).collect(), size });
        if end == dabs.len() {
            break;
        }
        i = end - 1;
    }
    out
}

/// Tests: whether every pixel of `shape` is at full strength under `spots`.
#[cfg(test)]
pub(crate) fn uncovered(shape: &Mask, spots: &[SpotPlan]) -> usize {
    let (w, h) = (shape.w as f64, shape.h as f64);
    let long = w.max(h);
    let mut missed = 0;
    for (i, b) in shape.bits.iter().enumerate() {
        if !*b {
            continue;
        }
        let (px, py) = ((i % shape.w) as f64 + 0.5, (i / shape.w) as f64 + 0.5);
        let ok = spots.iter().any(|s| {
            let r = s.size * long * full_strength();
            s.points.iter().any(|p| (p.0 * w - px).hypot(p.1 * h - py) <= r)
        });
        if !ok {
            missed += 1;
        }
    }
    missed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_is_covered_by_a_few_dabs() {
        // a standing figure: head and body
        let m = Mask::from_fn(400, 300, |x, y| {
            let (x, y) = (x as f64, y as f64);
            ((x - 200.0).powi(2) + (y - 70.0).powi(2)).sqrt() < 22.0 || ((x - 200.0).abs() < 34.0 && (95.0..260.0).contains(&y))
        });
        let spots = cover_mask(&m);
        assert!(!spots.is_empty());
        assert_eq!(uncovered(&m, &spots), 0);
        let dabs: usize = spots.iter().map(|s| s.points.len()).sum();
        assert!(dabs < 120, "{dabs} dabs");
        assert!(spots.iter().all(|s| s.points.len() <= MAX_DABS_PER_SPOT && s.size > 0.0 && s.size <= MAX_SIZE));
        assert!(spots.iter().flat_map(|s| &s.points).all(|p| (0.0..=1.0).contains(&p.0) && (0.0..=1.0).contains(&p.1)));
    }

    #[test]
    fn a_blob_at_the_edge_is_covered_too() {
        let m = Mask::from_fn(200, 100, |x, y| x >= 170 && y >= 60);
        let spots = cover_mask(&m);
        assert_eq!(uncovered(&m, &spots), 0);
    }

    #[test]
    fn nothing_and_nonsense_give_no_spots() {
        assert!(cover_mask(&Mask::new(40, 40)).is_empty());
        assert!(cover_mask(&Mask::new(0, 0)).is_empty());
        assert!(cover_mask(&Mask { w: 4, h: 4, bits: vec![true; 3] }).is_empty());
        assert!(cover_path(&[(1.0, 1.0)], 3.0, 100, 100).is_empty());
        assert!(cover_path(&[(1.0, 1.0), (f64::NAN, 2.0)], 3.0, 100, 100).iter().all(|s| s.size.is_finite()));
    }

    #[test]
    fn a_path_is_covered_in_chunks_that_overlap() {
        let (w, h) = (1600usize, 900usize);
        let pts: Vec<(f64, f64)> = (0..=1500).step_by(10).map(|x| (x as f64 + 40.0, 300.0 + 0.0002 * (x as f64 - 750.0).powi(2))).collect();
        let spots = cover_path(&pts, 3.0, w, h);
        assert!(spots.len() >= 3, "{} spots", spots.len());
        assert!(spots.iter().all(|s| s.points.len() <= MAX_DABS_PER_SPOT));
        // consecutive spots share their end dab
        for pair in spots.windows(2) {
            assert_eq!(pair[0].points.last(), pair[1].points.first());
        }
        // every point of the path is at full strength under some dab
        let long = w as f64;
        for p in &pts {
            let near = spots.iter().any(|s| {
                let r = s.size * long * full_strength();
                s.points.iter().any(|q| (q.0 * w as f64 - p.0).hypot(q.1 * h as f64 - p.1) <= r)
            });
            assert!(near, "{p:?}");
        }
    }
}
