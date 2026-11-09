//! Which pixels of a head crop are stray strands: Apple Vision's person matte minus the solid hair mass, thin and
//! elongated pieces only (`fork/retouch-hair.md`, "Stray Hair v2").
//!
//! Everything is a fraction of the face's size in the crop's pixels, so the same face gives the same strands at any
//! crop size (with a floor of a few pixels for a small crop).
//!
//! 1. **The solid** is the matte where it is nearly full, opened (so strands, wisps and fuzz narrower than a lock drop
//!    out). The people parse's Hair part is not used for it: it takes in the flyaways around the hair, which is what is
//!    looked for. A rim of the solid, a few pixels wide, is not looked at: the matte's soft edge is not a strand.
//! 2. **Candidates** are the picture's thin lines (what stands out of its luminance by more than it is wide, dark or light)
//!    where the matte sees the person at all, outside the solid, less what lies over the face's skin, glasses and
//!    jewellery, or clothes, and less where lines crowd (a tuft is hair). The matte says where to look, the picture
//!    where a line is: the matte alone gives faint flyaways no more than a grey cloud.
//! 3. **Strands** are the candidate pieces that are long and stand out, judged where they lie away from the solid (a
//!    thin rim hugging it, which the matte's soft edge can leave, is never a strand); a strand then grows back to its
//!    root and its full width through the weaker candidates.

use lightcraft_raster::Plane;
use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::{max_filter, min_filter};

/// The matte over this is solid (the person); under it, only fuzz.
const SOLID_AT: f32 = 0.85;
/// The solid is the matte opened at this fraction of the face: narrower pieces are not part of it.
const SOLID_OPEN: f64 = 0.012;
/// Gaps in the solid narrower than twice this are closed (fractions of the face).
const SOLID_CLOSE: f64 = 0.03;
/// The rim of the solid that is never a candidate, and how far from it most of a strand must lie.
const RIM: f64 = 0.012;
const AWAY: f64 = 0.05;
/// A strand lies where the matte sees the person at all: coverage over this.
const ZONE_AT: f32 = 0.03;
/// The grey opening and closing the thin lines are taken against, as a radius in fractions of the face: lines narrower
/// than twice it are thin.
const RIDGE: f64 = 0.004;
/// How far a thin line stands out of the picture's luminance (cube root) to count, and to count as a strand for sure.
const WEAK: f32 = 0.03;
const STRONG: f32 = 0.07;
/// Where more than this share of the area (blurred at [`CROWD_BLUR`] of the face) is lines, they are a tuft of hair.
const CROWD: f32 = 0.5;
const CROWD_BLUR: f64 = 0.02;
/// A strand grows back to its root and its full width by this much (a fraction of the face).
const ROOT: f64 = 0.003;
/// A strand is at least this long (fraction of the face) and stands out by this much somewhere.
const MIN_LENGTH: f64 = 0.06;
const MIN_PEAK: f32 = 0.07;
/// What is left alone is grown by these (fractions of the face): the skin, glasses and jewellery, clothes.
const SKIN_GROW: f64 = 0.01;
const OTHERS_GROW: f64 = 0.015;
const CLOTHES_GROW: f64 = 0.008;

/// The parts of the people parse the mask reads, each sampled on the crop's pixels (`None`: the parse has no such part).
#[derive(Default)]
pub(in crate::hair) struct Parts<'a> {
    /// Read only for the clothes (the person's pixels that are neither hair nor skin).
    pub hair: Option<&'a Plane>,
    pub skin: Option<&'a Plane>,
    pub body: Option<&'a Plane>,
    pub others: Option<&'a Plane>,
    pub person: Option<&'a Plane>,
}

/// The strands found: a 0/1 mask the size of the matte, and how many pieces it holds.
pub(in crate::hair) struct Strands {
    pub mask: Plane,
    pub count: u32,
}

/// `v` as 0 or 1 (a value that is not a number is 0).
fn on(v: f32, at: f32) -> f32 {
    if v > at { 1.0 } else { 0.0 }
}

fn open(p: &Plane, r: usize) -> Plane {
    max_filter(&min_filter(p, r), r)
}

