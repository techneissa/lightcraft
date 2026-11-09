//! Tests of the glare feature (`lightcraft_retouch::glare`): it blends a face's repair patch in by Amount.

mod common;

use std::sync::Arc;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::glare::Glare;
use lightcraft_retouch::input::Patch;
use lightcraft_retouch::{Face, Input, NormRect, Retouch, Stage, WholePhoto};

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Glare>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Glare>();
}

/// A patch over `area` that halves every channel inside a disc and leaves the rest (alpha 0 there).
fn halving_patch(area: NormRect) -> Patch {
    let n = 32;
    let alpha = Plane::from_fn(n, n, |x, y| {
        let r = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
        (12.0 - r).clamp(0.0, 1.0)
    });
    Patch { area, gain: Rgb32f::filled(n, n, [0.5, 0.5, 0.5]), alpha, stamp: 7 }
}

fn with_patch(patch: Option<Patch>) -> Input {
    let mut f = common::face(0.2, 0.2, 0.8, 0.8);
    f.patch = patch.map(Arc::new);
    Input::new(vec![f])
}

fn amount(v: f64) -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.glare.amount", v));
    r
}

fn run(img: &mut Rgb32f, r: &Retouch, input: &Input) {
    let view = WholePhoto { width: img.width, height: img.height };
    lightcraft_retouch::run::apply(img, r, input, &view, Stage::Retouch);
}

fn flat(w: usize, h: usize) -> Rgb32f {
    Rgb32f::filled(w, h, [0.8, 0.6, 0.4])
}

#[test]
fn the_patch_shows_by_amount_inside_its_mask_and_nothing_changes_outside() {
    let input = with_patch(Some(halving_patch(NormRect::new(0.25, 0.25, 0.75, 0.75))));
    let before = flat(100, 100);
    let mut full = before.clone();
    run(&mut full, &amount(100.0), &input);
    let mut half = before.clone();
    run(&mut half, &amount(50.0), &input);
    // the centre: halved at 100, three quarters at 50
    let c = 50 * 100 + 50;
    assert!((full.data[c][0] - 0.4).abs() < 1e-4, "{:?}", full.data[c]);
    assert!((half.data[c][0] - 0.6).abs() < 1e-4, "{:?}", half.data[c]);
    // outside the disc, and outside the area, every bit is kept
    for (x, y) in [(5, 5), (26, 26), (95, 50), (50, 74)] {
        let i = y * 100 + x;
        assert_eq!(full.data[i].map(f32::to_bits), before.data[i].map(f32::to_bits), "({x}, {y}) changed");
    }
}

#[test]
fn a_face_without_a_patch_is_left_alone() {
    let before = common::picture(64, 48);
    let mut img = before.clone();
    run(&mut img, &amount(100.0), &with_patch(None));
    assert!(img.data.iter().zip(&before.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
}

/// The patch is sampled in photo coordinates: a preview and an export four times its size make the same change.
#[test]
fn preview_equals_export() {
    let input = with_patch(Some(halving_patch(NormRect::new(0.25, 0.25, 0.75, 0.75))));
    let mut small = flat(50, 50);
    run(&mut small, &amount(80.0), &input);
    let mut big = flat(200, 200);
    run(&mut big, &amount(80.0), &input);
    for (sx, sy) in [(25, 25), (20, 25), (30, 30), (10, 40)] {
        // the big render's 4 × 4 block under the small pixel
        let mut mean = 0.0;
        for dy in 0..4 {
            for dx in 0..4 {
                mean += big.data[(sy * 4 + dy) * 200 + sx * 4 + dx][1];
            }
        }
        mean /= 16.0;
        let v = small.data[sy * 50 + sx][1];
        assert!((mean - v).abs() < 0.03, "({sx}, {sy}): preview {v}, export {mean}");
    }
}

/// Malformed patches (sizes that disagree, an empty one, numbers that are not numbers) change nothing and never panic.
#[test]
fn malformed_patches_change_nothing() {
    let area = NormRect::new(0.25, 0.25, 0.75, 0.75);
    let bad = [
        Patch { area, gain: Rgb32f::filled(4, 4, [0.5; 3]), alpha: Plane::filled(5, 4, 1.0), stamp: 1 },
        Patch { area, gain: Rgb32f::new(0, 0), alpha: Plane::new(0, 0), stamp: 2 },
        Patch { area: NormRect::new(f64::NAN, 0.0, 1.0, 1.0), gain: Rgb32f::filled(4, 4, [0.5; 3]), alpha: Plane::filled(4, 4, 1.0), stamp: 3 },
        Patch { area: NormRect::new(0.6, 0.6, 0.4, 0.4), gain: Rgb32f::filled(4, 4, [0.5; 3]), alpha: Plane::filled(4, 4, 1.0), stamp: 4 },
    ];
    let before = flat(40, 40);
    for p in bad {
        let mut img = before.clone();
        run(&mut img, &amount(100.0), &with_patch(Some(p)));
        assert!(img.data.iter().zip(&before.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)));
    }
    // gains that are not numbers keep the pixel; huge ones are capped
    let n = 4;
    let wild =
        Patch { area, gain: Rgb32f::from_fn(n, n, |x, _| if x < 2 { [f32::NAN; 3] } else { [1e30; 3] }), alpha: Plane::filled(n, n, 1.0), stamp: 5 };
    let mut img = before.clone();
    run(&mut img, &amount(100.0), &with_patch(Some(wild)));
    assert!(img.data.iter().flatten().all(|v| v.is_finite() && *v <= 0.8 * lightcraft_retouch::input::GAIN_MAX + 1e-3));
}

/// A patch changes the fingerprint (renders made before it are redrawn); the same patch gives the same one.
#[test]
fn the_patch_is_part_of_the_fingerprint() {
    let area = NormRect::new(0.25, 0.25, 0.75, 0.75);
    let none = with_patch(None).fingerprint();
    let a = with_patch(Some(halving_patch(area))).fingerprint();
    let mut other = halving_patch(area);
    other.stamp = 8;
    assert_ne!(none, a);
    assert_eq!(a, with_patch(Some(halving_patch(area))).fingerprint());
    assert_ne!(a, with_patch(Some(other)).fingerprint());
    let _ = Face::default();
}
