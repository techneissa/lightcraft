//! Tests of the wrinkles feature (`lightcraft_retouch::wrinkles`).

mod common;

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::wrinkles::{Area, Wrinkles, areas};
use lightcraft_retouch::{Face, Feature, Input, NormRect, Part, PartMask, Retouch, Stage, View, WholePhoto};

#[test]
fn off_by_default_and_every_slider_at_zero_changes_nothing() {
    common::off_changes_nothing::<Wrinkles>();
    // a face with the mesh and its masks, every slider at 0
    let img = forehead_lines(320, 400, 1.0, 0.0);
    let out = retouched(&img, &Retouch::default(), vec![meshed(FACE)]);
    assert!(same_bits(&img, &out));
    let out = retouched(&img, &wrinkles(0.0, 0.0, 0.0, 0.0), vec![meshed(FACE)]);
    assert!(same_bits(&img, &out));
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Wrinkles>();
    let r = wrinkles(100.0, 100.0, 100.0, 100.0);
    // 50 faces with the mesh, faces past the edges, a mesh of one point, a mesh with points that are not numbers
    let many: Vec<Face> = (0..50).map(|i| meshed([(i % 10) as f64 / 10.0, (i / 10) as f64 / 5.0, 0.1, 0.2])).collect();
    let mut one_point = meshed(FACE);
    one_point.landmarks.iter_mut().for_each(|p| *p = (0.5, 0.5));
    let mut nan = meshed(FACE);
    nan.landmarks.iter_mut().step_by(3).for_each(|p| *p = (f64::NAN, f64::INFINITY));
    let odd = vec![meshed([-0.3, -0.4, 0.6, 0.6]), meshed([0.8, 0.7, 0.6, 0.6]), one_point, nan];
    for (w, h) in [(64, 48), (1, 1), (0, 0), (3, 1), (320, 400)] {
        for faces in [many.clone(), odd.clone()] {
            let out = retouched(&common::picture(w, h), &r, faces);
            assert_eq!((out.width, out.height), (w, h));
            assert!(out.data.iter().flatten().all(|v| v.is_finite()));
        }
    }
}

/// The face's box in the test pictures (x0, y0, width, height, as fractions of a 320 × 400 picture): 200 × 200 px.
const FACE: [f64; 4] = [60.0 / 320.0, 40.0 / 400.0, 200.0 / 320.0, 200.0 / 400.0];

/// A face with YuNet's five points, the 478 of the mesh, Facial Skin over its box and Body Skin under it.
fn meshed(b: [f64; 4]) -> Face {
    let [x0, y0, w, h] = b;
    let at = |(fx, fy): (f64, f64)| (x0 + fx * w, y0 + fy * h);
    let five = [(0.33, 0.45), (0.67, 0.45), (0.5, 0.62), (0.38, 0.78), (0.62, 0.78)];
    let landmarks = five.into_iter().map(at).chain((0..areas::MESH).map(|i| at(areas::frontal(i)))).collect();
    let skin = |area: NormRect| PartMask { plane: Plane::filled(8, 8, 1.0), area };
    Face {
        landmarks,
        masks: vec![
            (Part::FacialSkin, skin(NormRect::new(x0, y0 - 0.1 * h, x0 + w, y0 + h))),
            (Part::BodySkin, skin(NormRect::new(x0, y0 + h, x0 + w, y0 + 1.8 * h))),
        ],
        ..Face::boxed(NormRect::new(x0, y0, x0 + w, y0 + h))
    }
}

fn wrinkles(forehead: f64, crows_feet: f64, smile_lines: f64, neck: f64) -> Retouch {
    let mut r = Retouch::default();
    for (id, v) in [("forehead", forehead), ("crowsFeet", crows_feet), ("smileLines", smile_lines), ("neck", neck)] {
        assert!(r.global.set(&format!("retouch.wrinkles.{id}"), v));
    }
    r
}

