//! Tests of the skin tone feature (`lightcraft_retouch::skin_tone`): Even Tone, Redness and Face Light.

mod common;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::Feature;
use lightcraft_retouch::skin_tone::{self, FACE_AREA, FACE_LIGHT_STOPS, SkinTone};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

#[test]
fn off_by_default_and_every_slider_at_zero_changes_nothing() {
    common::off_changes_nothing::<SkinTone>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<SkinTone>();
}

/// A disc over the middle of its box with a soft edge (1 inside 0.36 of the box, 0 past 0.44), drawn at 64 × 64.
fn disc() -> Plane {
    Plane::from_fn(64, 64, |x, y| {
        let (dx, dy) = ((x as f32 + 0.5) / 64.0 - 0.5, (y as f32 + 0.5) / 64.0 - 0.5);
        let r = (dx * dx + dy * dy).sqrt();
        ((0.44 - r) / 0.08).clamp(0.0, 1.0)
    })
}

/// A face over a box whose skin is `part`'s disc.
fn face_with(part: Part, x0: f64, y0: f64, x1: f64, y1: f64) -> Face {
    let area = NormRect::new(x0, y0, x1, y1);
    Face { masks: vec![(part, PartMask { plane: disc(), area })], ..Face::boxed(area) }
}

fn skin_face(x0: f64, y0: f64, x1: f64, y1: f64) -> Face {
    face_with(Part::FacialSkin, x0, y0, x1, y1)
}

fn with(even: f64, redness: f64, light: f64) -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.skinTone.evenTone", even));
    assert!(r.global.set("retouch.skinTone.redness", redness));
    assert!(r.global.set("retouch.skinTone.faceLight", light));
    r
}

fn retouched(img: &Rgb32f, r: &Retouch, faces: Vec<Face>) -> Rgb32f {
    let mut out = img.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    lightcraft_retouch::run::apply(&mut out, r, &Input::new(faces), &view, Stage::Retouch);
    out
}

fn mask_at(face: &Face, part: Part, w: usize, h: usize, x: usize, y: usize) -> f32 {
    face.mask(part).map_or(0.0, |m| m.sample((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64))
}

const SKIN: [f32; 3] = [0.42, 0.28, 0.20];

/// Fine texture: a pseudo-random ±6 % brightness per pixel (pores, grain).
fn grain(x: usize, y: usize) -> f32 {
    let h = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    1.0 + ((h % 1000) as f32 / 1000.0 - 0.5) * 0.12
}

/// Skin with blotches (red and yellow patches a twelfth of the picture across) and fine texture; `w × h`.
fn blotchy(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let b = (u * 37.7).sin() * (v * 37.7).cos() * 0.12;
        let g = grain(x * 64 / w.max(1), y * 64 / h.max(1));
        [SKIN[0] * (1.0 + b) * g, SKIN[1] * (1.0 - b * 0.5) * g, SKIN[2] * (1.0 - b) * g]
    })
}

/// The mean of `f` over the pixels where the face's mask is full.
fn mean_inside(img: &Rgb32f, face: &Face, part: Part, f: impl Fn(usize, usize, [f32; 3]) -> f32) -> f32 {
    let (w, h) = (img.width, img.height);
    let (mut sum, mut n) = (0.0, 0);
    for y in 0..h {
        for x in 0..w {
            if mask_at(face, part, w, h, x, y) >= 1.0 {
                sum += f(x, y, img.data[y * w + x]);
                n += 1;
            }
        }
    }
    assert!(n > 100, "{n} pixels inside");
    sum / n as f32
}

/// Log red over green, averaged over 8 × 8 blocks: the blotches without the texture.
fn block_redness(img: &Rgb32f, x: usize, y: usize) -> f32 {
    let (mut r, mut g) = (0.0, 0.0);
    for dy in 0..8 {
        for dx in 0..8 {
            let p = img.data[((y / 8) * 8 + dy).min(img.height - 1) * img.width + ((x / 8) * 8 + dx).min(img.width - 1)];
            (r, g) = (r + p[0], g + p[1]);
        }
    }
    (r / g).ln()
}

