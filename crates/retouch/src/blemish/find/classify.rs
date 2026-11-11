//! Each spot's kind, from its look ([`Traits`]) and its neighbours:
//!
//! - **Blemish (red)**: blood darkens green and leaves red: red darkens much less than green ([`BLOOD_RED`]) and
//!   blue less than green too ([`BLOOD_BLUE`]; melanin darkens blue more), or the skin around is redder than the
//!   skin farther out (an inflamed halo, [`HALO`]). Always a blemish, whatever is around it.
//! - **Freckle**: in a field of at least [`FRECKLED`] (a few brown spots in a group are an older face's age spots,
//!   which go), one of a field of like spots ([`FIELD`] or more within [`FIELD_NEAR`] of the face, of about its
//!   size and darkness and not red), in a field of spots large enough to be freckles ([`FRECKLE_MIN`]): skin
//!   texture and pores make fields of smaller dots.
//! - **Mole**: round, sharp-edged (its darkness stops at its edge, [`MOLE_RIM`]), large enough to see
//!   ([`MOLE_MIN`]) and much darker than the spots around it ([`MOLE_STANDS_OUT`] times their median, at least
//!   [`MOLE_DARK`]): on any skin tone a mole is the darkest thing near it, a mark or an age spot is not.
//! - Anything else is a **blemish**: a dark mark, a scab, an age spot, a spot a pimple left.
//! - **Not a spot**: where stubble grows ([`Beard`]), a field of dots smaller than freckles is stubble or pores, and so is
//!   a small spot much greyer than the face's other spots (a hair stump: red darkens nearly as much as the rest,
//!   [`GREYER`]), and so is any spot on textured skin there ([`BEARD_BUSY`]); only red spots, and spots of freckle
//!   size or more on smooth skin, are taken there.
//!
//! Colour is read as how the spot darkens each channel against the skin around it, so the same rules hold on dark
//! and light skin: no rule asks how brown a spot is, which on dark or tanned skin every spot is.

use super::Found;
use super::traits::Traits;
use crate::input::SpotKind;

/// Blood: blue darkens at most this share of green (melanin darkens blue more than green) …
pub const BLOOD_BLUE: f32 = 0.9;
/// … and red at most this share (a grey or blue-black mole darkens all three).
pub const BLOOD_RED: f32 = 0.6;
/// Colour is read only from a spot whose green darkens at least this much (natural log).
const COLOUR_FROM: f32 = 0.08;
/// An inflamed halo: the skin 1.3–1.9 radii out this much redder (ln R/G) than the skin farther out.
pub const HALO: f32 = 0.05;

/// A field of freckle-like spots is freckles when it has at least this many; a smaller group is age spots or marks.
pub const FRECKLED: usize = 20;
/// A freckle has at least this many like spots …
pub const FIELD: usize = 4;
/// … within this share of the face's size.
const FIELD_NEAR: f64 = 0.09;
/// Like spots: radii within this factor of each other …
const LIKE_SIZE: f64 = 2.2;
/// … and darkenings within this factor.
const LIKE_DARK: f32 = 2.5;
/// A field of spots is a freckle field when its spots' median radius is at least this share of the face (about
/// 1.5 mm across on an adult face): smaller dots in fields are pores, skin texture or stubble.
pub const FRECKLE_MIN: f64 = 0.0026;

/// A spot is greyer than the face's others when red's share of its darkening is past their median by this share
/// of the way to grey (1).
pub const GREYER: f32 = 0.5;

/// Where stubble grows, skin at least this textured around a spot (the finder's texture measure) is a beard or stubble.
pub const BEARD_BUSY: f32 = 0.05;

/// A mole's radius is at least this share of the face …
pub const MOLE_MIN: f64 = 0.0022;
/// … it is darker than the skin around it by at least this (natural log) …
pub const MOLE_DARK: f32 = 0.22;
/// … and by this many times the median of the spots within [`MOLE_NEAR`] of it (or of the face's spots) …
pub const MOLE_STANDS_OUT: f32 = 1.7;
const MOLE_NEAR: f64 = 0.15;
/// … its darkness stops at its edge: at 1.5 radii at most this share of its centre's …
pub const MOLE_RIM: f32 = 0.3;
/// … and it is round: at most this elongation, or [`MOLE_ROUND_DARK`] for one twice [`MOLE_DARK`] (a dark mole
/// beside stubble or a fold reads stretched).
const MOLE_ROUND: f32 = 1.9;
const MOLE_ROUND_DARK: f32 = 2.8;

/// The part of the face stubble grows on: below the mouth across the face's whole width (chin and jaw), and
/// between the nose tip and the mouth within [`LIP_WIDE`] of the middle (the upper lip); the cheeks beside the
/// nose are not in it. In the face's own frame, so a tilted face is measured along its tilt. Work pixels.
pub(super) struct Beard {
    /// The eyes' midpoint, the unit vector from it to the mouth's, and that distance.
    eyes: (f64, f64),
    down: (f64, f64),
    reach: f64,
    /// The nose tip's place along `down`, a share of `reach`.
    nose: f64,
    /// Half the mouth's width, in work pixels.
    mouth: f64,
}

