//! Where a face's lenses are, and what of them to repaint: pure geometry on the square crop around the glasses.
//!
//! The people parser marks glasses as part of the person's Others mask. Near the eyes, that mask filled (its holes
//! closed) is the glasses; shrunk past the frame (about 12 px of 768 for metal rims, 22 px for plastic ones, measured
//! from how dark the rim is against the lens) it is the lens interior, so the frame stays as it is. Each lens is then
//! looked at alone: where glare covers enough of it to hide the eye, the whole interior is repainted (FLUX.1 Fill);
//! elsewhere only the glints, which LaMa fills too. Glare is what the Highlights finder sees (bright, colourless spots)
//! and, as in the trials, a veil: lens pixels brighter and paler than the skin around the glasses, far paler and not
//! darker, much brighter than their own neighbourhood, or of a hue far from the skin's (a window, a screen).

use lightcraft_develop::retouch::{Face, NormRect, Part};
use lightcraft_raster::{Plane, Rgb32f};

use crate::reflections::glare;

/// The side the crop is worked at (the trials' FLUX size).
pub const SIDE: usize = 768;
/// Share of a lens glare must cover for the lens to be repainted whole.
pub const HIDES_EYE: f32 = 0.15;
/// Crop side in eye distances.
const CROP_EYES: f64 = 2.6;
/// The glasses are looked for this far around the eyes (eye distances across, and up and down).
const LOOK_WIDE: f64 = 1.35;
const LOOK_TALL: f64 = 0.75;

/// The square crop around `face`'s glasses, in normalised photo coordinates of a `w × h` photo (the frame of the
/// face's masks), and the eye distance in crop pixels at [`SIDE`]. `None` without two eyes or an Others mask.
pub fn crop_area(face: &Face, w: usize, h: usize) -> Option<(NormRect, f64)> {
    face.mask(Part::Others)?;
    let (wf, hf) = (w as f64, h as f64);
    if w == 0 || h == 0 {
        return None;
    }
    let (r, l) = (face.landmark(0)?, face.landmark(1)?);
    let (rx, ry, lx, ly) = (r.0 * wf, r.1 * hf, l.0 * wf, l.1 * hf);
    let d = ((rx - lx).powi(2) + (ry - ly).powi(2)).sqrt();
    if !d.is_finite() || d < 4.0 {
        return None;
    }
    let (cx, cy) = ((rx + lx) / 2.0, (ry + ly) / 2.0);
    let half = CROP_EYES * d / 2.0;
    let area = NormRect::new((cx - half) / wf, (cy - half) / hf, (cx + half) / wf, (cy + half) / hf);
    area.is_valid().then_some((area, d * SIDE as f64 / (2.0 * half)))
}

/// What the parser marks as glasses on the crop: the face's Others mask sampled at `n × n` over `area`, kept near the
/// eyes (the crop's middle band; a hat or a necklace is Others too).
pub fn glasses(face: &Face, area: &NormRect, n: usize, eye_px: f64) -> Vec<bool> {
    let Some(m) = face.mask(Part::Others) else { return vec![false; n * n] };
    let c = n as f64 / 2.0;
    let (hw, hh) = (LOOK_WIDE * eye_px, LOOK_TALL * eye_px);
    (0..n * n)
        .map(|i| {
            let (x, y) = ((i % n) as f64 + 0.5, (i / n) as f64 + 0.5);
            if (x - c).abs() > hw || (y - c).abs() > hh {
                return false;
            }
            let (nx, ny) = (area.x0 + x / n as f64 * area.width(), area.y0 + y / n as f64 * area.height());
            m.sample(nx, ny) > 0.5
        })
        .collect()
}

/// Distance (in pixels, chamfer 3-4) from each pixel to the nearest pixel where `set` is `want`, capped at `cap`.
fn distance(set: &[bool], n: usize, want: bool, cap: f32) -> Vec<f32> {
    let mut d: Vec<f32> = set.iter().map(|v| if *v == want { 0.0 } else { cap }).collect();
    let at = |d: &[f32], x: isize, y: isize| -> f32 {
        if x < 0 || y < 0 || x as usize >= n || y as usize >= n { cap } else { d.get(y as usize * n + x as usize).copied().unwrap_or(cap) }
    };
    for y in 0..n as isize {
        for x in 0..n as isize {
            let v = at(&d, x, y)
                .min(at(&d, x - 1, y) + 1.0)
                .min(at(&d, x, y - 1) + 1.0)
                .min(at(&d, x - 1, y - 1) + 1.414)
                .min(at(&d, x + 1, y - 1) + 1.414);
            if let Some(slot) = d.get_mut(y as usize * n + x as usize) {
                *slot = v.min(cap);
            }
        }
    }
    for y in (0..n as isize).rev() {
        for x in (0..n as isize).rev() {
            let v = at(&d, x, y)
                .min(at(&d, x + 1, y) + 1.0)
                .min(at(&d, x, y + 1) + 1.0)
                .min(at(&d, x + 1, y + 1) + 1.414)
                .min(at(&d, x - 1, y + 1) + 1.414);
            if let Some(slot) = d.get_mut(y as usize * n + x as usize) {
                *slot = v.min(cap);
            }
        }
    }
    d
}