/// Texture: each pixel's brightness against its four neighbours'.
fn texture(img: &Rgb32f, x: usize, y: usize) -> f32 {
    let w = img.width;
    let l = |x: usize, y: usize| {
        let p = img.data[y * w + x];
        0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
    };
    let around = (l(x - 1, y) + l(x + 1, y) + l(x, y - 1) + l(x, y + 1)) / 4.0;
    (l(x, y) / around).ln().abs()
}

#[test]
fn even_tone_evens_blotches_and_keeps_the_texture() {
    let (w, h) = (256, 256);
    let img = blotchy(w, h);
    let face = skin_face(0.0, 0.0, 1.0, 1.0);
    let out = retouched(&img, &with(100.0, 0.0, 0.0), vec![face.clone()]);
    let mean_red = |i: &Rgb32f| mean_inside(i, &face, Part::FacialSkin, |x, y, _| block_redness(i, x, y));
    let spread = |i: &Rgb32f| {
        let m = mean_red(i);
        mean_inside(i, &face, Part::FacialSkin, |x, y, _| (block_redness(i, x, y) - m).abs())
    };
    let (before, after) = (spread(&img), spread(&out));
    assert!(after < before * 0.5, "blotches: {before} → {after}");
    let tex = |i: &Rgb32f| mean_inside(i, &face, Part::FacialSkin, |x, y, _| texture(i, x, y));
    let (t0, t1) = (tex(&img), tex(&out));
    assert!((t1 / t0 - 1.0).abs() < 0.05, "texture kept: {t0} → {t1}");
    // half the amount, about half the evening
    let half = spread(&retouched(&img, &with(50.0, 0.0, 0.0), vec![face.clone()]));
    assert!(half < before && half > after, "{before} > {half} > {after}");
}

#[test]
fn redness_lowers_a_red_patch_and_leaves_the_skin_around_it() {
    let (w, h) = (240, 240);
    let spot = |x: usize, y: usize| {
        let (dx, dy) = (x as f32 - 100.0, y as f32 - 110.0);
        (-(dx * dx + dy * dy) / (2.0 * 8.0 * 8.0)).exp()
    };
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let s = spot(x, y);
        let g = grain(x, y);
        [SKIN[0] * (1.0 + 0.25 * s) * g, SKIN[1] * (1.0 - 0.12 * s) * g, SKIN[2] * (1.0 - 0.1 * s) * g]
    });
    let out = retouched(&img, &with(0.0, 100.0, 0.0), vec![skin_face(0.0, 0.0, 1.0, 1.0)]);
    let redness = |i: &Rgb32f| block_redness(i, 100, 110);
    let skin = (SKIN[0] / SKIN[1]).ln();
    let (before, after) = (redness(&img) - skin, redness(&out) - skin);
    assert!(after < before * 0.4, "the patch's excess red: {before} → {after}");
    // brightness kept at the patch, the skin far from it unchanged
    let lum = |p: [f32; 3]| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
    let i = 110 * w + 100;
    assert!((lum(out.data[i]) / lum(img.data[i]) - 1.0).abs() < 0.02);
    let j = 170 * w + 170;
    assert!(img.data[j].iter().zip(&out.data[j]).all(|(a, b)| (a / b - 1.0).abs() < 0.01), "{:?} → {:?}", img.data[j], out.data[j]);
}

