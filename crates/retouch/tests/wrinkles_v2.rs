//! Tests of Wrinkles' stored change (`lightcraft_retouch::wrinkles::with_fixes`, `Face::wrinkles`): the search once per
//! photo, and the sliders scaling what it kept.

use std::sync::Arc;

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::wrinkles::{Fill, Filler, Fix, NoFill, areas, with_fixes};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, View, WholePhoto};

/// The face's box in the test pictures (x0, y0, width, height, as fractions of a 320 × 400 picture): 200 × 200 px.
const FACE: [f64; 4] = [60.0 / 320.0, 40.0 / 400.0, 200.0 / 320.0, 200.0 / 400.0];
const SKIN: [f32; 3] = [0.45, 0.3, 0.24];
/// The forehead's lines, at these heights of the face's box; a line's half-width (fraction of the face) and depth.
const LINES: [f64; 3] = [0.12, 0.18, 0.24];
const LINE_SIGMA: f64 = 0.007;
const LINE_DEPTH: f32 = 0.3;

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

fn noise(x: usize, y: usize) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h % 2001) as f32 / 1000.0 - 1.0
}

/// Pores: a value noise (−1…1) on a lattice `cell` pixels apart, read bilinearly.
fn pore(x: usize, y: usize, cell: f32) -> f32 {
    let (fx, fy) = (x as f32 / cell, y as f32 / cell);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let n = |i: f32, j: f32| noise(i as usize, j as usize);
    let (a, b) = (n(x0, y0) * (1.0 - tx) + n(x0 + 1.0, y0) * tx, n(x0, y0 + 1.0) * (1.0 - tx) + n(x0 + 1.0, y0 + 1.0) * tx);
    a * (1.0 - ty) + b * ty
}

fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}

/// Skin over the picture with three dark lines across the forehead of [`FACE`] and pores (±`pores`).
fn forehead_lines(w: usize, h: usize, scale: f64, pores: f32) -> Rgb32f {
    let (fx, fy, fs) = (FACE[0] * w as f64, FACE[1] * h as f64, 200.0 * scale);
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (((x as f64 + 0.5) - fx) / fs, ((y as f64 + 0.5) - fy) / fs);
        let line: f64 = if (0.25..0.75).contains(&u) { LINES.iter().map(|l| (-0.5 * ((v - l) / LINE_SIGMA).powi(2)).exp()).sum() } else { 0.0 };
        let k = (1.0 - LINE_DEPTH * line as f32) * (1.0 + pores * noise(x, y));
        SKIN.map(|c| c * k)
    })
}

/// How dark the lines are against the skin between them (a fraction of the skin's luminance).
fn line_depth(img: &Rgb32f) -> f64 {
    let (fx, fy, fs) = (FACE[0] * img.width as f64, FACE[1] * img.height as f64, 200.0);
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

/// The face searched on `img` with no model (the banded heal).
fn searched(img: &Rgb32f) -> Face {
    let mut faces = with_fixes(img, vec![meshed(FACE)], &mut NoFill);
    faces.remove(0)
}

fn fix_of(face: &Face) -> Arc<Fix> {
    face.wrinkles.clone().expect("the face was searched")
}

#[test]
fn a_search_finds_the_lines_and_keeps_a_change_with_the_face() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let face = searched(&img);
    let fix = fix_of(&face);
    assert!(!fix.is_empty(), "the lines' change is kept");
    assert!(fix.area.is_valid() && fix.width > 0 && fix.height > 0 && fix.width <= 768 && fix.height <= 768);
    // a pixel on the middle line is lifted (the picture is darker there), the skin between the lines is not
    let at = |x: usize, y: usize| fix.sample((x as f64 + 0.5) / 320.0, (y as f64 + 0.5) / 400.0);
    let on_line = at(160, (40.0 + 0.18 * 200.0) as usize);
    assert!(on_line.iter().all(|v| *v > 0.1), "the line is lifted: {on_line:?}");
    let between = at(160, (40.0 + 0.30 * 200.0) as usize);
    assert!(between.iter().all(|v| v.abs() < 0.02), "skin between the lines is not changed: {between:?}");
}

