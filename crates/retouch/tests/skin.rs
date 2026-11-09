//! Tests of the skin feature (`lightcraft_retouch::skin`).

mod common;

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::skin::Skin;
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

#[test]
fn off_by_default_and_every_slider_at_zero_changes_nothing() {
    common::off_changes_nothing::<Skin>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Skin>();
}

/// A face whose only mask is Facial Skin: a disc filling most of its box, with a soft edge two samples wide, drawn
/// at 64 × 64.
fn face_with_skin(x0: f64, y0: f64, x1: f64, y1: f64) -> Face {
    let disc = Plane::from_fn(64, 64, |x, y| {
        let r = ((x as f32 - 31.5).powi(2) + (y as f32 - 31.5).powi(2)).sqrt();
        (28.0 - r).clamp(0.0, 2.0) / 2.0
    });
    Face {
        masks: vec![(Part::FacialSkin, PartMask { plane: disc, area: NormRect::new(x0, y0, x1, y1) })],
        ..Face::boxed(NormRect::new(x0, y0, x1, y1))
    }
}

fn skin(smoothness: f64, texture: f64, shine: f64) -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.skin.smoothness", smoothness));
    assert!(r.global.set("retouch.skin.texture", texture));
    assert!(r.global.set("retouch.skin.shine", shine));
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

fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}

/// A repeatable noise in −1…1 per pixel (the skin's pores).
fn noise(x: usize, y: usize) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h % 2001) as f32 / 1000.0 - 1.0
}

const SKIN: [f32; 3] = [0.45, 0.3, 0.24];

/// Skin with blotches (a wave `period` pixels long, ±`blotch`) and pores (per-pixel noise, ±`pores`).
fn skin_picture(w: usize, h: usize, period: f32, blotch: f32, pores: f32) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        // sampled at pixel centres, so a picture four times the size has the same wave
        let at = |v: usize| (v as f32 + 0.5) / period * std::f32::consts::TAU;
        let wave = at(x).sin() * at(y).cos();
        let k = 1.0 + blotch * wave + pores * noise(x, y);
        SKIN.map(|c| c * k)
    })
}

/// The mean square of `img`'s luminance less its blur by `fine`, less (with `coarse`) its blur by `coarse`: the
/// energy of the band between them, over the pixels where the mask is 1 and `margin` pixels from the picture's edge.
fn band_energy(img: &Rgb32f, face: &Face, fine: f32, coarse: Option<f32>, margin: usize) -> f64 {
    let y = img.luminance();
    let a = gaussian(&y, fine);
    let b = coarse.map(|c| gaussian(&y, c));
    let (w, h) = (img.width, img.height);
    let (mut sum, mut n) = (0.0f64, 0usize);
    for py in margin..h - margin {
        for px in margin..w - margin {
            if mask_at(face, w, h, px, py) < 1.0 {
                continue;
            }
            let i = py * w + px;
            let v = match &b {
                Some(b) => a.data[i] - b.data[i],
                None => y.data[i] - a.data[i],
            };
            sum += f64::from(v * v);
            n += 1;
        }
    }
    assert!(n > 1000, "{n} pixels measured");
    sum / n as f64
}

/// Pixels where every face's mask is 0 keep their bits; pixels in the masks change.
#[test]
fn pixels_outside_the_skin_mask_never_change() {
    let (w, h) = (160, 120);
    let img = skin_picture(w, h, 14.0, 0.1, 0.05);
    let faces = vec![face_with_skin(0.05, 0.1, 0.45, 0.7), face_with_skin(0.5, 0.05, 0.75, 0.45), face_with_skin(0.7, 0.5, 1.1, 1.2)];
    let out = retouched(&img, &skin(100.0, 100.0, 100.0), faces.clone());
    let (mut outside, mut changed) = (0, 0);
    for y in 0..h {
        for x in 0..w {
            let m = faces.iter().map(|f| mask_at(f, w, h, x, y)).fold(0.0f32, f32::max);
            let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
            if m == 0.0 {
                assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "({x}, {y}) is outside every mask");
                outside += 1;
            } else if a != b {
                changed += 1;
            }
        }
    }
    assert!(outside > 3000 && changed > 3000, "{outside} outside, {changed} changed");
}