fn retouched_in(img: &Rgb32f, r: &Retouch, faces: Vec<Face>, view: &dyn View) -> Rgb32f {
    let mut out = img.clone();
    lightcraft_retouch::run::apply(&mut out, r, &Input::new(faces), view, Stage::Retouch);
    out
}

fn retouched(img: &Rgb32f, r: &Retouch, faces: Vec<Face>) -> Rgb32f {
    retouched_in(img, r, faces, &WholePhoto { width: img.width, height: img.height })
}

fn same_bits(a: &Rgb32f, b: &Rgb32f) -> bool {
    a.data.iter().zip(&b.data).all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits))
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
/// The forehead's lines: at these heights of the face's box.
const LINES: [f64; 3] = [0.12, 0.18, 0.24];
/// A line's half-width (Gaussian sigma), a fraction of the face's size, and how much darker its middle is.
const LINE_SIGMA: f64 = 0.007;
const LINE_DEPTH: f32 = 0.3;

/// Skin over the whole picture with three dark lines across the forehead of [`FACE`] (the picture `scale` times
/// 320 × 400) and pores (±`pores` per pixel).
fn forehead_lines(w: usize, h: usize, scale: f64, pores: f32) -> Rgb32f {
    let (fx, fy, fs) = (FACE[0] * w as f64, FACE[1] * h as f64, 200.0 * scale);
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (((x as f64 + 0.5) - fx) / fs, ((y as f64 + 0.5) - fy) / fs);
        let across = (0.25..0.75).contains(&u);
        let line: f64 = if across { LINES.iter().map(|l| (-0.5 * ((v - l) / LINE_SIGMA).powi(2)).exp()).sum() } else { 0.0 };
        let k = (1.0 - LINE_DEPTH * line as f32) * (1.0 + pores * noise(x, y));
        SKIN.map(|c| c * k)
    })
}

/// How dark the lines are: the mean luminance between them less on them, over the middle of the forehead
/// (a fraction of the skin's luminance).
fn line_depth(img: &Rgb32f, scale: f64) -> f64 {
    let (fx, fy, fs) = (FACE[0] * img.width as f64, FACE[1] * img.height as f64, 200.0 * scale);
    let row_mean = |v: f64| {
        let y = (fy + v * fs) as usize;
        let xs = (fx + 0.35 * fs) as usize..(fx + 0.65 * fs) as usize;
        let n = xs.len() as f64;
        xs.map(|x| f64::from(luma(img.data[y * img.width + x]))).sum::<f64>() / n
    };
    let between = (row_mean(0.15) + row_mean(0.21)) / 2.0;
    let on = LINES.iter().map(|l| row_mean(*l)).sum::<f64>() / LINES.len() as f64;
    (between - on) / f64::from(luma(SKIN))
}

/// The energy of the picture's luminance finer than a 1 px blur over the middle of the forehead, away from the lines.
fn pore_energy(img: &Rgb32f) -> f64 {
    let y = Plane::from_fn(img.width, img.height, |x, y| luma(img.data[y * img.width + x]).ln());
    let b = gaussian(&y, 1.0);
    let (fx, fy) = (FACE[0] * img.width as f64, FACE[1] * img.height as f64);
    let (mut e, mut n) = (0.0, 0);
    for yy in (fy + 0.1 * 200.0) as usize..(fy + 0.28 * 200.0) as usize {
        let v = (yy as f64 + 0.5 - fy) / 200.0;
        if LINES.iter().any(|l| (v - l).abs() < 4.0 * LINE_SIGMA) {
            continue;
        }
        for x in (fx + 0.35 * 200.0) as usize..(fx + 0.65 * 200.0) as usize {
            let d = f64::from(y.data[yy * img.width + x] - b.data[yy * img.width + x]);
            e += d * d;
            n += 1;
        }
    }
    e / n.max(1) as f64
}

