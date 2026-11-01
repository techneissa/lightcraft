//! People as candidates: from SAM 3's "person" mask (one candidate per connected blob) or, without the model, from a
//! rough body shape under each face the face detector found.
//!
//! The main subject is marked: the person with the largest share of the photo weighted by how central they are, when
//! they take at least [`MAIN_MIN_AREA`] of it (a small figure in a landscape is a bystander).

use super::shape::{Component, Mask, components, dilate, simplify, trace_outline};
use super::{Candidate, Kind, cover};
use crate::blemish::FaceBox;

/// Long edge of the grid the masks are worked on.
const WORK: usize = 480;
/// Blobs smaller than this share of the photo are noise, not people.
const MIN_AREA: f64 = 0.0003;
/// Blobs closer than this (a share of the long edge) are one person.
const JOIN: f64 = 0.006;
/// Most people proposed.
pub const MAX_PEOPLE: usize = 24;
/// A main subject takes at least this share of the photo.
pub const MAIN_MIN_AREA: f64 = 0.01;

/// The working grid for a photo of `w × h`: `None` for a photo with no size.
fn grid(w: usize, h: usize) -> Option<(usize, usize)> {
    if w == 0 || h == 0 {
        return None;
    }
    let s = WORK as f64 / w.max(h) as f64;
    Some((((w as f64 * s).round() as usize).max(1), ((h as f64 * s).round() as usize).max(1)))
}

/// Candidates from one mask on the working grid.
fn from_mask(kind: Kind, mask: &Mask, source: &'static str, score: impl Fn(&super::shape::Component) -> f32) -> Vec<Candidate> {
    let (w, h) = (mask.w, mask.h);
    let long = w.max(h) as f64;
    // (a wire is a few pixels wide: what counts is its length, so it is held to a pixel count)
    let min_px = if kind == Kind::Wire { 24 } else { ((MIN_AREA * (w * h) as f64) as usize).max(12) };
    // a head, an arm or a bag cut off by a thin gap still belongs to the person: link blobs a few pixels apart, then
    // take back only the mask's own pixels
    let joined = dilate(mask, (JOIN * long) as f32);
    components(&joined, MAX_PEOPLE)
        .into_iter()
        .filter_map(|mut c| {
            c.mask.bits.iter_mut().zip(&mask.bits).for_each(|(b, own)| *b = *b && *own);
            Component::of(c.mask)
        })
        .filter(|c| c.pixels >= min_px)
        .map(|c| {
            let ring = trace_outline(&c.mask);
            let ring = simplify(&ring, 0.003 * long, true);
            let norm = |p: &(f64, f64)| (p.0 / w as f64, p.1 / h as f64);
            Candidate {
                kind,
                source,
                outline: ring.iter().map(norm).collect(),
                closed: true,
                bbox: [
                    c.bbox[0] as f64 / w as f64,
                    c.bbox[1] as f64 / h as f64,
                    (c.bbox[2] + 1) as f64 / w as f64,
                    (c.bbox[3] + 1) as f64 / h as f64,
                ],
                area: c.pixels as f64 / (w * h) as f64,
                score: score(&c),
                main: false,
                spots: cover::cover_mask(&c.mask),
            }
        })
        .collect()
}

/// People from a SAM 3 mask: `side × side` logits stretched over the whole `img_w × img_h` photo; above 0 is person.
pub fn from_logits(side: usize, logits: &[f32], img_w: usize, img_h: usize) -> Vec<Candidate> {
    let mut out = regions_from_logits(Kind::Person, side, logits, img_w, img_h);
    mark_main(&mut out);
    out
}