/// No people parse (no model, or YuNet's boxes only), or a parse without Facial Skin: the picture is left alone.
#[test]
fn a_face_without_the_facial_skin_mask_is_left_alone() {
    let img = skin_picture(80, 60, 10.0, 0.1, 0.05);
    let mut hair_only = face_with_skin(0.2, 0.2, 0.8, 0.8);
    hair_only.masks[0].0 = Part::Hair;
    let mut empty = face_with_skin(0.1, 0.1, 0.6, 0.6);
    empty.masks[0].1.plane = Plane::filled(8, 8, 0.0);
    let out = retouched(&img, &skin(100.0, 100.0, 100.0), vec![hair_only, empty, Face::boxed(NormRect::new(0.1, 0.1, 0.5, 0.5))]);
    assert_eq!(out, img);
}

/// Smoothness takes the blotches away and keeps the pores: the band between the fine and the tone blur loses most of
/// its energy, the finest band keeps its own within a set ratio.
#[test]
fn smoothness_evens_the_skin_and_keeps_its_texture() {
    let (w, h) = (400, 400);
    // a face 380 px across: blotches 40 px long (a tenth of the face), pores one pixel
    let face = face_with_skin(0.0, 0.0, 0.95, 0.95);
    let img = skin_picture(w, h, 40.0, 0.12, 0.08);
    let out = retouched(&img, &skin(100.0, 0.0, 0.0), vec![face.clone()]);
    let (blotches, pores) = (|i: &Rgb32f| band_energy(i, &face, 3.0, Some(30.0), 10), |i: &Rgb32f| band_energy(i, &face, 1.0, None, 10));
    let (b0, b1) = (blotches(&img), blotches(&out));
    assert!(b1 < b0 * 0.35, "blotch energy {b0:.2e} → {b1:.2e}");
    let ratio = pores(&out) / pores(&img);
    assert!((0.85..=1.15).contains(&ratio), "pore energy kept: ratio {ratio:.3}");
    // and on average the skin keeps its brightness
    let mean = |i: &Rgb32f| i.data.iter().map(|p| f64::from(luma(*p))).sum::<f64>() / i.data.len() as f64;
    assert!((mean(&out) / mean(&img) - 1.0).abs() < 0.02, "{} → {}", mean(&img), mean(&out));
}

/// Texture strengthens the pores and leaves the blotches as they are.
#[test]
fn texture_strengthens_the_pores_only() {
    let face = face_with_skin(0.0, 0.0, 0.95, 0.95);
    let img = skin_picture(400, 400, 40.0, 0.12, 0.04);
    let out = retouched(&img, &skin(0.0, 100.0, 0.0), vec![face.clone()]);
    let ratio = band_energy(&out, &face, 1.0, None, 10) / band_energy(&img, &face, 1.0, None, 10);
    assert!(ratio > 1.8, "pore energy ×{ratio:.2}");
    // the change, blurred past the pores, is next to nothing beside the blotches
    let change = Plane::from_fn(400, 400, |x, y| luma(out.data[y * 400 + x]) - luma(img.data[y * 400 + x]));
    let blotches = band_energy(&img, &face, 6.0, Some(30.0), 10);
    let low = gaussian(&change, 6.0);
    let low_energy = low.data.iter().map(|v| f64::from(v * v)).sum::<f64>() / low.data.len() as f64;
    assert!(low_energy < blotches * 0.01, "the blotches changed: {low_energy:.2e} against {blotches:.2e}");
}