/// The upper lip's stubble reaches this many half mouth widths from the face's middle.
const LIP_WIDE: f64 = 1.3;
/// Below the mouth starts this share of the eyes-to-mouth distance above the mouth corners (the lower lip).
const BELOW_MOUTH: f64 = 0.1;

impl Beard {
    /// From the face's five landmarks (eyes, nose tip, mouth corners) in work pixels; `None` without them or when
    /// they make no face.
    pub(super) fn new(eyes: [(f64, f64); 2], nose: (f64, f64), mouth: [(f64, f64); 2]) -> Option<Beard> {
        let e = ((eyes[0].0 + eyes[1].0) / 2.0, (eyes[0].1 + eyes[1].1) / 2.0);
        let m = ((mouth[0].0 + mouth[1].0) / 2.0, (mouth[0].1 + mouth[1].1) / 2.0);
        let reach = ((m.0 - e.0).powi(2) + (m.1 - e.1).powi(2)).sqrt();
        if !(reach.is_finite() && reach > 1.0) {
            return None;
        }
        let down = ((m.0 - e.0) / reach, (m.1 - e.1) / reach);
        let nose = ((nose.0 - e.0) * down.0 + (nose.1 - e.1) * down.1) / reach;
        let mouth = ((mouth[1].0 - mouth[0].0).powi(2) + (mouth[1].1 - mouth[0].1).powi(2)).sqrt() / 2.0;
        (nose.is_finite() && nose > 0.2 && nose < 0.95 && mouth.is_finite()).then_some(Beard { eyes: e, down, reach, nose, mouth })
    }

    /// Whether work pixel (`x`, `y`) is where stubble grows.
    pub(super) fn holds(&self, x: f64, y: f64) -> bool {
        let (dx, dy) = (x - self.eyes.0, y - self.eyes.1);
        let along = (dx * self.down.0 + dy * self.down.1) / self.reach;
        let across = (dx * self.down.1 - dy * self.down.0).abs();
        along >= 1.0 - BELOW_MOUTH || (along > self.nose && across <= LIP_WIDE * self.mouth)
    }
}

/// Whether `f` lies in a freckle field: at least two of `freckles` within [`FIELD_NEAR`] of it.
pub(super) fn in_field(f: &Found, freckles: &[Found], size: f64) -> bool {
    freckles.iter().filter(|o| dist(f, o) <= FIELD_NEAR * size).nth(1).is_some()
}

/// Whether a spot shows blood: a pimple, a red mark.
pub(super) fn is_red(t: &Traits) -> bool {
    // (a faint spot's colour is mostly noise: its ratios say nothing)
    t.d[1] >= COLOUR_FROM && ((t.d[2] < BLOOD_BLUE * t.d[1] && t.d[0] < BLOOD_RED * t.d[1]) || t.halo >= HALO)
}

fn dist(a: &Found, b: &Found) -> f64 {
    ((a.x as f64 - b.x as f64).powi(2) + (a.y as f64 - b.y as f64).powi(2)).sqrt()
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    Some(*v.select_nth_unstable_by(mid, f32::total_cmp).1)
}