#[test]
fn slider_zero_gives_the_original_bytes_and_a_slider_scales_its_area_only() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let face = searched(&img);
    // every slider at 0, and the other areas' sliders with Forehead at 0: no bits change
    assert!(same_bits(&img, &retouched(&img, &Retouch::default(), vec![face.clone()])));
    assert!(same_bits(&img, &retouched(&img, &wrinkles(0.0, 0.0, 0.0, 0.0), vec![face.clone()])));
    assert!(same_bits(&img, &retouched(&img, &wrinkles(0.0, 0.0, 100.0, 100.0), vec![face.clone()])), "the forehead's lines wait for Forehead");
    // the forehead's lines move with Forehead
    let out = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![face]);
    assert!(!same_bits(&img, &out));
}

#[test]
fn at_100_a_pixel_has_the_stored_change_and_at_50_half_of_it() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let face = searched(&img);
    let fix = fix_of(&face);
    let (x, y) = (160usize, (40.0 + 0.18 * 200.0) as usize);
    let d = fix.sample((x as f64 + 0.5) / 320.0, (y as f64 + 0.5) / 400.0);
    assert!(d.iter().all(|v| *v > 0.1), "{d:?}");
    for (slider, k) in [(100.0, 1.0f32), (50.0, 0.5)] {
        let out = retouched(&img, &wrinkles(slider, 0.0, 0.0, 0.0), vec![face.clone()]);
        let (a, b) = (img.data[y * 320 + x], out.data[y * 320 + x]);
        for c in 0..3 {
            let want = a[c] * (d[c] * k).exp();
            assert!((b[c] - want).abs() <= 2e-4 * want, "slider {slider}, channel {c}: {} ≠ {want}", b[c]);
        }
    }
}

#[test]
fn forehead_100_takes_away_the_lines() {
    let img = forehead_lines(320, 400, 1.0, 0.0);
    let face = searched(&img);
    let out = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![face.clone()]);
    let (before, after) = (line_depth(&img), line_depth(&out));
    assert!(before > 0.2, "the lines are there: {before}");
    assert!(after < before * 0.25, "Forehead 100 takes away most of the lines: {before:.3} → {after:.3}");
    let half = line_depth(&retouched(&img, &wrinkles(50.0, 0.0, 0.0, 0.0), vec![face]));
    assert!(half < before && half > after, "{before:.3} {half:.3} {after:.3}");
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

/// A face 320 px across with pores a couple of pixels across: the fine band of the search comes back from the skin
/// nearby, so the pores inside the lifted lines are still there.
#[test]
fn the_pores_come_back_inside_the_lifted_lines() {
    let (w, h, k) = (512, 640, 1.6);
    let lined = forehead_lines(w, h, k, 0.0);
    // pores over the lines too: a darkening by 4 % of a value noise two pixels across
    let img = Rgb32f::from_fn(w, h, |x, y| lined.data[y * w + x].map(|c| c * (1.0 + 0.04 * pore(x, y, 2.0))));
    let face = with_fixes(&img, vec![meshed(FACE)], &mut NoFill).remove(0);
    let out = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![face]);
    // the energy finer than 3 px, over the whole middle of the forehead, on the lines too
    let energy = |im: &Rgb32f| {
        let y = Plane::from_fn(w, h, |x, y| luma(im.data[y * w + x]).ln());
        let b = gaussian(&y, 3.0);
        let (fx, fy) = (FACE[0] * w as f64, FACE[1] * h as f64);
        let mut e = 0.0;
        let mut n = 0;
        for yy in (fy + 0.1 * 320.0) as usize..(fy + 0.28 * 320.0) as usize {
            for x in (fx + 0.35 * 320.0) as usize..(fx + 0.65 * 320.0) as usize {
                let d = f64::from(y.data[yy * w + x] - b.data[yy * w + x]);
                e += d * d;
                n += 1;
            }
        }
        e / f64::from(n)
    };
    // (what the same pores have on skin with no lines)
    let skin = Rgb32f::from_fn(w, h, |x, y| SKIN.map(|c| c * (1.0 + 0.04 * pore(x, y, 2.0))));
    let (reference, after) = (energy(&skin), energy(&out));
    assert!(after > 0.6 * reference && after < 1.4 * reference, "the pores of clean skin are there again: {reference:.3e} → {after:.3e}");
}

