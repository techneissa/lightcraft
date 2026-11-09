//! Which face's own settings ([`crate::FaceOverride`]) go with which detected face.
//!
//! Rules (they never depend on the order the detector lists faces in):
//! 1. A named face takes the settings made for its person. Among several faces of one name, or several settings
//!    of one name, the closest boxes pair up.
//! 2. Every other face takes the unclaimed settings whose box centre is closest to its own, when its centre lies
//!    within half the settings' box (width and height) of the settings' centre. Boxes are compared on the photo
//!    before the user's rotation ([`Face::key_bounds`]), in one frame: a key and a face of different frames
//!    ([`KeyFrame`]: after or before lens corrections and transform) are compared once the photo's [`KeyMap`] has
//!    taken the one after them to before them; without a map they never pair by place.
//! 3. Settings made for a person never go to a face named as someone else: re-detecting faces never moves them to
//!    another person. They may go to an unnamed face at their place (the same face, not recognised this time).

use crate::input::{Face, KeyFrame, KeyMap, NormRect, edit_box_to_photo};
use crate::{FaceKey, Retouch};

/// For each face of `faces`, the index into `r.faces` of the settings it takes, or `None` (it takes the global
/// ones). Each override goes to at most one face. Keys and faces of different frames pair by person only.
pub fn resolve(r: &Retouch, faces: &[Face]) -> Vec<Option<usize>> {
    resolve_in(r, faces, None)
}

/// The face's box and the key's box in one frame: as they are when their frames agree, else the one after lens
/// corrections and transform taken before them by `map`. `None`: they can't be compared.
fn same_frame(f: &Face, k: &FaceKey, map: Option<&dyn KeyMap>) -> Option<(NormRect, NormRect)> {
    let fb = f.key_bounds();
    match (f.key_frame, k.frame) {
        (a, b) if a == b => Some((fb, k.bounds)),
        (KeyFrame::Photo, _) => Some((fb, edit_box_to_photo(map?, &k.bounds)?)),
        (_, _) => Some((edit_box_to_photo(map?, &fb)?, k.bounds)),
    }
}

/// [`resolve`], with `map` (the photo's lens corrections and transform) to compare keys and faces of different
/// frames by place.
pub fn resolve_in(r: &Retouch, faces: &[Face], map: Option<&dyn KeyMap>) -> Vec<Option<usize>> {
    let mut taken: Vec<Option<usize>> = vec![None; faces.len()];
    if r.faces.is_empty() || faces.is_empty() {
        return taken;
    }
    let mut claimed = vec![false; r.faces.len()];
    // 1. by person
    let mut pairs: Vec<Pair> = Vec::new();
    for (fi, f) in faces.iter().enumerate() {
        let Some(name) = f.person.as_deref() else { continue };
        for (oi, o) in r.faces.iter().enumerate() {
            if o.face.person.as_deref() == Some(name) {
                let d = same_frame(f, &o.face, map).map_or(f64::INFINITY, |(fb, kb)| distance(&fb, &kb));
                pairs.push((d, f.key_bounds().centre(), fi, oi));
            }
        }
    }
    assign(&mut pairs, &mut taken, &mut claimed);
    // 2. by place
    let mut pairs: Vec<Pair> = Vec::new();
    for (fi, f) in faces.iter().enumerate() {
        if taken.get(fi).copied().flatten().is_some() {
            continue;
        }
        for (oi, o) in r.faces.iter().enumerate() {
            if claimed.get(oi).copied().unwrap_or(true) {
                continue;
            }
            // 3. never another person's
            if let (Some(a), Some(b)) = (f.person.as_deref(), o.face.person.as_deref())
                && a != b
            {
                continue;
            }
            if let Some((fb, kb)) = same_frame(f, &o.face, map)
                && within(&fb, &kb)
            {
                pairs.push((distance(&fb, &kb), f.key_bounds().centre(), fi, oi));
            }
        }
    }
    assign(&mut pairs, &mut taken, &mut claimed);
    taken
}

/// (distance, the face's centre, face index, override index).
type Pair = (f64, (f64, f64), usize, usize);

/// Pair faces and overrides closest first, each at most once. Ties break on the override and then on the face's
/// place in the photo, never on where the detector listed it.
fn assign(pairs: &mut [Pair], taken: &mut [Option<usize>], claimed: &mut [bool]) {
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.3.cmp(&b.3)).then(a.1.0.total_cmp(&b.1.0)).then(a.1.1.total_cmp(&b.1.1)));
    for &(_, _, fi, oi) in pairs.iter() {
        let (Some(slot), Some(c)) = (taken.get(fi).copied(), claimed.get(oi).copied()) else { continue };
        if slot.is_none() && !c {
            if let Some(t) = taken.get_mut(fi) {
                *t = Some(oi);
            }
            if let Some(c) = claimed.get_mut(oi) {
                *c = true;
            }
        }
    }
}

/// Distance between two boxes' centres, in units of the second box's size (∞ for a box that is not one).
fn distance(face: &NormRect, key: &NormRect) -> f64 {
    let ((fx, fy), (kx, ky)) = (face.centre(), key.centre());
    let (w, h) = (key.width().abs().max(1e-9), key.height().abs().max(1e-9));
    let d = (((fx - kx) / w).powi(2) + ((fy - ky) / h).powi(2)).sqrt();
    if d.is_finite() { d } else { f64::INFINITY }
}

/// The face's centre lies within half the key's box of the key's centre.
fn within(face: &NormRect, key: &NormRect) -> bool {
    let ((fx, fy), (kx, ky)) = (face.centre(), key.centre());
    face.is_valid() && key.is_valid() && (fx - kx).abs() <= key.width() / 2.0 && (fy - ky).abs() <= key.height() / 2.0
}