/// `set` grown by `r` pixels (a disc, near enough).
pub fn dilate(set: &[bool], n: usize, r: f32) -> Vec<bool> {
    if r <= 0.0 {
        return set.to_vec();
    }
    distance(set, n, true, r + 2.0).iter().map(|d| *d <= r).collect()
}

/// `set` shrunk by `r` pixels.
pub fn erode(set: &[bool], n: usize, r: f32) -> Vec<bool> {
    if r <= 0.0 {
        return set.to_vec();
    }
    distance(set, n, false, r + 2.0).iter().map(|d| *d > r).collect()
}

/// `set` with its holes filled: whatever the outside (reached from the border through unset pixels) does not reach.
pub fn fill_holes(set: &[bool], n: usize) -> Vec<bool> {
    let mut outside = vec![false; n * n];
    let mut stack: Vec<usize> =
        (0..n).flat_map(|i| [i, (n - 1) * n + i, i * n, i * n + n - 1]).filter(|i| !set.get(*i).copied().unwrap_or(true)).collect();
    while let Some(i) = stack.pop() {
        if outside.get(i).copied().unwrap_or(true) || set.get(i).copied().unwrap_or(true) {
            continue;
        }
        if let Some(o) = outside.get_mut(i) {
            *o = true;
        }
        let (x, y) = (i % n, i / n);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < n {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - n);
        }
        if y + 1 < n {
            stack.push(i + n);
        }
    }
    outside.iter().map(|o| !o).collect()
}

/// The 4-connected pieces of `set`, as pixel lists, largest first.
pub fn pieces(set: &[bool], n: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; n * n];
    let mut out = Vec::new();
    for start in 0..n * n {
        if !set.get(start).copied().unwrap_or(false) || seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut piece = Vec::new();
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            if !set.get(i).copied().unwrap_or(false) || seen.get(i).copied().unwrap_or(true) {
                continue;
            }
            if let Some(s) = seen.get_mut(i) {
                *s = true;
            }
            piece.push(i);
            let (x, y) = (i % n, i / n);
            if x > 0 {
                stack.push(i - 1);
            }
            if x + 1 < n {
                stack.push(i + 1);
            }
            if y > 0 {
                stack.push(i - n);
            }
            if y + 1 < n {
                stack.push(i + n);
            }
        }
        out.push(piece);
    }
    out.sort_by_key(|p| std::cmp::Reverse(p.len()));
    out
}

fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    v.select_nth_unstable_by(mid, f32::total_cmp);
    v.get(mid).copied()
}

/// How far in from the glasses' outline the lenses start: 12 px of 768 for a thin rim, 22 for a thick dark one (its
/// outline much darker than the lens), scaled to `n`.
pub fn frame_width(filled: &[bool], img: &Rgb32f, n: usize) -> f32 {
    let k = n as f32 / SIDE as f32;
    let edge = erode(filled, n, 4.0 * k);
    let core = erode(filled, n, 40.0 * k);
    let lum = |keep: &dyn Fn(usize) -> bool| median((0..n * n).filter(|i| keep(*i)).filter_map(|i| img.data.get(i).map(|p| luma(*p))).collect());
    let rim = lum(&|i| filled.get(i).copied().unwrap_or(false) && !edge.get(i).copied().unwrap_or(true));
    let inside = lum(&|i| core.get(i).copied().unwrap_or(false));
    let thick = matches!((rim, inside), (Some(r), Some(c)) if r < 0.5 * c);
    if thick { 22.0 * k } else { 12.0 * k }
}

/// HSV of a display-encoded pixel: hue (0…1), saturation, value.
fn hsv(p: [f32; 3]) -> (f32, f32, f32) {
    let (max, min) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
    let c = max - min;
    let s = if max > 0.0 { c / max } else { 0.0 };
    let h = if c <= 0.0 {
        0.0
    } else if max == p[0] {
        ((p[1] - p[2]) / c).rem_euclid(6.0) / 6.0
    } else if max == p[1] {
        ((p[2] - p[0]) / c + 2.0) / 6.0
    } else {
        ((p[0] - p[1]) / c + 4.0) / 6.0
    };
    (h, s, max)
}