/// The strands of the picture whose cube-root luminance is `luma`, outside the person `matte` (Apple Vision's person
/// coverage, the picture's size) holds solid, for a face `face_px` pixels across.
pub(in crate::hair) fn strands<'a>(matte: &Plane, luma: &Plane, parts: &Parts<'a>, face_px: f64) -> Strands {
    let (w, h) = (matte.width, matte.height);
    let none = || Strands { mask: Plane::new(w, h), count: 0 };
    let size_of = |p: &Plane| p.width == w && p.height == h && p.data.len() == matte.data.len();
    if w == 0 || h == 0 || !size_of(matte) || !size_of(luma) || !(face_px.is_finite() && face_px > 0.0) {
        return none();
    }
    let r = |fraction: f64, least: usize| ((fraction * face_px).round().max(least as f64)) as usize;
    let part = |p: Option<&'a Plane>| p.filter(|p| size_of(p));

    // the solid: the person's matte where it is nearly full, opened (the people parse's Hair part is not used: it takes
    // in the flyaways around the hair, which is what is looked for)
    let solid = open(&matte.map(|v| on(v, SOLID_AT)), r(SOLID_OPEN, 3));
    // (and its gaps closed: a patch of the matte that dips inside the hair is not outside it)
    let solid = min_filter(&max_filter(&solid, r(SOLID_CLOSE, 4)), r(SOLID_CLOSE, 4));
    if !solid.data.iter().any(|v| *v > 0.5) {
        // (nobody solid in the crop: no mass to be away from, and nothing to tell a strand from the person)
        return none();
    }
    let near = max_filter(&solid, r(RIM, 2));
    let away = max_filter(&solid, r(AWAY, 4));
    // thin lines of the picture: what stands out of the luminance by more than it is wide, dark or light (the closing and
    // the opening of its grey levels less it): a strand; a lock, an edge or a smooth shade do not
    let ridge_r = r(RIDGE, 2);
    let (closed, opened) = (max_filter_then_min(luma, ridge_r), open(luma, ridge_r));
    let ridge = Plane::from_fn(w, h, |x, y| {
        let k = y * w + x;
        let at = |p: &Plane| p.data.get(k).copied().unwrap_or(0.0);
        (at(&closed) - at(luma)).max(at(luma) - at(&opened)).max(0.0)
    });

    // what is left alone: skin, glasses and jewellery, clothes
    let grown = |p: &Plane, by: f64| max_filter(&p.map(|v| on(v, 0.3)), r(by, 2));
    let mut left = Plane::new(w, h);
    for (p, by) in [(part(parts.skin), SKIN_GROW), (part(parts.others), OTHERS_GROW)] {
        if let Some(p) = p {
            left = left.zip_map(&grown(p, by), f32::max);
        }
    }
    if let Some(person) = part(parts.person) {
        // clothes: the person's pixels that are neither hair nor skin
        let mut body = Plane::new(w, h);
        for p in [parts.hair, parts.skin, parts.body] {
            if let Some(p) = part(p) {
                body = body.zip_map(p, f32::max);
            }
        }
        let clothes = person.zip_map(&body, |p, b| (p - b).max(0.0));
        left = left.zip_map(&grown(&clothes, CLOTHES_GROW), f32::max);
    }

    // where a strand may be: where the matte sees the person at all, outside the solid and what is left alone
    let zone = Plane::from_fn(w, h, |x, y| {
        let k = y * w + x;
        let get = |p: &Plane| p.data.get(k).copied().unwrap_or(0.0);
        if get(matte) > ZONE_AT && get(&near) < 0.5 && get(&left) < 0.5 { 1.0 } else { 0.0 }
    });
    let lines = |at_least: f32| ridge.zip_map(&zone, |v, z| if z > 0.5 && v > at_least { 1.0 } else { 0.0 });
    let (weak, strong) = (lines(WEAK), lines(STRONG));
    // a tuft is hair, not a flyaway: where lines crowd, nothing is taken
    let crowd = gaussian(&weak, r(CROWD_BLUR, 3) as f32);
    let sparse = crowd.map(|c| if c < CROWD { 1.0 } else { 0.0 });
    let weak = weak.zip_map(&sparse, f32::min);
    // strands are judged where they stand out and lie away from the solid, so a rim hugging it never joins one
    let strong = Plane::from_fn(w, h, |x, y| {
        let k = y * w + x;
        let on = |p: &Plane| p.data.get(k).copied().unwrap_or(0.0) > 0.5;
        if on(&strong) && on(&sparse) && away.data.get(k).is_some_and(|a| *a < 0.5) { 1.0 } else { 0.0 }
    });
    let found = pieces(&strong, &ridge, face_px);
    // (and grow back along the weaker line pixels, to the strand's full width and to its root)
    Strands { mask: grow_within(&found.mask, &weak, r(ROOT, 2)), count: found.count }
}