#[test]
fn forehead_softens_its_lines_and_keeps_the_pores() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let out = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![meshed(FACE)]);
    let (before, after) = (line_depth(&img, 1.0), line_depth(&out, 1.0));
    assert!(before > 0.2, "the lines are there: {before}");
    assert!(after < before * 0.4, "Forehead 100 takes away most of the lines' depth: {before:.3} → {after:.3}");
    let (pb, pa) = (pore_energy(&img), pore_energy(&out));
    assert!((pa / pb - 1.0).abs() < 0.15, "the pores keep their energy: {pb:.3e} → {pa:.3e}");
    // halfway is about half
    let half = line_depth(&retouched(&img, &wrinkles(50.0, 0.0, 0.0, 0.0), vec![meshed(FACE)]), 1.0);
    assert!(half < before && half > after, "{before:.3} {half:.3} {after:.3}");
}

#[test]
fn the_face_shape_and_flat_skin_stay() {
    // light falling across the face (a broad gradient and a broad shadow), no lines: nothing to lift
    let (w, h) = (320, 400);
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let u = x as f32 / w as f32;
        let v = (y as f32 - 120.0) / 60.0;
        SKIN.map(|c| c * (0.6 + 0.6 * u) * (1.0 - 0.35 * (-0.5 * v * v).exp()))
    });
    let out = retouched(&img, &wrinkles(100.0, 100.0, 100.0, 100.0), vec![meshed(FACE)]);
    let most = img.data.iter().zip(&out.data).map(|(a, b)| (luma(*b) / luma(*a) - 1.0).abs()).fold(0.0f32, f32::max);
    assert!(most < 0.01, "the face's shape and light stay: {most}");
    let grey = Rgb32f::from_fn(w, h, |_, _| [0.18; 3]);
    let out = retouched(&grey, &wrinkles(100.0, 100.0, 100.0, 100.0), vec![meshed(FACE)]);
    assert!(out.data.iter().all(|p| p.iter().all(|v| (v - 0.18).abs() < 1e-5)), "flat grey stays flat");
}

/// The mask of `part` of `face` at pixel (`x`, `y`) of a `w × h` picture.
fn mask_at(face: &Face, part: Part, w: usize, h: usize, x: usize, y: usize) -> f32 {
    face.mask(part).map_or(0.0, |m| m.sample((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64))
}

#[test]
fn pixels_outside_the_masks_never_change() {
    let (w, h) = (320, 400);
    // lines everywhere, and a Facial Skin mask that covers the left half of the forehead only
    let img = Rgb32f::from_fn(w, h, |x, y| SKIN.map(|c| c * (1.0 - 0.3 * ((y % 9 == 0) as u8 as f32)) * (1.0 + 0.03 * noise(x, y))));
    let mut face = meshed(FACE);
    face.masks.retain(|(p, _)| *p != Part::FacialSkin);
    let half = Plane::from_fn(16, 16, |x, _| if x < 8 { 1.0 } else { 0.0 });
    face.masks.push((Part::FacialSkin, PartMask { plane: half, area: NormRect::new(FACE[0], FACE[1], FACE[0] + FACE[2], FACE[1] + FACE[3]) }));
    let out = retouched(&img, &wrinkles(100.0, 100.0, 100.0, 100.0), vec![face.clone()]);
    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let outside = mask_at(&face, Part::FacialSkin, w, h, x, y) <= 0.0 && mask_at(&face, Part::BodySkin, w, h, x, y) <= 0.0;
            if outside {
                assert_eq!(img.data[i].map(f32::to_bits), out.data[i].map(f32::to_bits), "pixel {x},{y} is outside every mask");
            } else if img.data[i] != out.data[i] {
                changed += 1;
            }
        }
    }
    assert!(changed > 500, "the lines inside the masks are lifted: {changed}");
}