/// The kind of each spot of `spots` (with their traits), on a face `size` work pixels across; `None` for stubble
/// or pores ([`Beard`]).
pub(super) fn kinds(spots: &[(Found, Traits)], size: f64, beard: Option<&Beard>) -> Vec<Option<SpotKind>> {
    let red: Vec<bool> = spots.iter().map(|(_, t)| is_red(t)).collect();
    let face_median = median(spots.iter().map(|(f, _)| f.contrast).collect()).unwrap_or(0.0);
    let red_median = median(spots.iter().map(|(f, _)| f.red_share).collect()).unwrap_or(1.0);
    let grey = red_median + GREYER * (1.0 - red_median).max(0.0);
    let like = |a: &Found, b: &Found| {
        let size_ok = a.radius <= b.radius * LIKE_SIZE && b.radius <= a.radius * LIKE_SIZE;
        let dark_ok = a.contrast <= b.contrast * LIKE_DARK && b.contrast <= a.contrast * LIKE_DARK;
        size_ok && dark_ok
    };
    let mut kinds: Vec<Option<SpotKind>> = spots
        .iter()
        .enumerate()
        .map(|(i, (f, t))| {
            if red.get(i).copied().unwrap_or(false) {
                return Some(SpotKind::Blemish);
            }
            let others = || spots.iter().enumerate().filter(move |(j, _)| *j != i);
            // a mole: much darker than what is around it, round and sharp
            let near: Vec<f32> = others().filter(|(_, (o, _))| dist(f, o) <= MOLE_NEAR * size).map(|(_, (o, _))| o.contrast).collect();
            let around = if near.len() >= 3 { median(near).unwrap_or(face_median) } else { face_median };
            let stands_out = f.contrast >= MOLE_DARK && f.contrast >= MOLE_STANDS_OUT * around;
            let round = if f.contrast >= 2.0 * MOLE_DARK { MOLE_ROUND_DARK } else { MOLE_ROUND };
            let mole_shape = f.radius >= MOLE_MIN * size && f.elongation <= round && t.rim <= MOLE_RIM;
            if stands_out && mole_shape {
                return Some(SpotKind::Mole);
            }
            // stubble and pores where stubble grows (a mole there was taken above)
            let small_below_nose = f.radius < FRECKLE_MIN * size && beard.is_some_and(|b| b.holds(f.x as f64 + 0.5, f.y as f64 + 0.5));
            if small_below_nose && f.red_share >= grey {
                return None;
            }
            // where stubble grows, textured skin is a beard or stubble
            if f.busy >= BEARD_BUSY && beard.is_some_and(|b| b.holds(f.x as f64 + 0.5, f.y as f64 + 0.5)) {
                return None;
            }
            // a freckle: one of a field of like spots, large enough to be freckles
            let field: Vec<f64> = others()
                .filter(|(j, (o, _))| !red.get(*j).copied().unwrap_or(false) && dist(f, o) <= FIELD_NEAR * size && like(f, o))
                .map(|(_, (o, _))| o.radius)
                .collect();
            if field.len() >= FIELD {
                let radii: Vec<f32> = field.iter().chain(std::iter::once(&f.radius)).map(|r| (r / size) as f32).collect();
                if median(radii).is_some_and(|r| f64::from(r) >= FRECKLE_MIN) {
                    return Some(SpotKind::Freckle);
                }
                // smaller dots in a field where stubble grows: stubble or pores
                if small_below_nose {
                    return None;
                }
            }
            Some(SpotKind::Blemish)
        })
        .collect();
    // a field is the freckle-like spots linked to each other within FIELD_NEAR; only a field of FRECKLED or more is
    // freckles (a few brown spots in a group are an older face's age spots)
    let like_freckles: Vec<usize> = kinds.iter().enumerate().filter(|(_, k)| **k == Some(SpotKind::Freckle)).map(|(i, _)| i).collect();
    let field_of = fields(&like_freckles, spots, size);
    let mut members = vec![0usize; like_freckles.len()];
    for f in &field_of {
        if let Some(m) = members.get_mut(*f) {
            *m += 1;
        }
    }
    let mut freckles: Vec<Found> = Vec::new();
    for (n, i) in like_freckles.iter().enumerate() {
        let big = field_of.get(n).and_then(|f| members.get(*f)).is_some_and(|m| *m >= FRECKLED);
        match (big, kinds.get_mut(*i), spots.get(*i)) {
            (true, _, Some((f, _))) => freckles.push(*f),
            (false, Some(k), _) => *k = Some(SpotKind::Blemish),
            _ => {}
        }
    }
    if freckles.is_empty() {
        return kinds;
    }
    // in a freckle field, a spot that is not red, not a mole and not larger than the freckles is one of them (as
    // the second, finer search takes it)
    let largest = freckles.iter().map(|f| f.radius).fold(0.0f64, f64::max);
    for (((f, _), k), red) in spots.iter().zip(&mut kinds).zip(&red) {
        if *k == Some(SpotKind::Blemish) && !red && f.radius <= largest && in_field(f, &freckles, size) {
            *k = Some(SpotKind::Freckle);
        }
    }
    kinds
}

/// For each of `which` (indices into `spots`), the field it belongs to: spots within [`FIELD_NEAR`] of each other
/// are in one field (and so on, link by link).
fn fields(which: &[usize], spots: &[(Found, Traits)], size: f64) -> Vec<usize> {
    let mut field: Vec<usize> = (0..which.len()).collect();
    fn root(field: &mut [usize], mut i: usize) -> usize {
        // (each step moves to a smaller index, so this ends)
        while let Some(&p) = field.get(i) {
            if p == i {
                break;
            }
            i = p;
        }
        i
    }
    for a in 0..which.len() {
        for b in a + 1..which.len() {
            let (Some((fa, _)), Some((fb, _))) = (which.get(a).and_then(|i| spots.get(*i)), which.get(b).and_then(|i| spots.get(*i))) else {
                continue;
            };
            if dist(fa, fb) <= FIELD_NEAR * size {
                let (ra, rb) = (root(&mut field, a), root(&mut field, b));
                let (lo, hi) = (ra.min(rb), ra.max(rb));
                if let Some(p) = field.get_mut(hi) {
                    *p = lo;
                }
            }
        }
    }
    (0..which.len()).map(|i| root(&mut field, i)).collect()
}