/// A face lit from one side with a dark blotch (35 % darker at its middle) and a red patch: at 100, Even Tone takes
/// most of the blotch away and Redness most of the red, while the light across the face stays (issue #205: the
/// blotch was kept as "structure" and only half the red went).
#[test]
fn top_even_tone_and_redness_remove_a_blotch_and_a_red_patch_and_keep_the_shading() {
    let (w, h) = (240, 240);
    let bump = |x: usize, y: usize, cx: f32, cy: f32, s: f32| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        (-(dx * dx + dy * dy) / (2.0 * s * s)).exp()
    };
    // the light: from 0.6 on the left to 1.2 on the right of the face
    let light = |x: usize| 0.6 + 0.6 * x as f32 / w as f32;
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let dark = 1.0 - 0.35 * bump(x, y, 90.0, 100.0, 6.0);
        let red = bump(x, y, 150.0, 140.0, 7.0);
        let g = grain(x, y) * light(x) * dark;
        [SKIN[0] * (1.0 + 0.25 * red) * g, SKIN[1] * (1.0 - 0.12 * red) * g, SKIN[2] * (1.0 - 0.1 * red) * g]
    });
    let out = retouched(&img, &with(100.0, 100.0, 0.0), vec![skin_face(0.0, 0.0, 1.0, 1.0)]);
    let lum = |p: [f32; 3]| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
    // mean log luminance and log red over green in a disc, and in the ring round it
    let disc = |i: &Rgb32f, cx: usize, cy: usize, r0: f32, r1: f32| {
        let (mut l, mut rg, mut n) = (0.0f32, 0.0f32, 0.0f32);
        for y in cy - 30..cy + 30 {
            for x in cx - 30..cx + 30 {
                let d = ((x as f32 - cx as f32).powi(2) + (y as f32 - cy as f32).powi(2)).sqrt();
                if d >= r0 && d < r1 {
                    let p = i.data[y * w + x];
                    (l, rg, n) = (l + lum(p).ln(), rg + (p[0] / p[1]).ln(), n + 1.0);
                }
            }
        }
        (l / n, rg / n)
    };
    let contrast = |i: &Rgb32f| disc(i, 90, 100, 15.0, 22.0).0 - disc(i, 90, 100, 0.0, 4.0).0;
    let redness = |i: &Rgb32f| disc(i, 150, 140, 0.0, 4.0).1 - disc(i, 150, 140, 18.0, 25.0).1;
    let (c0, c1) = (contrast(&img), contrast(&out));
    assert!(c1 < c0 * 0.4, "the blotch's darkness: {c0} → {c1}");
    let (r0, r1) = (redness(&img), redness(&out));
    assert!(r1 < r0 * 0.1, "the patch's excess red: {r0} → {r1}");
    // the light across the face: left against right, away from both
    let side = |i: &Rgb32f| disc(i, 200, 60, 0.0, 10.0).0 - disc(i, 45, 190, 0.0, 10.0).0;
    let (s0, s1) = (side(&img), side(&out));
    assert!((s1 - s0).abs() < 0.03, "the shading: {s0} → {s1}");
}

#[test]
fn face_light_lights_the_face_with_a_soft_edge_and_nothing_outside_it() {
    let (w, h) = (200, 200);
    let img = Rgb32f::from_fn(w, h, |_, _| [0.2, 0.15, 0.1]);
    // the face: skin, and an iris in its middle (the eyes are lit with the face)
    let mut face = skin_face(0.2, 0.2, 0.8, 0.8);
    let iris = Plane::from_fn(8, 8, |_, _| 1.0);
    face.masks[0].1.plane = Plane::from_fn(64, 64, |x, y| {
        let inner = (28..36).contains(&x) && (28..36).contains(&y);
        if inner { 0.0 } else { disc().data[y * 64 + x] }
    });
    face.masks.push((Part::Iris, PartMask { plane: iris, area: NormRect::new(0.4375, 0.4375, 0.5625, 0.5625) }));
    let up = retouched(&img, &with(0.0, 0.0, 100.0), vec![face.clone()]);
    let full = 2f32.powf(FACE_LIGHT_STOPS);
    let gain = |x: usize, y: usize| up.data[y * w + x][1] / img.data[y * w + x][1];
    assert!((gain(100, 100) - full).abs() < 1e-3, "the eye is lit: {}", gain(100, 100));
    assert!((gain(80, 100) - full).abs() < 1e-3, "the middle: {}", gain(80, 100));
    // the edge falls off to nothing
    let r = |x: usize| (x as f32 + 0.5 - 100.0) / 120.0;
    let (mut last, mut seen) = (full, 0);
    for x in 100..200 {
        let g = gain(x, 100);
        assert!(g <= last + 1e-4, "falls off outwards: {x} {g}");
        last = g;
        if g > 1.001 && g < full - 0.01 {
            seen += 1;
            assert!(r(x) > 0.25, "the falloff is near the edge: {x}");
        }
    }
    assert!(seen >= 3, "a soft edge, {seen} pixels wide");
    for y in 0..h {
        for x in 0..w {
            let m = mask_at(&face, Part::FacialSkin, w, h, x, y).max(mask_at(&face, Part::Iris, w, h, x, y));
            if m == 0.0 {
                assert_eq!(up.data[y * w + x].map(f32::to_bits), img.data[y * w + x].map(f32::to_bits), "({x}, {y}) is outside the face");
            }
        }
    }
    // −100 darkens by as much
    let down = retouched(&img, &with(0.0, 0.0, -100.0), vec![face]);
    assert!((img.data[100 * w + 80][1] / down.data[100 * w + 80][1] - full).abs() < 1e-3);
}