#[test]
fn a_face_without_the_mesh_or_the_masks_is_left_alone() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let r = wrinkles(100.0, 100.0, 100.0, 100.0);
    // YuNet only: five points, no masks
    let mut yunet = meshed(FACE);
    yunet.landmarks.truncate(5);
    yunet.masks.clear();
    // the mesh, no masks
    let mut bare = meshed(FACE);
    bare.masks.clear();
    // masks, five points
    let mut five = meshed(FACE);
    five.landmarks.truncate(5);
    for face in [yunet, bare, five] {
        assert!(same_bits(&img, &retouched(&img, &r, vec![face])));
    }
}

/// The pixels each slider changes, as a fraction of the face's box: (top, bottom) of the changed rows.
fn changed_rows(r: &Retouch) -> Option<(f64, f64)> {
    let (w, h) = (320, 400);
    // fine lines all over the skin, every 7 px
    let img = Rgb32f::from_fn(w, h, |x, y| SKIN.map(|c| c * (1.0 - 0.25 * ((y % 7 == 0) as u8 as f32)) * (1.0 + 0.02 * noise(x, y))));
    let out = retouched(&img, r, vec![meshed(FACE)]);
    let rows: Vec<usize> = (0..h).filter(|y| (0..w).any(|x| img.data[y * w + x] != out.data[y * w + x])).collect();
    let face_y = |y: usize| (y as f64 / h as f64 - FACE[1]) / FACE[3];
    Some((face_y(*rows.first()?), face_y(*rows.last()?)))
}

#[test]
fn each_slider_works_on_its_own_area() {
    let (top, bottom) = changed_rows(&wrinkles(100.0, 0.0, 0.0, 0.0)).unwrap_or((9.0, 9.0));
    assert!(top > -0.25 && bottom < 0.5, "Forehead: above the brows and between them ({top:.2}…{bottom:.2})");
    let (top, bottom) = changed_rows(&wrinkles(0.0, 100.0, 0.0, 0.0)).unwrap_or((9.0, 9.0));
    assert!(top > 0.3 && bottom < 0.62, "Crow's Feet: beside and under the eyes ({top:.2}…{bottom:.2})");
    let (top, bottom) = changed_rows(&wrinkles(0.0, 0.0, 100.0, 0.0)).unwrap_or((9.0, 9.0));
    assert!(top > 0.4 && bottom < 1.06, "Cheeks and Smile Lines: under the eyes to the chin ({top:.2}…{bottom:.2})");
    let (top, bottom) = changed_rows(&wrinkles(0.0, 0.0, 0.0, 100.0)).unwrap_or((9.0, 9.0));
    assert!(top > 0.95 && bottom < 1.7, "Neck: under the jaw, on Body Skin ({top:.2}…{bottom:.2})");
}

#[test]
fn every_area_lies_on_its_part_and_the_neck_reads_body_skin() {
    assert_eq!(Wrinkles::PARTS, &[Part::FacialSkin, Part::BodySkin]);
    for area in Area::ALL {
        assert_eq!(area.part(), if area == Area::Neck { Part::BodySkin } else { Part::FacialSkin });
    }
    // without Body Skin the neck is left alone
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let mut face = meshed(FACE);
    face.masks.retain(|(p, _)| *p != Part::BodySkin);
    let neck_only = wrinkles(0.0, 0.0, 0.0, 100.0);
    assert!(same_bits(&img, &retouched(&img, &neck_only, vec![face])));
}

#[test]
fn no_halo_next_to_dark_hair() {
    let (w, h) = (320, 400);
    let hair = [0.02, 0.025, 0.06];
    // dark blue-black hair over the top of the forehead, outside the Facial Skin mask; skin with lines below it
    let edge = 40 + 30;
    let img = Rgb32f::from_fn(w, h, |x, y| if y < edge { hair } else { forehead_lines(w, h, 1.0, 0.03).data[y * w + x] });
    let mut face = meshed(FACE);
    face.masks.retain(|(p, _)| *p != Part::FacialSkin);
    let below = NormRect::new(FACE[0], edge as f64 / h as f64, FACE[0] + FACE[2], FACE[1] + FACE[3]);
    face.masks.push((Part::FacialSkin, PartMask { plane: Plane::filled(8, 8, 1.0), area: below }));
    let out = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![face]);
    let blue = |p: [f32; 3]| p[2] / p[0];
    for y in edge..edge + 6 {
        for x in 110..210 {
            let (a, b) = (img.data[y * w + x], out.data[y * w + x]);
            assert!(luma(b) >= luma(a) * 0.999, "skin by the hair is not darkened at {x},{y}");
            assert!(blue(b) <= blue(a) * 1.02, "skin by the hair does not take its colour at {x},{y}: {a:?} → {b:?}");
        }
    }
}