/// `p` closed by `r` (the grey closing: the maximum, then the minimum, over squares of radius `r`).
fn max_filter_then_min(p: &Plane, r: usize) -> Plane {
    min_filter(&max_filter(p, r), r)
}

/// `seed` grown by `steps` pixels (a square each) where `allowed` is on: a strand back to its root.
fn grow_within(seed: &Plane, allowed: &Plane, steps: usize) -> Plane {
    let mut cur = seed.clone();
    for _ in 0..steps {
        let next = max_filter(&cur, 1).zip_map(allowed, |g, a| if a > 0.5 { g } else { 0.0 }).zip_map(&cur, f32::max);
        if next.data == cur.data {
            break;
        }
        cur = next;
    }
    cur
}

/// The pieces of `thin` (8-connected) that are strands: long and standing out (`stand_out` is how much, per pixel).
fn pieces(thin: &Plane, stand_out: &Plane, face_px: f64) -> Strands {
    let (w, h) = (thin.width, thin.height);
    let mut seen = vec![false; thin.data.len()];
    let mut mask = Plane::new(w, h);
    let mut count = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    let mut members: Vec<usize> = Vec::new();
    let min_length = (MIN_LENGTH * face_px).max(8.0);
    for start in 0..thin.data.len() {
        if seen.get(start).copied().unwrap_or(true) || thin.data.get(start).copied().unwrap_or(0.0) <= 0.5 {
            continue;
        }
        members.clear();
        stack.push(start);
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        while let Some(k) = stack.pop() {
            members.push(k);
            let (x, y) = ((k % w) as isize, (k / w) as isize);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if thin.data.get(n).is_some_and(|v| *v > 0.5)
                        && let Some(s) = seen.get_mut(n)
                        && !*s
                    {
                        *s = true;
                        stack.push(n);
                    }
                }
            }
        }
        if is_strand(&members, w, stand_out, min_length) {
            count = count.saturating_add(1);
            for k in &members {
                if let Some(m) = mask.data.get_mut(*k) {
                    *m = 1.0;
                }
            }
        }
    }
    Strands { mask, count }
}