/// The veil on the lenses (the trials' rule, `crate::glare_repair` docs): `srgb` is the display-encoded crop, `filled`
/// the glasses with their holes closed. Thin lines (rims, lashes) are dropped.
pub fn veil(srgb: &Rgb32f, filled: &[bool], n: usize) -> Vec<bool> {
    if srgb.width != n || srgb.height != n || filled.len() != n * n {
        return vec![false; n * n];
    }
    let k = n as f32 / SIDE as f32;
    let px: Vec<(f32, f32, f32)> = srgb.data.iter().map(|p| hsv(*p)).collect();
    // the skin: just outside the glasses
    let ring: Vec<bool> = dilate(filled, n, 24.0 * k).iter().zip(filled).map(|(a, f)| *a && !f).collect();
    let pick = |f: fn(&(f32, f32, f32)) -> f32| median(px.iter().zip(&ring).filter(|(_, r)| **r).map(|(p, _)| f(p)).collect());
    let (Some(hs), Some(ss), Some(vs)) = (pick(|p| p.0), pick(|p| p.1), pick(|p| p.2)) else { return vec![false; n * n] };
    let v_plane = Plane::from_fn(n, n, |x, y| px.get(y * n + x).map_or(0.0, |p| p.2));
    let r = (20.0 * k).round().max(1.0) as usize;
    let local = box_blur(&v_plane, r);
    let hi: Vec<bool> = px
        .iter()
        .enumerate()
        .map(|(i, &(h, s, v))| {
            if !filled.get(i).copied().unwrap_or(false) {
                return false;
            }
            let dh = (h - hs).abs().min(1.0 - (h - hs).abs());
            let lb = local.data.get(i).copied().unwrap_or(v);
            (v > (vs + 0.15).min(0.95) && s < 0.6 * ss + 0.1)
                || v > 0.93
                || (v - lb > 0.10 && s < 0.6 * ss + 0.15)
                || (dh > 0.12 && v > 0.8 * vs && s > 0.12)
                // a pale reflection over the whole lens (papers, a wall): far paler than skin, and not darker
                || (s < 0.5 * ss && v > 0.85 * vs)
        })
        .collect();
    dilate(&erode(&hi, n, 3.0 * k), n, 3.0 * k)
}

/// One lens: its interior, and how much of it glare covers.
pub struct Lens {
    pub interior: Vec<bool>,
    pub glints: Vec<bool>,
    pub covered: f32,
}

impl Lens {
    pub fn hides_eye(&self) -> bool {
        self.covered >= HIDES_EYE
    }
}

/// The lenses of the crop (`img`, linear, `n × n`; `glasses` from [`glasses`]), largest first, at most two, with the
/// glare the Highlights finder sees in each.
pub fn lenses(img: &Rgb32f, srgb: &Rgb32f, glasses: &[bool], n: usize) -> Vec<Lens> {
    if img.width != n || img.height != n || glasses.len() != n * n {
        return Vec::new();
    }
    let k = n as f32 / SIDE as f32;
    let closed = erode(&dilate(glasses, n, 4.0 * k), n, 4.0 * k);
    let filled = fill_holes(&closed, n);
    let interior = erode(&filled, n, frame_width(&filled, img, n));
    let veiled = veil(srgb, &filled, n);
    let min = (n * n) / 200;
    let params = glare::Params { threshold: 50.0, expand: 30.0, photo_long: n as f64, max_spot: 1.0, chosen: true, boxed: false };
    pieces(&interior, n)
        .into_iter()
        .filter(|p| p.len() >= min)
        .take(2)
        .map(|p| {
            let mut inside = vec![false; n * n];
            for i in &p {
                if let Some(v) = inside.get_mut(*i) {
                    *v = true;
                }
            }
            let region = Plane::from_fn(n, n, |x, y| if inside.get(y * n + x).copied().unwrap_or(false) { 1.0 } else { 0.0 });
            let found = glare::find(img, Some(&region), &params, &[]);
            let spots: Vec<bool> = found.data.iter().zip(&veiled).map(|(v, w)| *v > 0.25 || *w).collect();
            let covered = p.iter().filter(|i| spots.get(**i).copied().unwrap_or(false)).count() as f32 / p.len().max(1) as f32;
            let glints: Vec<bool> = dilate(&spots, n, 6.0 * k).iter().zip(&inside).map(|(a, b)| *a && *b).collect();
            Lens { interior: inside, glints, covered }
        })
        .collect()
}