/// Every slider on, three faces (one cut by the photo's edge): no pixel outside every mask changes, bit for bit.
#[test]
fn nothing_outside_the_masks_changes() {
    let (w, h) = (160, 120);
    let img = blotchy(w, h);
    let faces = vec![skin_face(0.05, 0.1, 0.4, 0.6), face_with(Part::BodySkin, 0.5, 0.05, 0.75, 0.45), skin_face(0.7, 0.5, 1.2, 1.3)];
    let out = retouched(&img, &with(100.0, 100.0, 100.0), faces.clone());
    let (mut inside, mut outside) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let m = faces.iter().flat_map(|f| [Part::FacialSkin, Part::BodySkin].map(|p| mask_at(f, p, w, h, x, y))).fold(0.0f32, f32::max);
            let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
            if m == 0.0 {
                assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "({x}, {y}) is outside every mask");
                outside += 1;
            } else if m == 1.0 && a != b {
                inside += 1;
            }
        }
    }
    assert!(inside > 1000 && outside > 5000, "{inside} changed inside, {outside} outside");
    // the face cut by the edge is retouched inside the photo
    assert_ne!(out.data[110 * w + 150], img.data[110 * w + 150]);
}

/// A face without masks (no people-parts model: YuNet boxes only), or with masks Skin Tone does not read, is left
/// alone.
#[test]
fn a_face_without_its_masks_is_left_alone() {
    let img = blotchy(80, 60);
    let hair = face_with(Part::Hair, 0.1, 0.1, 0.9, 0.9);
    let out = retouched(&img, &with(100.0, 100.0, 100.0), vec![hair, Face::boxed(NormRect::new(0.2, 0.2, 0.6, 0.7))]);
    assert!(out.data.iter().zip(&img.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
}

/// Dark hair right next to the skin: the skin near the mask's edge keeps its colour (no halo), as every blur reads
/// the skin only.
#[test]
fn no_halo_next_to_dark_hair() {
    let (w, h) = (200, 200);
    let face = skin_face(0.1, 0.1, 0.9, 0.9);
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let m = mask_at(&face, Part::FacialSkin, w, h, x, y);
        if m > 0.0 { SKIN } else { [0.03, 0.02, 0.015] }
    });
    let out = retouched(&img, &with(100.0, 100.0, 0.0), vec![face.clone()]);
    for y in 0..h {
        for x in 0..w {
            if mask_at(&face, Part::FacialSkin, w, h, x, y) > 0.0 {
                let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
                assert!(a.iter().zip(&b).all(|(a, b)| (a / b - 1.0).abs() < 1e-3), "({x}, {y}): {a:?} → {b:?}");
            }
        }
    }
}

/// A flat grey photo stays flat under Even Tone and Redness (nothing to even), and Face Light lights it.
#[test]
fn a_flat_grey_photo_and_a_one_pixel_photo() {
    let grey = Rgb32f::from_fn(60, 60, |_, _| [0.18; 3]);
    let face = skin_face(0.2, 0.2, 0.8, 0.8);
    let out = retouched(&grey, &with(100.0, 100.0, 0.0), vec![face.clone()]);
    assert!(out.data.iter().flatten().all(|v| (v - 0.18).abs() < 1e-5));
    let lit = retouched(&grey, &with(0.0, 0.0, 100.0), vec![face]);
    assert!(lit.data[30 * 60 + 30][0] > 0.18 * 1.4 && lit.data[0] == [0.18; 3]);
    let dot = Rgb32f::from_fn(1, 1, |_, _| [0.4, 0.3, 0.2]);
    let out = retouched(&dot, &with(100.0, 100.0, 100.0), vec![skin_face(0.0, 0.0, 1.0, 1.0)]);
    assert!(out.data[0].iter().all(|v| v.is_finite()));
}