/// Skin with a hot spot (a Gaussian 0.04 of the face wide), of `light` added in the colour `tint` (white: shine; the skin's own
/// colour: light falling on it).
fn hot_spot(tint: [f32; 3], light: f32) -> (Rgb32f, usize) {
    let (w, h) = (300, 300);
    let (cx, cy, s) = (150.0f32, 150.0f32, 0.04 * 285.0);
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let d2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
        let g = light * (-d2 / (2.0 * s * s)).exp();
        std::array::from_fn(|c| SKIN[c] + g * tint[c])
    });
    (img, 150 * w + 150)
}

/// Shine lowers a pale highlight and gives it back the skin's colour; light in the skin's own colour (one side of a
/// face lit) is lowered far less; flat skin is left as it is.
#[test]
fn shine_lowers_oily_highlights() {
    let face = face_with_skin(0.0, 0.0, 0.95, 0.95);
    let sat = |p: [f32; 3]| (p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])) / p[0].max(p[1]).max(p[2]);
    let (img, i) = hot_spot([1.0, 1.0, 1.0], 0.4);
    let out = retouched(&img, &skin(0.0, 0.0, 100.0), vec![face.clone()]);
    let (before, after, flat) = (luma(img.data[i]), luma(out.data[i]), luma(SKIN));
    assert!(after - flat < (before - flat) * 0.6, "the highlight's excess {:.3} → {:.3}", before - flat, after - flat);
    assert!(sat(out.data[i]) > sat(img.data[i]) * 1.2, "colour back: {:.3} → {:.3}", sat(img.data[i]), sat(out.data[i]));
    let far = 40 * 300 + 150;
    assert!((luma(out.data[far]) - luma(img.data[far])).abs() < 1e-4, "skin away from the highlight is left alone");
    // the same brightening in the skin's own colour
    let tint = SKIN.map(|c| c / luma(SKIN));
    let (lit, j) = hot_spot(tint, 0.4 * luma([1.0; 3]) / luma(tint));
    let lit_out = retouched(&lit, &skin(0.0, 0.0, 100.0), vec![face.clone()]);
    let (pale_drop, lit_drop) = (before - after, luma(lit.data[j]) - luma(lit_out.data[j]));
    assert!(lit_drop < pale_drop * 0.4, "light in the skin's colour lowered {lit_drop:.3}, shine {pale_drop:.3}");
}

/// A flat grey photo has no unevenness, texture or shine: every slider at 100 leaves it flat.
#[test]
fn flat_grey_stays_flat() {
    let img = Rgb32f::from_fn(90, 70, |_, _| [0.18; 3]);
    let out = retouched(&img, &skin(100.0, 100.0, 100.0), vec![face_with_skin(0.2, 0.1, 0.8, 0.9)]);
    assert!(
        out.data.iter().flatten().all(|v| (v - 0.18).abs() < 1e-5),
        "{:?}",
        out.data.iter().flatten().fold(0.0f32, |a, v| a.max((v - 0.18).abs()))
    );
}

/// No halo at the mask's edge: skin next to dark hair keeps its brightness up to the edge (a plain blur would pull
/// the hair's darkness into it).
#[test]
fn no_halo_at_the_mask_edge() {
    let (w, h) = (300, 300);
    let face = face_with_skin(0.1, 0.1, 0.9, 0.9);
    let hair = [0.02, 0.015, 0.01];
    let img = Rgb32f::from_fn(w, h, |x, y| if mask_at(&face, w, h, x, y) > 0.0 { SKIN.map(|c| c * (1.0 + 0.05 * noise(x, y))) } else { hair });
    let out = retouched(&img, &skin(100.0, 100.0, 100.0), vec![face.clone()]);
    let (mut worst, mut ring) = (0.0f32, 0);
    for y in 0..h {
        for x in 0..w {
            let m = mask_at(&face, w, h, x, y);
            // the skin's outer ring: fully inside the mask, within about 2 % of the face from its edge
            let inner = [(6isize, 0isize), (-6, 0), (0, 6), (0, -6)].iter().any(|(dx, dy)| {
                mask_at(&face, w, h, (x as isize + dx).clamp(0, w as isize - 1) as usize, (y as isize + dy).clamp(0, h as isize - 1) as usize) < 1.0
            });
            if m == 1.0 && inner {
                let (a, b) = (luma(img.data[y * w + x]), luma(out.data[y * w + x]));
                worst = worst.max((a - b) / a);
                ring += 1;
            }
        }
    }
    assert!(ring > 500, "{ring} pixels in the ring");
    assert!(worst < 0.12, "a pixel at the edge darkened by {:.1} %", worst * 100.0);
}