/// Which part of each lens is repainted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lenses {
    /// The whole interior where glare hides the eye ([`HIDES_EYE`]), the glints elsewhere.
    Auto,
    /// Every lens's whole interior (a reflection the finder can't tell from the eye behind it).
    Whole,
    /// The glints alone.
    Glints,
}

impl Lenses {
    pub fn name(self) -> &'static str {
        match self {
            Lenses::Auto => "auto",
            Lenses::Whole => "whole",
            Lenses::Glints => "glints",
        }
    }

    pub fn from_name(s: &str) -> Option<Lenses> {
        [Lenses::Auto, Lenses::Whole, Lenses::Glints].into_iter().find(|l| l.name() == s)
    }
}

/// What to repaint of `lenses` (see [`Lenses`]).
pub fn repaint_mask(lenses: &[Lens], n: usize, which: Lenses) -> Vec<bool> {
    let mut out = vec![false; n * n];
    for l in lenses {
        let whole = match which {
            Lenses::Auto => l.hides_eye(),
            Lenses::Whole => true,
            Lenses::Glints => false,
        };
        let take = if whole { &l.interior } else { &l.glints };
        for (o, t) in out.iter_mut().zip(take) {
            *o |= *t;
        }
    }
    out
}

/// The paste weight of `mask`: grown 4 px and blurred about 4 px (at 1024 px, as the trials composited), 0…1.
pub fn feather(mask: &[bool], n: usize) -> Plane {
    let k = n as f32 / 1024.0;
    let grown = dilate(mask, n, (4.0 * k).max(1.0));
    let mut p = Plane::from_fn(n, n, |x, y| if grown.get(y * n + x).copied().unwrap_or(false) { 1.0 } else { 0.0 });
    let r = (4.0 * k).round().max(1.0) as usize;
    for _ in 0..3 {
        p = box_blur(&p, r);
    }
    // nothing outside what was grown (a blur would spread past it), and the core stays whole
    for (v, g) in p.data.iter_mut().zip(&grown) {
        if !g {
            *v = 0.0;
        }
    }
    for (v, m) in p.data.iter_mut().zip(mask) {
        if *m {
            *v = 1.0;
        }
    }
    p
}