#[test]
fn a_window_equals_the_whole_render_inside_it() {
    let (w, h) = (320, 400);
    let img = forehead_lines(w, h, 1.0, 0.05);
    let face = searched(&img);
    let r = wrinkles(100.0, 80.0, 60.0, 40.0);
    let whole = retouched(&img, &r, vec![face.clone()]);
    let reach = (lightcraft_retouch::wrinkles::Wrinkles::REACH_OF_FIX * 200.0).ceil() as usize + 4;
    for (x, y, ww, wh) in [(160usize, 40usize, 100usize, 70usize), (60, 110, 70, 60), (120, 170, 50, 50), (100, 230, 120, 80)] {
        let (gx, gy) = (x.saturating_sub(reach), y.saturating_sub(reach));
        let (gw, gh) = ((x + ww + reach).min(w) - gx, (y + wh + reach).min(h) - gy);
        let part = Rgb32f::from_fn(gw, gh, |px, py| img.data[(gy + py) * w + gx + px]);
        let view = Window { x: gx, y: gy, full_w: w, full_h: h };
        let out = retouched_in(&part, &r, vec![face.clone()], &view);
        let mut changed = 0;
        for yy in y..y + wh {
            for xx in x..x + ww {
                let (a, b) = (whole.data[yy * w + xx], out.data[(yy - gy) * gw + xx - gx]);
                assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() <= 1e-4 * p.abs().max(1e-3)), "window {x},{y}: pixel {xx},{yy} {a:?} ≠ {b:?}");
                changed += usize::from(a != img.data[yy * w + xx]);
            }
        }
        if (x, y) == (160, 40) {
            assert!(changed > 0, "the forehead window holds changed pixels");
        }
    }
}

/// A draft and an export four times its size make the same change once scaled (the change is kept in the photo's
/// normalised coordinates, so a render samples it at its own size).
#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let small = forehead_lines(320, 400, 1.0, 0.0);
    let face = searched(&small);
    let big = forehead_lines(1280, 1600, 4.0, 0.0);
    let r = wrinkles(70.0, 0.0, 0.0, 0.0);
    let depth = |img: &Rgb32f, scale: f64| {
        let (fx, fy, fs) = (FACE[0] * img.width as f64, FACE[1] * img.height as f64, 200.0 * scale);
        let row = |v: f64| {
            let y = (fy + v * fs) as usize;
            let xs = (fx + 0.35 * fs) as usize..(fx + 0.65 * fs) as usize;
            let n = xs.len() as f64;
            xs.map(|x| f64::from(luma(img.data[y * img.width + x]))).sum::<f64>() / n
        };
        (row(0.15) + row(0.21)) / 2.0 - LINES.iter().map(|l| row(*l)).sum::<f64>() / LINES.len() as f64
    };
    let ka = depth(&retouched(&small, &r, vec![face.clone()]), 1.0) / depth(&small, 1.0);
    let kb = depth(&retouched(&big, &r, vec![face]), 4.0) / depth(&big, 4.0);
    assert!((ka - kb).abs() < 0.2, "the same share of the lines is left: draft {ka:.3}, export {kb:.3}");
}

/// A filler that gives back the clean skin (the picture without the lines), as LaMa would.
struct Clean(Rgb32f);

impl Filler for Clean {
    fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
        Fill::Done(self.0.clone())
    }
}

struct Say(fn() -> Fill, bool);

impl Filler for Say {
    fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
        (self.0)()
    }

    fn cancelled(&self) -> bool {
        self.1
    }
}

#[test]
fn a_filler_gives_the_low_band_and_without_one_the_banded_heal_stands_in() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let clean = forehead_lines_none(320, 400);
    let with = with_fixes(&img, vec![meshed(FACE)], &mut Clean(clean)).remove(0);
    let fix = fix_of(&with);
    assert!(!fix.is_empty());
    let out = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![with]);
    assert!(line_depth(&out) < line_depth(&img) * 0.25, "a fill from the filler also takes the lines away");
    // no model (or one that failed): the banded heal, the same as NoFill
    let a = with_fixes(&img, vec![meshed(FACE)], &mut Say(|| Fill::Unavailable, false)).remove(0);
    let b = searched(&img);
    assert_eq!(a.wrinkles, b.wrinkles, "Unavailable is the banded heal");
    assert!(!fix_of(&a).is_empty());
}