/// One candidate of `kind` per blob of a SAM 3 mask (the same frame as [`from_logits`]); nothing is marked main. Wires
/// from SAM 3's "power line" mask come this way: crossing wires are one blob, so one candidate.
pub fn regions_from_logits(kind: Kind, side: usize, logits: &[f32], img_w: usize, img_h: usize) -> Vec<Candidate> {
    let Some((w, h)) = grid(img_w, img_h) else { return Vec::new() };
    if side < 2 || logits.len() < side * side {
        return Vec::new();
    }
    // bilinear, so the outline is smooth rather than the model's 288-cell staircase
    let at = |x: usize, y: usize| logits.get(y * side + x).copied().filter(|v| !v.is_nan()).unwrap_or(-10.0);
    let sample = |px: usize, py: usize| {
        let fx = ((px as f32 + 0.5) / w as f32 * side as f32 - 0.5).clamp(0.0, side as f32 - 1.0);
        let fy = ((py as f32 + 0.5) / h as f32 * side as f32 - 0.5).clamp(0.0, side as f32 - 1.0);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(side - 1), (y0 + 1).min(side - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let values: Vec<f32> = (0..w * h).map(|i| sample(i % w, i / w)).collect();
    let mask = Mask { w, h, bits: values.iter().map(|v| *v > 0.0).collect() };
    from_mask(kind, &mask, "sam3", |c| {
        let mut sum = 0.0f32;
        let mut n = 0usize;
        for (i, v) in values.iter().enumerate() {
            if c.mask.bits.get(i).copied().unwrap_or(false) {
                sum += 1.0 / (1.0 + (-v).exp());
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { sum / n as f32 }
    })
}

/// A rough body (head, shoulders, torso, legs) under each face, in the same frame as `from_logits`. The detector sees
/// faces only, so this is an estimate: candidates made this way say `faces` as their source.
pub fn from_faces(faces: &[FaceBox], img_w: usize, img_h: usize) -> Vec<Candidate> {
    let Some((w, h)) = grid(img_w, img_h) else { return Vec::new() };
    let mut mask = Mask::new(w, h);
    if mask.bits.len() != w * h {
        return Vec::new();
    }
    for f in faces.iter().take(MAX_PEOPLE) {
        let (fw, fh) = ((f.x1 - f.x0) * w as f64, (f.y1 - f.y0) * h as f64);
        if !(fw.is_finite() && fh.is_finite()) || fw < 2.0 || fh < 2.0 {
            continue;
        }
        let (cx, cy) = ((f.x0 + f.x1) / 2.0 * w as f64, (f.y0 + f.y1) / 2.0 * h as f64);
        let body = [
            (cx - 1.3 * fw, cy + 0.55 * fh),
            (cx + 1.3 * fw, cy + 0.55 * fh),
            (cx + 1.1 * fw, cy + 3.5 * fh),
            (cx + 0.75 * fw, cy + 8.5 * fh),
            (cx - 0.75 * fw, cy + 8.5 * fh),
            (cx - 1.1 * fw, cy + 3.5 * fh),
        ];
        let (x0, x1) = (((cx - 1.4 * fw).floor().max(0.0)) as usize, ((cx + 1.4 * fw).ceil().max(0.0) as usize).min(w));
        let (y0, y1) = (((cy - fh).floor().max(0.0)) as usize, ((cy + 8.6 * fh).ceil().max(0.0) as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let head = ((px - cx) / (0.75 * fw)).powi(2) + ((py - cy + 0.1 * fh) / (0.9 * fh)).powi(2) <= 1.0;
                if head || super::shape::in_polygon(&body, px, py) {
                    mask.bits[y * w + x] = true;
                }
            }
        }
    }
    let mut out = from_mask(Kind::Person, &mask, "faces", |_| 0.3);
    mark_main(&mut out);
    out
}

/// Marks the main subject: the largest share of the photo weighted by closeness to its centre, if it takes at least
/// [`MAIN_MIN_AREA`]. Clears any earlier mark.
pub fn mark_main(c: &mut [Candidate]) {
    for p in c.iter_mut() {
        p.main = false;
    }
    let weight = |p: &Candidate| {
        let (cx, cy) = ((p.bbox[0] + p.bbox[2]) / 2.0, (p.bbox[1] + p.bbox[3]) / 2.0);
        let d = ((cx - 0.5).powi(2) + (cy - 0.5).powi(2)).sqrt() / std::f64::consts::FRAC_1_SQRT_2;
        p.area * (1.0 - 0.8 * d.clamp(0.0, 1.0))
    };
    let best = c.iter().enumerate().max_by(|a, b| weight(a.1).total_cmp(&weight(b.1))).map(|(i, _)| i);
    if let Some(i) = best
        && c[i].area >= MAIN_MIN_AREA
    {
        c[i].main = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Logits (side 288) with positive blobs where `blob(u, v)` (0..1) says so.
    fn logits(blob: impl Fn(f64, f64) -> bool) -> Vec<f32> {
        (0..288 * 288).map(|i| if blob(((i % 288) as f64 + 0.5) / 288.0, ((i / 288) as f64 + 0.5) / 288.0) { 6.0 } else { -6.0 }).collect()
    }

    #[test]
    fn two_people_one_main() {
        // a big person in the middle and a small one at the right, as masks over a 3:2 photo
        let l = logits(|u, v| ((u - 0.45).abs() < 0.08 && (0.25..0.85).contains(&v)) || ((u - 0.85).abs() < 0.02 && (0.55..0.7).contains(&v)));
        let c = from_logits(288, &l, 1500, 1000);
        assert_eq!(c.len(), 2, "{c:?}");
        let (main, other) = if c[0].main { (&c[0], &c[1]) } else { (&c[1], &c[0]) };
        assert!(main.main && !other.main);
        assert!(main.area > other.area * 4.0 && main.bbox[0] < 0.5 && other.bbox[0] > 0.8);
        assert!(c.iter().all(|p| p.kind == Kind::Person && p.source == "sam3" && p.closed && !p.spots.is_empty() && p.score > 0.9));
        assert!(main.hit(0.45, 0.5, 0.0) && !main.hit(0.1, 0.1, 0.0));
        assert!(other.outline.iter().all(|p| (0.0..=1.0).contains(&p.0) && (0.0..=1.0).contains(&p.1)));
    }

    #[test]
    fn a_small_figure_alone_is_a_bystander() {
        let l = logits(|u, v| (u - 0.3).abs() < 0.01 && (0.5..0.6).contains(&v));
        let c = from_logits(288, &l, 1500, 1000);
        assert_eq!(c.len(), 1);
        assert!(!c[0].main, "area {}", c[0].area);
    }

    #[test]
    fn no_people_and_hostile_input() {
        assert!(from_logits(288, &vec![-5.0; 288 * 288], 1500, 1000).is_empty());
        assert!(from_logits(288, &[], 1500, 1000).is_empty());
        assert!(from_logits(1, &[1.0], 1500, 1000).is_empty());
        assert!(from_logits(288, &vec![1.0; 288 * 288], 0, 1000).is_empty());
        assert!(from_logits(288, &vec![f32::NAN; 288 * 288], 3, 3).is_empty());
        // a one-pixel photo and a very thin one
        let _ = from_logits(288, &vec![1.0; 288 * 288], 1, 1);
        let _ = from_logits(288, &vec![1.0; 288 * 288], 100_000, 3);
        // everything is a person: one candidate covering the photo, still bounded
        let all = from_logits(288, &vec![4.0; 288 * 288], 1500, 1000);
        assert_eq!(all.len(), 1);
        assert!(all[0].area > 0.99 && all[0].spots.iter().map(|s| s.points.len()).sum::<usize>() <= cover::MAX_DABS);
    }

    #[test]
    fn faces_give_rough_bodies() {
        let face = |cx: f64, cy: f64, s: f64| FaceBox { x0: cx - s, y0: cy - s * 1.2, x1: cx + s, y1: cy + s * 1.2, landmarks: [(cx, cy); 5] };
        let c = from_faces(&[face(0.3, 0.2, 0.03), face(0.7, 0.25, 0.012)], 1500, 1000);
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c.iter().all(|p| p.source == "faces" && !p.spots.is_empty()));
        assert!(c.iter().filter(|p| p.main).count() <= 1);
        // a face whose box is nonsense is skipped
        let bad = FaceBox { x0: f64::NAN, y0: 0.0, x1: 1.0, y1: 1.0, landmarks: [(0.0, 0.0); 5] };
        assert!(from_faces(&[bad], 100, 100).is_empty());
        assert!(from_faces(&[], 100, 100).is_empty());
        assert!(from_faces(&[face(0.5, 0.5, 0.05)], 0, 0).is_empty());
    }
}
