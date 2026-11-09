//! Small gaps in the hair and faint patches in the person's mask are the person. The people parse and the hair
//! matte mark gaps between curls as background, and give an earring or a strand lying over the face a few percent of
//! background: a swapped or blurred background then showed there as rings inside the hair. So:
//! - a hole (mask under 0.5) whose sides reach no border of the mask, that is at most [`HOLE`] of the mask's longer
//!   side across, and that lies in or along the person's hair (their Hair mask reaches it within [`SEAL`]) is filled,
//!   with its soft rim. A closed pocket of background elsewhere (between an arm and the body) stays background.
//! - a pixel of at least [`FAINT`] with no background (mask under 0.5) within [`INSIDE`] of the mask's longer side
//!   is wholly person.
//!
//! Done on the mask itself (in the photo's normalised frame), so every render and every window of one sees the same
//! mask. The kept work of a picture keeps the filled masks.

use std::collections::VecDeque;

use lightcraft_raster::Plane;

use crate::PartMask;

/// The widest hole filled, a fraction of the mask's longer side.
pub const HOLE: f64 = 0.03;
/// How far round a filled hole its soft rim is closed too, a fraction of the mask's longer side.
pub const SEAL: f64 = 0.004;
/// A pixel this far (a fraction of the mask's longer side) from any background is wholly person.
pub const INSIDE: f64 = 0.012;
/// The least mask value the rule above lifts (a faint patch the matte left in the person, not a soft edge).
pub const FAINT: f32 = 0.75;
/// The filled mask, shared (see the module docs).
pub(crate) fn filled_kept(m: &PartMask, hair: Option<&PartMask>) -> std::sync::Arc<PartMask> {
    std::sync::Arc::new(filled(m, hair))
}

/// `m` with the small holes of its hair (`hair`: the person's Hair mask) filled; `m` itself (a copy, values made
/// sane) when it has none or is malformed.
pub(crate) fn filled(m: &PartMask, hair: Option<&PartMask>) -> PartMask {
    let (w, h) = (m.plane.width, m.plane.height);
    if w == 0 || h == 0 || m.plane.data.len() != w.saturating_mul(h) {
        return m.clone();
    }
    let long = w.max(h) as f64;
    let max_side = (HOLE * long).ceil() as usize;
    let on: Vec<bool> = m.plane.data.iter().map(|v| v.is_finite() && *v > 0.5).collect();
    let mut seen = vec![false; w * h];
    let mut holes = Plane::new(w, h);
    let mut any = false;
    let mut queue = VecDeque::new();
    let mut pixels = Vec::new();
    for start in 0..w * h {
        if on.get(start).copied().unwrap_or(true) || seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        // one background region (4-connected)
        pixels.clear();
        queue.push_back(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        let (mut x0, mut y0, mut x1, mut y1, mut border) = (usize::MAX, usize::MAX, 0, 0, false);
        while let Some(i) = queue.pop_front() {
            let (x, y) = (i % w, i / w);
            pixels.push(i);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            border |= x == 0 || y == 0 || x + 1 == w || y + 1 == h;
            let around = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for n in around.into_iter().flatten() {
                if !on.get(n).copied().unwrap_or(true) && seen.get(n).is_some_and(|s| !s) {
                    if let Some(s) = seen.get_mut(n) {
                        *s = true;
                    }
                    queue.push_back(n);
                }
            }
        }
        if !border && x1 - x0 < max_side && y1 - y0 < max_side && hair.is_some_and(|hm| in_hair(m, hm, &pixels, w, h, long)) {
            any = true;
            for &i in &pixels {
                if let Some(v) = holes.data.get_mut(i) {
                    *v = 1.0;
                }
            }
        }
    }
    let mut data: Vec<f32> = m.plane.data.iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }).collect();
    if any {
        let rim = lightcraft_raster::morph::max_filter(&holes, (SEAL * long).ceil().max(1.0) as usize);
        data.iter_mut().zip(&rim.data).for_each(|(v, r)| *v = v.max(*r));
    }
    // background near each pixel: none within INSIDE makes it wholly person
    let bg = Plane { width: w, height: h, data: data.iter().map(|v| if *v < 0.5 { 1.0 } else { 0.0 }).collect() };
    let near = lightcraft_raster::morph::max_filter(&bg, (INSIDE * long).ceil().max(1.0) as usize);
    data.iter_mut().zip(&near.data).for_each(|(v, n)| {
        if *n <= 0.0 && *v >= FAINT {
            *v = 1.0;
        }
    });
    PartMask { plane: Plane { width: w, height: h, data }, area: m.area }
}

/// Whether the Hair mask `hair` reaches the hole `pixels` (indices into `m`'s `w × h` plane) within [`SEAL`]: a
/// point of the hole, or one [`SEAL`] away from it in eight directions, is hair.
fn in_hair(m: &PartMask, hair: &PartMask, pixels: &[usize], w: usize, h: usize, long: f64) -> bool {
    let r = (SEAL * long).ceil().max(1.0);
    let step = (pixels.len() / 64).max(1);
    let dirs = [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r), (r, r), (-r, -r), (r, -r), (-r, r)];
    pixels.iter().step_by(step).any(|&i| {
        let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
        dirs.iter().any(|(dx, dy)| {
            let nx = m.area.x0 + (x + dx) / w as f64 * m.area.width();
            let ny = m.area.y0 + (y + dy) / h as f64 * m.area.height();
            hair.sample(nx, ny) > 0.5
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NormRect;

    #[test]
    fn a_small_hole_in_the_hair_is_filled_and_a_pocket_elsewhere_a_gap_to_the_border_or_a_big_hole_is_not() {
        let (w, h) = (200, 200);
        let plane = Plane::from_fn(w, h, |x, y| {
            let in_hair = (50..53).contains(&x) && (50..53).contains(&y);
            let pocket = (60..63).contains(&x) && (150..153).contains(&y);
            let big = (100..160).contains(&x) && (100..160).contains(&y);
            let gap = x > 185;
            if in_hair || pocket || big || gap { 0.0 } else { 1.0 }
        });
        let area = NormRect::new(0.0, 0.0, 1.0, 1.0);
        let m = PartMask { plane, area };
        // hair over the top third
        let hair = PartMask { plane: Plane::from_fn(50, 50, |_, y| if y < 17 { 1.0 } else { 0.0 }), area };
        let f = filled(&m, Some(&hair));
        assert_eq!(f.plane.get(51, 51), 1.0, "the small hole in the hair is the person");
        assert_eq!(f.plane.get(61, 151), 0.0, "a pocket away from the hair stays background");
        assert_eq!(f.plane.get(130, 130), 0.0, "a big hole stays background");
        assert_eq!(f.plane.get(195, 10), 0.0, "a gap reaching the border stays background");
        assert_eq!(filled(&m, None).plane.get(51, 51), 0.0, "without a Hair mask no hole is filled");
        assert_eq!(f.area, m.area);
        let empty = PartMask { plane: Plane::new(0, 0), area: m.area };
        assert_eq!(filled(&empty, None), empty);
        // a faint ring well inside the person (an earring) is the person; the soft edge next to the background stays
        let ring = Plane::from_fn(w, h, |x, y| {
            if x > 150 {
                0.0
            } else if (60..64).contains(&x) && (20..140).contains(&y) {
                0.85
            } else if x > 146 {
                0.6
            } else {
                1.0
            }
        });
        let f = filled(&PartMask { plane: ring, area: m.area }, None);
        assert_eq!(f.plane.get(62, 80), 1.0);
        assert!((f.plane.get(148, 80) - 0.6).abs() < 1e-6);
    }
}