/// Whether the pixels `members` (indices into a `w`-wide plane) are a strand: length by the spread along their main
/// direction, and how much they stand out.
fn is_strand(members: &[usize], w: usize, stand_out: &Plane, min_length: f64) -> bool {
    let n = members.len() as f64;
    if n < min_length {
        return false;
    }
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut peak = 0.0f32;
    for k in members {
        let (x, y) = ((k % w) as f64, (k / w) as f64);
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
        peak = peak.max(stand_out.data.get(*k).copied().unwrap_or(0.0));
    }
    let (mx, my) = (sx / n, sy / n);
    let (vxx, vyy, vxy) = (sxx / n - mx * mx, syy / n - my * my, sxy / n - mx * my);
    // the larger eigenvalue of the covariance: a line of length L has 1/12 L² along it
    let half = (vxx + vyy) / 2.0;
    let spread = (((vxx - vyy) / 2.0).powi(2) + vxy * vxy).sqrt();
    let length = (12.0 * (half + spread).max(0.0)).sqrt();
    length >= min_length && peak >= MIN_PEAK
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACE: f64 = 100.0;

    /// A 300 × 300 matte: a solid disc (the head) with a soft rim, and what `extra` draws.
    fn matte(extra: impl Fn(f32, f32) -> f32) -> Plane {
        Plane::from_fn(300, 300, |x, y| {
            let d = ((x as f32 - 150.0).powi(2) + (y as f32 - 160.0).powi(2)).sqrt();
            // 1 inside 70, a rim fading out over 6 pixels
            let disc = (1.0 - (d - 70.0) / 6.0).clamp(0.0, 1.0);
            disc.max(extra(x as f32, y as f32))
        })
    }

    /// A line from (x0, y0) to (x1, y1), `width` pixels across, coverage 1.
    fn line(x0: f32, y0: f32, x1: f32, y1: f32, width: f32) -> impl Fn(f32, f32) -> f32 {
        move |x, y| {
            let (dx, dy) = (x1 - x0, y1 - y0);
            let t = (((x - x0) * dx + (y - y0) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            let d = ((x - (x0 + t * dx)).powi(2) + (y - (y0 + t * dy)).powi(2)).sqrt();
            if d <= width / 2.0 { 1.0 } else { 0.0 }
        }
    }

    /// The picture of a matte: a dark person on a light ground.
    fn luma(m: &Plane) -> Plane {
        m.map(|v| 0.8 - 0.6 * v)
    }

    fn wrong_luma() -> Plane {
        Plane::filled(5, 5, 0.5)
    }

    fn at(s: &Strands, x: usize, y: usize) -> bool {
        s.mask.get(x, y) > 0.5
    }

    #[test]
    fn a_thin_strand_off_the_head_is_kept_and_the_head_and_its_rim_are_not() {
        // a hair 2 px wide and 35 px long leaving the head at the top
        let m = matte(line(150.0, 92.0, 140.0, 56.0, 2.0));
        let s = strands(&m, &luma(&m), &Parts::default(), FACE);
        assert_eq!(s.count, 1);
        assert!(at(&s, 142, 62), "the strand away from the head is in the mask");
        assert!(!at(&s, 150, 160), "the head's solid is not");
        assert!(!at(&s, 150, 92), "the rim of the head is not");
        let rim: usize =
            (0..300).flat_map(|y| (0..300).map(move |x| (x, y))).filter(|(x, y)| at(&s, *x, *y) && (*y > 100 || *x < 110 || *x > 190)).count();
        assert_eq!(rim, 0, "nothing along the head's edge");
    }

    #[test]
    fn a_rim_that_only_hugs_the_head_is_not_a_strand() {
        // a faint ring of fuzz hugging the head's edge, 1 px wide
        let m = matte(|x, y| {
            let d = ((x - 150.0).powi(2) + (y - 160.0).powi(2)).sqrt();
            if (72.5..73.5).contains(&d) { 0.5 } else { 0.0 }
        });
        let s = strands(&m, &luma(&m), &Parts::default(), FACE);
        assert_eq!(s.count, 0);
        assert!(s.mask.data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn a_thick_lock_and_a_short_speck_are_left_alone() {
        // a lock 14 px wide, and a 4 px speck, off the head
        let lock = line(60.0, 130.0, 20.0, 130.0, 14.0);
        let speck = line(260.0, 40.0, 262.0, 44.0, 3.0);
        let m = matte(|x, y| lock(x, y).max(speck(x, y)));
        let s = strands(&m, &luma(&m), &Parts::default(), FACE);
        assert_eq!(s.count, 0, "no strand");
    }

    #[test]
    fn glasses_and_jewellery_are_left_alone() {
        // a thin line that is a glasses arm (Others) is left alone; the same line elsewhere is a strand
        let arm = line(224.0, 150.0, 262.0, 150.0, 2.0);
        let strand = line(150.0, 92.0, 140.0, 56.0, 2.0);
        let m = matte(|x, y| arm(x, y).max(strand(x, y)));
        let others = Plane::from_fn(300, 300, |x, y| if (215..270).contains(&x) && (140..160).contains(&y) { 1.0 } else { 0.0 });
        let s = strands(&m, &luma(&m), &Parts { others: Some(&others), ..Parts::default() }, FACE);
        assert_eq!(s.count, 1, "only the strand");
        assert!(at(&s, 142, 62) && !at(&s, 250, 150));
        let without = strands(&m, &luma(&m), &Parts::default(), FACE);
        assert_eq!(without.count, 2, "without the part both are strands");
    }

    #[test]
    fn a_strand_over_the_face_skin_is_ignored() {
        let strand = line(150.0, 92.0, 140.0, 56.0, 2.0);
        let m = matte(strand);
        let skin = Plane::filled(300, 300, 1.0);
        assert_eq!(strands(&m, &luma(&m), &Parts { skin: Some(&skin), ..Parts::default() }, FACE).count, 0);
    }

    #[test]
    fn odd_input_gives_nothing_and_no_panic() {
        let empty = Plane::new(0, 0);
        assert_eq!(strands(&empty, &empty, &Parts::default(), FACE).count, 0);
        let m = matte(|_, _| 0.0);
        assert_eq!(strands(&m, &luma(&m), &Parts::default(), f64::NAN).count, 0);
        assert_eq!(strands(&m, &luma(&m), &Parts::default(), 0.0).count, 0);
        let nothing = Plane::new(40, 40);
        assert_eq!(strands(&nothing, &luma(&nothing), &Parts::default(), FACE).count, 0);
        // a picture of another size is not searched
        assert_eq!(strands(&m, &wrong_luma(), &Parts::default(), FACE).count, 0);
        // a part of another size is not read
        let wrong = Plane::filled(5, 5, 1.0);
        assert_eq!(strands(&m, &luma(&m), &Parts { hair: Some(&wrong), skin: Some(&wrong), ..Parts::default() }, FACE).count, 0);
    }
}