fn box_blur(p: &Plane, r: usize) -> Plane {
    let (w, h) = (p.width, p.height);
    let pass = |src: &Plane, horizontal: bool| {
        Plane::from_fn(w, h, |x, y| {
            let (mut sum, mut cnt) = (0.0f32, 0.0f32);
            let (c, len) = if horizontal { (x, w) } else { (y, h) };
            for j in c.saturating_sub(r)..(c + r + 1).min(len) {
                let (sx, sy) = if horizontal { (j, y) } else { (x, j) };
                sum += src.data.get(sy * w + sx).copied().unwrap_or(0.0);
                cnt += 1.0;
            }
            sum / cnt.max(1.0)
        })
    };
    pass(&pass(p, true), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::retouch::PartMask;

    /// A face with eyes at (0.4, 0.5) and (0.6, 0.5) and an Others mask of two filled ellipses (lenses with rims).
    fn face() -> Face {
        let plane = Plane::from_fn(200, 200, |x, y| {
            let e = |cx: f32| ((x as f32 - cx) / 18.0).powi(2) + ((y as f32 - 100.0) / 12.0).powi(2);
            if e(80.0) <= 1.0 || e(120.0) <= 1.0 { 1.0 } else { 0.0 }
        });
        Face {
            landmarks: vec![(0.4, 0.5), (0.6, 0.5), (0.5, 0.6), (0.42, 0.7), (0.58, 0.7)],
            masks: vec![(Part::Others, PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) })],
            ..Face::boxed(NormRect::new(0.3, 0.3, 0.7, 0.8))
        }
    }

    #[test]
    fn the_crop_is_square_around_the_eyes() {
        let (a, eye) = crop_area(&face(), 1000, 1000).unwrap();
        assert!((a.width() - 0.2 * CROP_EYES).abs() < 1e-9 && (a.height() - a.width()).abs() < 1e-9);
        assert!((a.centre().0 - 0.5).abs() < 1e-9 && (a.centre().1 - 0.5).abs() < 1e-9);
        assert!((eye - SIDE as f64 / CROP_EYES).abs() < 1e-6);
        // a wide photo: square in pixels, so narrower in normalised x
        let (a, _) = crop_area(&face(), 2000, 1000).unwrap();
        assert!((a.width() * 2000.0 - a.height() * 1000.0).abs() < 1e-6);
        // no eyes or no Others mask: no crop
        assert!(crop_area(&Face { landmarks: vec![], ..face() }, 1000, 1000).is_none());
        assert!(crop_area(&Face { masks: vec![], ..face() }, 1000, 1000).is_none());
        assert!(crop_area(&face(), 0, 0).is_none());
    }

    #[test]
    fn morphology_grows_shrinks_and_fills() {
        let n = 21;
        let mut ring = vec![false; n * n];
        for y in 5..16 {
            for x in 5..16 {
                if x == 5 || x == 15 || y == 5 || y == 15 {
                    ring[y * n + x] = true;
                }
            }
        }
        let filled = fill_holes(&ring, n);
        assert_eq!(filled.iter().filter(|v| **v).count(), 121);
        let shrunk = erode(&filled, n, 2.0);
        assert_eq!(shrunk.iter().filter(|v| **v).count(), 49);
        let grown = dilate(&shrunk, n, 2.0);
        assert!(grown.iter().filter(|v| **v).count() >= 100);
        assert_eq!(pieces(&ring, n).len(), 1);
        assert_eq!(pieces(&vec![false; n * n], n).len(), 0);
    }

    /// Two glasses lenses: one under a bright veil (glare hides the eye), one with a small glint.
    fn crop() -> (Rgb32f, Vec<bool>) {
        let n = 192;
        let f = face();
        let (area, eye) = crop_area(&f, 1000, 1000).unwrap();
        let g = glasses(&f, &area, n, eye * n as f64 / SIDE as f64);
        let img = Rgb32f::from_fn(n, n, |x, y| {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            // left lens: centre (0.35, 0.5) mostly white; right lens: centre (0.65, 0.5), a small glint
            let left = ((fx - 0.346) / 0.1).powi(2) + ((fy - 0.5) / 0.07).powi(2) < 1.0;
            let glint = ((fx - 0.66) / 0.012).powi(2) + ((fy - 0.5) / 0.012).powi(2) < 1.0;
            if left || glint { [0.98, 0.98, 0.98] } else { [0.18, 0.12, 0.09] }
        });
        (img, g)
    }

    fn srgb(img: &Rgb32f) -> Rgb32f {
        Rgb32f { width: img.width, height: img.height, data: img.data.iter().map(|p| p.map(lightcraft_color::transfer::linear_to_srgb)).collect() }
    }

    #[test]
    fn a_lens_glare_hides_is_repainted_whole_and_glints_alone_elsewhere() {
        let (img, g) = crop();
        let n = img.width;
        let found = lenses(&img, &srgb(&img), &g, n);
        assert_eq!(found.len(), 2, "two lenses");
        let hidden = found.iter().filter(|l| l.hides_eye()).count();
        assert_eq!(hidden, 1, "coverage {:?}", found.iter().map(|l| l.covered).collect::<Vec<_>>());
        let count = |m: &[bool]| m.iter().filter(|v| **v).count();
        let flux = repaint_mask(&found, n, Lenses::Auto);
        let lama = repaint_mask(&found, n, Lenses::Glints);
        let whole = repaint_mask(&found, n, Lenses::Whole);
        assert!(count(&whole) > count(&flux));
        assert!(count(&flux) > count(&lama), "{} vs {}", count(&flux), count(&lama));
        assert!(count(&lama) > 0);
        // nothing outside the glasses
        let near = dilate(&g, n, 2.0);
        assert!(flux.iter().zip(&near).all(|(m, g)| !m || *g));
        let a = feather(&flux, n);
        assert!(a.data.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(a.data.iter().zip(&flux).all(|(v, m)| !m || *v > 0.99));
    }

    #[test]
    fn hostile_sizes_give_nothing_not_a_panic() {
        let (img, g) = crop();
        assert!(lenses(&img, &srgb(&img), &g[..10], img.width).is_empty());
        assert!(lenses(&Rgb32f::new(0, 0), &Rgb32f::new(0, 0), &[], 0).is_empty());
        assert_eq!(veil(&img, &g[..10], img.width), vec![false; img.width * img.width]);
        assert_eq!(repaint_mask(&[], 4, Lenses::Whole), vec![false; 16]);
        assert_eq!(feather(&[], 0).data.len(), 0);
        assert!(glasses(&Face::default(), &NormRect::new(0.0, 0.0, 1.0, 1.0), 4, 1.0).iter().all(|v| !v));
    }
}