#[test]
fn a_face_cut_by_the_edge_is_retouched_inside_the_photo() {
    let img = forehead_lines(320, 400, 1.0, 0.03);
    // the face's left third and the forehead's top past the photo's edge
    let face = meshed([-60.0 / 320.0, -30.0 / 400.0, FACE[2], FACE[3]]);
    let out = retouched(&img, &wrinkles(100.0, 100.0, 100.0, 100.0), vec![face]);
    assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    assert!(!same_bits(&img, &out), "the part inside the photo is retouched");
}

#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let r = wrinkles(70.0, 0.0, 0.0, 0.0);
    let small = forehead_lines(320, 400, 1.0, 0.0);
    let big = forehead_lines(1280, 1600, 4.0, 0.0);
    let (a, b) = (line_depth(&retouched(&small, &r, vec![meshed(FACE)]), 1.0), line_depth(&retouched(&big, &r, vec![meshed(FACE)]), 4.0));
    let (a0, b0) = (line_depth(&small, 1.0), line_depth(&big, 4.0));
    let (ka, kb) = (a / a0, b / b0);
    assert!((ka - kb).abs() < 0.15, "the same share of the lines is left: draft {ka:.3}, export {kb:.3}");
}

/// A window of a `full_w × full_h` render, from pixel (`x`, `y`).
struct Window {
    x: usize,
    y: usize,
    full_w: usize,
    full_h: usize,
}

impl View for Window {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        ((x + self.x as f64) / self.full_w as f64, (y + self.y as f64) / self.full_h as f64)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        Some((nx * self.full_w as f64 - self.x as f64, ny * self.full_h as f64 - self.y as f64))
    }
}

#[test]
fn a_window_grown_by_the_reach_equals_the_whole_render_inside_it() {
    let (w, h) = (320, 400);
    let img = forehead_lines(w, h, 1.0, 0.05);
    let r = wrinkles(100.0, 80.0, 60.0, 40.0);
    let whole = retouched(&img, &r, vec![meshed(FACE)]);
    let reach = (Wrinkles::REACH * 200.0).ceil() as usize + 4;
    assert!(reach > 4, "the feature declares its reach");
    // windows holding part of the face: the forehead's right half, an eye, a mouth corner and the neck
    for (x, y, ww, wh) in [(160usize, 40usize, 100usize, 70usize), (60, 110, 70, 60), (120, 170, 50, 50), (100, 230, 120, 80)] {
        let (gx, gy) = (x.saturating_sub(reach), y.saturating_sub(reach));
        let (gw, gh) = ((x + ww + reach).min(w) - gx, (y + wh + reach).min(h) - gy);
        let part = Rgb32f::from_fn(gw, gh, |px, py| img.data[(gy + py) * w + gx + px]);
        let view = Window { x: gx, y: gy, full_w: w, full_h: h };
        let out = retouched_in(&part, &r, vec![meshed(FACE)], &view);
        let mut changed = 0;
        for yy in y..y + wh {
            for xx in x..x + ww {
                let (a, b) = (whole.data[yy * w + xx], out.data[(yy - gy) * gw + xx - gx]);
                assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() <= 1e-4 * p.abs().max(1e-3)), "window {x},{y}: pixel {xx},{yy} {a:?} ≠ {b:?}");
                changed += usize::from(a != img.data[yy * w + xx]);
            }
        }
        assert!(changed > 0, "window {x},{y} holds retouched pixels");
    }
}