/// A face cut by the photo's edge is retouched where it is in the photo.
#[test]
fn a_face_cut_by_the_edge_is_retouched_inside_the_photo() {
    let img = skin_picture(120, 90, 12.0, 0.12, 0.05);
    let cut = face_with_skin(-0.3, -0.4, 0.4, 0.5);
    let out = retouched(&img, &skin(100.0, 50.0, 50.0), vec![cut.clone()]);
    assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    let inside = (0..90 * 120).filter(|i| mask_at(&cut, 120, 90, i % 120, i / 120) > 0.0).count();
    let changed = (0..90 * 120).filter(|i| out.data[*i] != img.data[*i]).count();
    assert!(inside > 1000 && changed * 10 > inside * 9, "{changed} of the {inside} pixels in the mask changed");
    let dot = Rgb32f::from_fn(1, 1, |_, _| SKIN);
    let one = retouched(&dot, &skin(100.0, 100.0, 100.0), vec![face_with_skin(0.0, 0.0, 1.0, 1.0)]);
    assert!(one.data[0].iter().all(|v| v.is_finite() && *v >= 0.0));
}

/// Sizes are fractions of the face: a draft and an export four times its size make the same change once the export
/// is scaled down.
#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let face = face_with_skin(0.05, 0.05, 0.95, 0.95);
    let photo = |w: usize| {
        let p = w as f32 / 10.0;
        skin_picture(w, w, p, 0.12, 0.0)
    };
    let r = skin(80.0, 0.0, 60.0);
    let (small, big) = (photo(120), photo(480));
    let (ds, db) = (retouched(&small, &r, vec![face.clone()]), retouched(&big, &r, vec![face.clone()]));
    let (mut diff, mut change) = (0.0f64, 0.0f64);
    for y in 0..120 {
        for x in 0..120 {
            if mask_at(&face, 120, 120, x, y) < 1.0 {
                continue;
            }
            let mut sum = 0.0;
            for k in 0..16 {
                let i = (y * 4 + k / 4) * 480 + x * 4 + k % 4;
                sum += luma(db.data[i]) - luma(big.data[i]);
            }
            let (c_big, c_small) = (sum / 16.0, luma(ds.data[y * 120 + x]) - luma(small.data[y * 120 + x]));
            diff += f64::from((c_big - c_small).abs());
            change += f64::from(c_small.abs());
        }
    }
    assert!(change > 1.0, "the draft changed: {change}");
    assert!(diff < change * 0.15, "the export's change differs from the draft's by {:.1} %", diff / change * 100.0);
}

/// A face with its own settings gets them; the others keep the settings for every face (here: off).
#[test]
fn a_face_with_its_own_settings_is_the_only_one_retouched() {
    let (w, h) = (160, 80);
    let img = skin_picture(w, h, 12.0, 0.12, 0.05);
    let faces = vec![face_with_skin(0.0, 0.0, 0.45, 0.9), face_with_skin(0.5, 0.0, 0.95, 0.9)];
    let mut r = Retouch::default();
    assert!(r.override_mut(&faces, 1).is_some_and(|f| f.set("retouch.skin.smoothness", 100.0)));
    let out = retouched(&img, &r, faces.clone());
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
            if mask_at(&faces[0], w, h, x, y) > 0.0 {
                assert_eq!(a, b, "face 0 is off");
            }
        }
    }
    assert!((0..w * h).any(|i| img.data[i] != out.data[i]), "face 1 is retouched");
}