/// Sizes are fractions of the face: a draft and an export four times its size make the same change (the export,
/// scaled down by averaging, matches the draft).
#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let faces = vec![skin_face(0.1, 0.1, 0.6, 0.9), skin_face(0.55, 0.2, 0.95, 0.8)];
    let (w, h) = (100, 80);
    // the same picture at both sizes: the export's pixels are 4 × 4 copies of the draft's
    let small = blotchy(w, h);
    let big = Rgb32f::from_fn(w * 4, h * 4, |x, y| small.data[(y / 4) * w + x / 4]);
    for r in [with(100.0, 0.0, 0.0), with(0.0, 100.0, 0.0), with(0.0, 0.0, 60.0), with(100.0, 100.0, 60.0)] {
        let (draft, export) = (retouched(&small, &r, faces.clone()), retouched(&big, &r, faces.clone()));
        let (mut worst, mut sum) = (0.0f32, 0.0f32);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                for dy in 0..4 {
                    for dx in 0..4 {
                        let p = export.data[(y * 4 + dy) * w * 4 + x * 4 + dx];
                        acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
                    }
                }
                let d = draft.data[y * w + x];
                let e = (0..3).map(|k| (acc[k] / 16.0 / d[k].max(1e-6) - 1.0).abs()).fold(0.0f32, f32::max);
                worst = worst.max(e);
                sum += e;
            }
        }
        let mean = sum / (w * h) as f32;
        // The faces are 64 and 48 pixels tall in the draft: its cells are a whole pixel, coarser than 1/160 of the face
        // the export gets, so single pixels in a blotch differ by up to about 9 % (Even Tone); on the real portraits
        // the draft's and the export's changes correlate at 0.98 to 0.998.
        assert!(mean < 0.01 && worst < 0.1, "mean difference {mean}, worst {worst}");
    }
}

/// The face search keeps only the masks a feature lists in `PARTS`: every part Skin Tone reads is there.
#[test]
fn parts_lists_every_mask_skin_tone_reads() {
    for p in skin_tone::SKIN.iter().chain(&FACE_AREA) {
        assert!(SkinTone::PARTS.contains(p), "{p:?} is read but not kept");
    }
}

/// Settings saved with the stand-in `amount` slider (the Face Light that showed the masks arrive) read as Face Light.
#[test]
fn a_saved_amount_reads_as_face_light() {
    let old: SkinTone = serde_json::from_str(r#"{"amount": 80}"#).unwrap();
    assert_eq!(old, SkinTone { face_light: 80.0, ..SkinTone::default() });
    let new: SkinTone = serde_json::from_str(r#"{"evenTone": 20, "faceLight": -30}"#).unwrap();
    assert_eq!((new.even_tone, new.face_light), (20.0, -30.0));
    // and it is written under its own name
    assert!(serde_json::to_string(&old).unwrap().contains("\"faceLight\":80"));
}

/// A face under 160 pixels in the draft (its grid's cells are then a whole pixel, coarser than 1/160 of the face)
/// still makes the change its export four times larger makes.
#[test]
fn a_small_face_in_the_draft_agrees_with_the_export() {
    let face = vec![skin_face(0.1, 0.1, 0.2, 0.225)];
    let (w, h) = (300, 240);
    let small = blotchy(w, h);
    let big = Rgb32f::from_fn(w * 4, h * 4, |x, y| small.data[(y / 4) * w + x / 4]);
    for r in [with(100.0, 0.0, 0.0), with(0.0, 100.0, 0.0), with(0.0, 0.0, 60.0)] {
        let (draft, export) = (retouched(&small, &r, face.clone()), retouched(&big, &r, face.clone()));
        let (mut worst, mut sum, mut n) = (0.0f32, 0.0f32, 0);
        for y in 0..h {
            for x in 0..w {
                let d = draft.data[y * w + x];
                if d == small.data[y * w + x] {
                    continue;
                }
                let mut acc = [0.0f32; 3];
                for dy in 0..4 {
                    for dx in 0..4 {
                        let p = export.data[(y * 4 + dy) * w * 4 + x * 4 + dx];
                        acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
                    }
                }
                let e = (0..3).map(|k| (acc[k] / 16.0 / d[k].max(1e-6) - 1.0).abs()).fold(0.0f32, f32::max);
                (worst, sum, n) = (worst.max(e), sum + e, n + 1);
            }
        }
        let mean = sum / n.max(1) as f32;
        assert!(n > 200 && mean < 0.01 && worst < 0.05, "{n} changed, mean difference {mean}, worst {worst}");
    }
}