#[test]
fn the_reach_is_declared_and_covers_every_area() {
    assert!(Wrinkles::REACH >= 6.0 * Area::ALL.iter().map(|a| a.coarse()).fold(0.0, f64::max));
    assert_eq!(wrinkles(0.0, 0.0, 0.0, 0.0).reads(), None, "an off block reads nothing");
    let reads = wrinkles(0.0, 0.0, 0.0, 10.0).reads();
    assert_eq!(reads.as_ref().map(|r| r.reach), Some(Wrinkles::REACH));
    assert!(reads.is_some_and(|r| r.parts.contains(&Part::FacialSkin) && r.parts.contains(&Part::BodySkin)), "the skin masks it cuts by");
}

#[test]
fn a_face_with_its_own_settings_is_the_only_one_retouched() {
    let (w, h) = (640, 400);
    let one = forehead_lines(320, 400, 1.0, 0.0);
    let img = Rgb32f::from_fn(w, h, |x, y| one.data[y * 320 + x % 320]);
    let half = |b: [f64; 4], dx: f64| meshed([b[0] / 2.0 + dx, b[1], b[2] / 2.0, b[3]]);
    let faces = vec![half(FACE, 0.0), half(FACE, 0.5)];
    let mut own = Retouch::default();
    assert!(own.override_mut(&faces, 1).is_some_and(|f| f.set("retouch.wrinkles.forehead", 100.0)));
    let out = retouched(&img, &own, faces.clone());
    let left_same = (0..h).all(|y| (0..320).all(|x| img.data[y * w + x] == out.data[y * w + x]));
    let right_changed = (0..h).any(|y| (320..640).any(|x| img.data[y * w + x] != out.data[y * w + x]));
    assert!(left_same && right_changed);
}

/// A face whose areas' box is past the most pixels Wrinkles works on at full size (a face filling a very large
/// export) is worked on reduced and the change carried back: never skipped, the lines lifted about as much as at
/// full size, and the pixels outside the masks keep their bits.
#[test]
fn a_face_too_big_for_full_size_work_is_retouched_reduced_not_skipped() {
    let (w, h) = (640, 800);
    let img = forehead_lines(w, h, 2.0, 0.03);
    let face = meshed(FACE);
    let input = Input::new(vec![face.clone()]);
    let view = WholePhoto { width: w, height: h };
    let scene = lightcraft_retouch::Scene::new(&input, &view, w, h);
    let s = wrinkles(80.0, 50.0, 50.0, 50.0).global.wrinkles;
    let mut full = img.clone();
    s.apply_capped(&mut full, &scene, Some(0), usize::MAX);
    let left = |out: &Rgb32f| line_depth(out, 2.0) / line_depth(&img, 2.0);
    let skin_at = |x: usize, y: usize| {
        let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        [Part::FacialSkin, Part::BodySkin].iter().filter_map(|p| face.mask(*p)).map(|m| m.sample(nx, ny)).fold(0.0f32, f32::max)
    };
    for max in [w * h / 4, w * h / 9] {
        let mut capped = img.clone();
        s.apply_capped(&mut capped, &scene, Some(0), max);
        assert!(!same_bits(&capped, &img), "cap {max}: the face is retouched");
        let (kf, kc) = (left(&full), left(&capped));
        assert!(kf < 0.6 && (kf - kc).abs() < 0.15, "cap {max}: the share of the lines left, full size {kf:.3}, reduced {kc:.3}");
        for y in 0..h {
            for x in 0..w {
                if skin_at(x, y) == 0.0 {
                    let i = y * w + x;
                    assert_eq!(capped.data[i].map(f32::to_bits), img.data[i].map(f32::to_bits), "({x}, {y}) is outside the masks");
                }
            }
        }
    }
}