fn forehead_lines_none(w: usize, h: usize) -> Rgb32f {
    // (the same pores, no lines)
    let lined = forehead_lines(w, h, 1.0, 0.04);
    let (fx, fy, fs) = (FACE[0] * w as f64, FACE[1] * h as f64, 200.0);
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = (((x as f64 + 0.5) - fx) / fs, ((y as f64 + 0.5) - fy) / fs);
        let line: f64 = if (0.25..0.75).contains(&u) { LINES.iter().map(|l| (-0.5 * ((v - l) / LINE_SIGMA).powi(2)).exp()).sum() } else { 0.0 };
        let p = lined.data[y * w + x];
        p.map(|c| c / (1.0 - LINE_DEPTH * line as f32))
    })
}

#[test]
fn a_cancelled_search_keeps_nothing_and_a_face_without_a_search_shows_the_old_wrinkles() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let cancelled = with_fixes(&img, vec![meshed(FACE)], &mut Say(|| Fill::Cancelled, true));
    assert_eq!(cancelled[0].wrinkles, None);
    let stopped_in_the_fill = with_fixes(&img, vec![meshed(FACE)], &mut Say(|| Fill::Cancelled, false));
    assert_eq!(stopped_in_the_fill[0].wrinkles, None);
    // not searched: the old Wrinkles soften the lines (and change pixels)
    let old = retouched(&img, &wrinkles(100.0, 0.0, 0.0, 0.0), vec![meshed(FACE)]);
    assert!(!same_bits(&img, &old));
    assert!(line_depth(&old) < line_depth(&img));
}

#[test]
fn faces_that_cannot_be_searched_get_none_and_a_face_without_wrinkles_gets_an_empty_change() {
    let img = forehead_lines(320, 400, 1.0, 0.04);
    let mut five = meshed(FACE);
    five.landmarks.truncate(5);
    let mut no_skin = meshed(FACE);
    no_skin.masks.clear();
    for f in with_fixes(&img, vec![five, no_skin], &mut NoFill) {
        assert_eq!(f.wrinkles, None);
    }
    let flat = Rgb32f::from_fn(320, 400, |x, y| SKIN.map(|c| c * (1.0 + 0.02 * noise(x, y))));
    let face = with_fixes(&flat, vec![meshed(FACE)], &mut NoFill).remove(0);
    assert!(fix_of(&face).is_empty(), "no wrinkles, no change");
    assert!(same_bits(&flat, &retouched(&flat, &wrinkles(100.0, 100.0, 100.0, 100.0), vec![face])), "and nothing is touched");
}

#[test]
fn no_panic_on_odd_faces_and_pictures() {
    let r = wrinkles(100.0, 100.0, 100.0, 100.0);
    let many: Vec<Face> = (0..20).map(|i| meshed([(i % 10) as f64 / 10.0, (i / 10) as f64 / 3.0, 0.1, 0.2])).collect();
    let mut nan = meshed(FACE);
    nan.landmarks.iter_mut().step_by(3).for_each(|p| *p = (f64::NAN, f64::INFINITY));
    let mut one_point = meshed(FACE);
    one_point.landmarks.iter_mut().for_each(|p| *p = (0.5, 0.5));
    let odd = vec![meshed([-0.3, -0.4, 0.6, 0.6]), meshed([0.8, 0.7, 0.6, 0.6]), nan, one_point, Face::default()];
    for (w, h) in [(64, 48), (1, 1), (0, 0), (3, 1), (320, 400)] {
        for faces in [many.clone(), odd.clone()] {
            let img = Rgb32f::from_fn(w, h, |x, y| SKIN.map(|c| c * (1.0 + 0.02 * noise(x, y))));
            let faces = with_fixes(&img, faces, &mut NoFill);
            let out = retouched(&img, &r, faces);
            assert_eq!((out.width, out.height), (w, h));
            assert!(out.data.iter().flatten().all(|v| v.is_finite()));
        }
    }
}
