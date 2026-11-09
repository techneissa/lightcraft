//! Tests of the skin tone feature (`lightcraft_retouch::skin_tone`).

mod common;

use lightcraft_retouch::skin_tone::SkinTone;

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<SkinTone>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<SkinTone>();
}

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

/// A face whose only mask is Facial Skin: a disc in the middle of its box, drawn at 32 × 32.
fn face_with_skin(x0: f64, y0: f64, x1: f64, y1: f64) -> Face {
    let disc = Plane::from_fn(32, 32, |x, y| {
        let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
        if dx * dx + dy * dy < 12.0 * 12.0 { 1.0 } else { 0.0 }
    });
    Face {
        masks: vec![(Part::FacialSkin, PartMask { plane: disc, area: NormRect::new(x0, y0, x1, y1) })],
        ..Face::boxed(NormRect::new(x0, y0, x1, y1))
    }
}

fn face_light(amount: f64) -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.skinTone.amount", amount));
    r
}

fn retouched(img: &Rgb32f, r: &Retouch, faces: Vec<Face>) -> Rgb32f {
    let mut out = img.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    lightcraft_retouch::run::apply(&mut out, r, &Input::new(faces), &view, Stage::Retouch);
    out
}

fn mask_at(face: &Face, w: usize, h: usize, x: usize, y: usize) -> f32 {
    face.mask(Part::FacialSkin).map_or(0.0, |m| m.sample((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64))
}

#[test]
fn face_light_brightens_facial_skin_and_nothing_outside_it() {
    let (w, h) = (120, 90);
    let img = common::picture(w, h);
    let faces = vec![face_with_skin(0.1, 0.2, 0.4, 0.6), face_with_skin(0.5, 0.1, 0.7, 0.4), face_with_skin(0.75, 0.5, 0.95, 0.9)];
    let out = retouched(&img, &face_light(100.0), faces.clone());
    let (mut inside, mut outside) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let m = faces.iter().map(|f| mask_at(f, w, h, x, y)).fold(0.0f32, f32::max);
            let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
            if m == 0.0 {
                assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "({x}, {y}) is outside every mask");
                outside += 1;
            } else if m == 1.0 {
                assert!((b[1] - a[1] * (1.0 + lightcraft_retouch::skin_tone::FACE_LIGHT_GAIN)).abs() < 1e-5, "({x}, {y}): {a:?} → {b:?}");
                inside += 1;
            }
        }
    }
    assert!(inside > 300 && outside > 3000, "{inside} inside, {outside} outside");
    // half the amount, half the gain
    let half = retouched(&img, &face_light(50.0), faces.clone());
    let c = faces[0].bounds.centre();
    let i = (c.1 * h as f64) as usize * w + (c.0 * w as f64) as usize;
    assert!((half.data[i][0] - img.data[i][0] * 1.25).abs() < 1e-5);
}

#[test]
fn face_light_leaves_a_face_without_the_facial_skin_mask_alone() {
    let img = common::picture(64, 48);
    let mut hair_only = face_with_skin(0.2, 0.2, 0.8, 0.8);
    hair_only.masks[0].0 = Part::Hair;
    let out = retouched(&img, &face_light(100.0), vec![hair_only, Face::boxed(NormRect::new(0.1, 0.1, 0.5, 0.5))]);
    assert!(out.data.iter().zip(&img.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
}

/// A face cut by the photo's edge, a 1 × 1 and a flat grey photo: no panic, and the part inside is lit.
#[test]
fn face_light_on_a_face_cut_by_the_edge_a_one_pixel_and_a_flat_grey_photo() {
    let cut = face_with_skin(-0.2, -0.3, 0.3, 0.4);
    let img = common::picture(80, 60);
    let out = retouched(&img, &face_light(100.0), vec![cut.clone()]);
    let (x, y) = (2, 5);
    assert!(mask_at(&cut, 80, 60, x, y) == 1.0 && out.data[y * 80 + x][0] > img.data[y * 80 + x][0]);
    let dot = Rgb32f::from_fn(1, 1, |_, _| [0.4, 0.3, 0.2]);
    let lit = retouched(&dot, &face_light(100.0), vec![face_with_skin(0.0, 0.0, 1.0, 1.0)]);
    assert!(lit.data[0][0] > 0.4 && lit.data[0][0].is_finite());
    let grey = Rgb32f::from_fn(50, 50, |_, _| [0.18; 3]);
    let lit = retouched(&grey, &face_light(100.0), vec![face_with_skin(0.3, 0.3, 0.7, 0.7)]);
    assert!(lit.data[25 * 50 + 25][1] > 0.18 * 1.49 && lit.data[0] == [0.18; 3]);
}

/// The mask is in fractions of the photo: a draft and an export four times its size light the same places (the
/// export, scaled down by averaging, matches the draft).
#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let faces = vec![face_with_skin(0.2, 0.15, 0.55, 0.7), face_with_skin(0.6, 0.3, 0.85, 0.65)];
    let flat = |w: usize, h: usize| Rgb32f::from_fn(w, h, |_, _| [0.3, 0.25, 0.2]);
    let (w, h) = (60, 40);
    let draft = retouched(&flat(w, h), &face_light(80.0), faces.clone());
    let big = retouched(&flat(w * 4, h * 4), &face_light(80.0), faces);
    let mut worst = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            for dy in 0..4 {
                for dx in 0..4 {
                    sum += big.data[(y * 4 + dy) * w * 4 + x * 4 + dx][0];
                }
            }
            worst = worst.max((sum / 16.0 - draft.data[y * w + x][0]).abs());
        }
    }
    // (only along the disc's edge, where the export resolves what one draft pixel averages)
    assert!(worst < 0.3 * 0.4 * 0.5, "worst difference {worst}");
}
