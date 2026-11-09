//! Tests of the eyes feature (`lightcraft_retouch::eyes`).

mod common;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::eyes::Eyes;
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

#[test]
fn off_by_default_and_amount_zero_changes_nothing() {
    common::off_changes_nothing::<Eyes>();
}

#[test]
fn no_panic_without_faces_with_many_faces_or_a_one_pixel_photo() {
    common::never_panics::<Eyes>();
}

const SKIN: [f32; 3] = [0.5, 0.36, 0.28];
const WHITES: [f32; 3] = [0.62, 0.5, 0.4];
const IRIS: [f32; 3] = [0.12, 0.08, 0.05];
/// The eyes' centres, in fractions of the photo, and their half width as a share of the photo's width (YuNet's
/// points: the half width is 0.22 of the eyes' spacing).
const EYES: [(f64, f64); 2] = [(0.38, 0.4), (0.62, 0.4)];
const HALF: f64 = 0.22 * 0.24;

/// Inside the ellipse of half axes `rx`, `ry` (photo fractions) around `c`, softened over `soft` of its radius.
fn ellipse(nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64, soft: f64) -> f32 {
    let d = (((nx - c.0) / rx).powi(2) + ((ny - c.1) / ry).powi(2)).sqrt();
    (((1.0 + soft) - d) / (2.0 * soft)).clamp(0.0, 1.0) as f32
}

fn whites_at(nx: f64, ny: f64) -> f32 {
    EYES.iter().map(|c| ellipse(nx, ny, *c, HALF * 0.95, HALF * 0.45, 0.1)).fold(0.0, f32::max)
}

fn iris_at(nx: f64, ny: f64) -> f32 {
    EYES.iter().map(|c| ellipse(nx, ny, *c, HALF * 0.4, HALF * 0.4 * 4.0 / 3.0, 0.1)).fold(0.0, f32::max)
}

/// How dark the dark circle under each eye is (0…1), smooth.
fn circle_at(nx: f64, ny: f64) -> f32 {
    EYES.iter().map(|c| ellipse(nx, ny, (c.0, c.1 + HALF * 0.75), HALF * 0.9, HALF * 0.4 * 4.0 / 3.0, 0.6)).fold(0.0, f32::max)
}

/// A face: skin, a bluish dark circle under each eye, yellowish whites with a red vein, dark irises.
fn colour_at(nx: f64, ny: f64) -> [f32; 3] {
    let dc = circle_at(nx, ny);
    let skin = [SKIN[0] * (1.0 - 0.35 * dc), SKIN[1] * (1.0 - 0.4 * dc), SKIN[2] * (1.0 - 0.2 * dc)];
    let (w, i) = (whites_at(nx, ny), iris_at(nx, ny));
    let vein = if (ny - EYES[0].1).abs() < HALF * 0.05 { 0.7 } else { 1.0 };
    let white = [WHITES[0], WHITES[1] * vein, WHITES[2] * vein];
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
    mix(mix(skin, white, w), IRIS, i)
}

fn picture(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| colour_at((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64))
}

/// A part mask drawn over the face's box at 128 × 128 (as a parser's are: their own resolution).
fn mask(area: NormRect, f: impl Fn(f64, f64) -> f32) -> PartMask {
    let n = 128;
    let plane =
        Plane::from_fn(n, n, |x, y| f(area.x0 + (x as f64 + 0.5) / n as f64 * area.width(), area.y0 + (y as f64 + 0.5) / n as f64 * area.height()));
    PartMask { plane, area }
}

/// The face, with YuNet's five landmarks and the masks Eyes reads.
fn face(parts: &[Part]) -> Face {
    let bounds = NormRect::new(0.2, 0.1, 0.8, 0.9);
    let all = [
        (Part::FacialSkin, mask(bounds, |x, y| 1.0 - whites_at(x, y).max(iris_at(x, y)))),
        (Part::EyeWhites, mask(bounds, |x, y| whites_at(x, y) * (1.0 - iris_at(x, y)))),
        (Part::Iris, mask(bounds, iris_at)),
    ];
    Face {
        landmarks: vec![EYES[0], EYES[1], (0.5, 0.55), (0.42, 0.7), (0.58, 0.7)],
        masks: all.into_iter().filter(|(p, _)| parts.contains(p)).collect(),
        ..Face::boxed(bounds)
    }
}

fn full() -> Face {
    face(&[Part::FacialSkin, Part::EyeWhites, Part::Iris])
}

fn eyes(whites: f64, iris: f64, dark: f64, bags: f64) -> Retouch {
    let mut r = Retouch::default();
    for (id, v) in [("whites", whites), ("iris", iris), ("darkCircles", dark), ("eyeBags", bags)] {
        assert!(r.global.set(&format!("retouch.eyes.{id}"), v));
    }
    r
}

fn retouched(img: &Rgb32f, r: &Retouch, faces: Vec<Face>) -> Rgb32f {
    let mut out = img.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    lightcraft_retouch::run::apply(&mut out, r, &Input::new(faces), &view, Stage::Retouch);
    out
}

fn same(a: [f32; 3], b: [f32; 3]) -> bool {
    a.map(f32::to_bits) == b.map(f32::to_bits)
}

fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}

fn chroma(p: [f32; 3]) -> f32 {
    p.iter().fold(0.0f32, |a, v| a.max(*v)) - p.iter().fold(f32::MAX, |a, v| a.min(*v))
}

const W: usize = 400;
const H: usize = 300;

fn at(img: &Rgb32f, nx: f64, ny: f64) -> [f32; 3] {
    let (x, y) = ((nx * img.width as f64) as usize, (ny * img.height as f64) as usize);
    img.data[y * img.width + x]
}

/// One slider on leaves what the others would change alone.
#[test]
fn each_slider_keeps_to_its_own_area() {
    let img = picture(W, H);
    let under = (EYES[0].0, EYES[0].1 + HALF * 0.75);
    for (r, untouched) in [(eyes(80.0, 0.0, 0.0, 0.0), under), (eyes(0.0, 80.0, 0.0, 0.0), under), (eyes(0.0, 0.0, 80.0, 80.0), EYES[0])] {
        let out = retouched(&img, &r, vec![full()]);
        assert!(same(at(&out, untouched.0, untouched.1), at(&img, untouched.0, untouched.1)), "{r:?} changed {untouched:?}");
    }
}

#[test]
fn whites_get_brighter_and_lose_their_colour_veins_fade() {
    let img = picture(W, H);
    let out = retouched(&img, &eyes(100.0, 0.0, 0.0, 0.0), vec![full()]);
    let p = (EYES[0].0 - HALF * 0.6, EYES[0].1 - HALF * 0.2);
    let (a, b) = (at(&img, p.0, p.1), at(&out, p.0, p.1));
    assert!(luma(b) > luma(a) * 1.2 && chroma(b) < chroma(a) * 0.5, "{a:?} → {b:?}");
    // the vein is lighter against the white than it was, and still there
    let v = (EYES[0].0 - HALF * 0.6, EYES[0].1);
    let (va, vb) = (luma(at(&img, v.0, v.1)) / luma(a), luma(at(&out, v.0, v.1)) / luma(b));
    assert!(vb > va && vb < 1.0, "vein against the white: {va} → {vb}");
    // the iris and the skin are as they were
    for q in [EYES[0], (EYES[0].0, EYES[0].1 + HALF), (0.5, 0.7)] {
        assert!(same(at(&img, q.0, q.1), at(&out, q.0, q.1)), "{q:?} changed");
    }
}

#[test]
fn iris_gets_brighter() {
    let img = picture(W, H);
    let out = retouched(&img, &eyes(0.0, 100.0, 0.0, 0.0), vec![full()]);
    let (a, b) = (at(&img, EYES[1].0, EYES[1].1), at(&out, EYES[1].0, EYES[1].1));
    assert!(luma(b) > luma(a) * 1.3, "{a:?} → {b:?}");
}

#[test]
fn dark_circles_lift_toward_the_cheek_and_keep_the_skin_elsewhere() {
    let img = picture(W, H);
    let out = retouched(&img, &eyes(0.0, 0.0, 100.0, 0.0), vec![full()]);
    for c in EYES {
        let p = (c.0, c.1 + HALF * 0.75);
        let (a, b) = (at(&img, p.0, p.1), at(&out, p.0, p.1));
        let gap = |q: [f32; 3]| (luma(SKIN) - luma(q)).abs();
        assert!(luma(b) > luma(a) && gap(b) < gap(a) * 0.6, "{a:?} → {b:?}");
        // and its colour toward the skin's
        let hue = |q: [f32; 3]| {
            let (n, m) = (q.iter().sum::<f32>(), SKIN.iter().sum::<f32>());
            (0..3).map(|k| (q[k] / n - SKIN[k] / m).powi(2)).sum::<f32>().sqrt()
        };
        assert!(hue(b) < hue(a), "{} → {}", hue(a), hue(b));
    }
}

#[test]
fn eye_bags_flatten_the_shading_under_the_eye() {
    let img = picture(W, H);
    let out = retouched(&img, &eyes(0.0, 0.0, 0.0, 100.0), vec![full()]);
    // the shading's depth: the dark circle's centre against the skin beside it
    let depth = |im: &Rgb32f| luma(at(im, EYES[0].0, EYES[0].1 + 2.2 * HALF)) - luma(at(im, EYES[0].0, EYES[0].1 + 0.75 * HALF));
    assert!(depth(&out) < depth(&img) * 0.75, "{} → {}", depth(&img), depth(&out));
}

/// Every pixel away from the eyes (outside a box from just above the eyes to below the eye bags) keeps its bits.
#[test]
fn nothing_outside_the_eye_areas_changes() {
    let img = picture(W, H);
    let out = retouched(&img, &eyes(100.0, 100.0, 100.0, 100.0), vec![full()]);
    let (mut changed, mut kept) = (0, 0);
    for y in 0..H {
        for x in 0..W {
            let (nx, ny) = ((x as f64 + 0.5) / W as f64, (y as f64 + 0.5) / H as f64);
            let near = EYES.iter().any(|c| (nx - c.0).abs() < 1.6 * HALF && ny > c.1 - 0.6 * HALF * 4.0 / 3.0 && ny < c.1 + 2.0 * HALF * 4.0 / 3.0);
            let (a, b) = (img.data[y * W + x], out.data[y * W + x]);
            if !near {
                assert!(same(a, b), "({x}, {y}) is away from the eyes and changed: {a:?} → {b:?}");
                kept += 1;
            } else if !same(a, b) {
                changed += 1;
            }
        }
    }
    assert!(changed > 1000 && kept > 50_000, "{changed} changed, {kept} kept");
}

/// A dark line on even skin under the eye (a lash, a frame's edge): the skin beside it changes no more than the
/// skin away from it, so no light or dark rim follows the line.
#[test]
fn no_halo_beside_a_dark_line() {
    let line = |nx: f64, ny: f64| (ny - (EYES[0].1 + HALF * 0.9)).abs() < 0.004 && (nx - EYES[0].0).abs() < HALF * 0.8;
    let img = Rgb32f::from_fn(W, H, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / W as f64, (y as f64 + 0.5) / H as f64);
        if line(nx, ny) { [0.02, 0.015, 0.012] } else { SKIN }
    });
    let mut worst = 0.0f32;
    for r in [eyes(0.0, 0.0, 100.0, 0.0), eyes(0.0, 0.0, 0.0, 100.0), eyes(0.0, 0.0, 100.0, 100.0)] {
        let out = retouched(&img, &r, vec![face(&[Part::FacialSkin])]);
        for (i, (a, b)) in img.data.iter().zip(&out.data).enumerate() {
            let (nx, ny) = (((i % W) as f64 + 0.5) / W as f64, ((i / W) as f64 + 0.5) / H as f64);
            let d = (luma(*b) / luma(*a) - 1.0).abs();
            if !line(nx, ny) {
                worst = worst.max(d);
            }
        }
    }
    assert!(worst < 0.02, "skin beside the line changed by {worst}");
}

/// A draft and an export four times its size agree once the export is scaled down (sizes follow the face).
#[test]
fn a_draft_and_a_bigger_export_agree_after_scaling() {
    let r = eyes(70.0, 70.0, 70.0, 70.0);
    let (w, h) = (200, 150);
    let draft = retouched(&picture(w, h), &r, vec![full()]);
    let big = retouched(&picture(w * 4, h * 4), &r, vec![full()]);
    let (mut sum, mut n, mut worst) = (0.0f32, 0, 0.0f32);
    for y in 0..h {
        for x in 0..w {
            let mut avg = [0.0f32; 3];
            let mut src = [0.0f32; 3];
            for dy in 0..4 {
                for dx in 0..4 {
                    let i = (y * 4 + dy) * w * 4 + x * 4 + dx;
                    for k in 0..3 {
                        avg[k] += big.data[i][k] / 16.0;
                    }
                }
            }
            let d = draft.data[y * w + x];
            for k in 0..3 {
                src[k] = (avg[k] - d[k]).abs();
            }
            let e = src.iter().fold(0.0f32, |a, v| a.max(*v));
            sum += e;
            n += 1;
            worst = worst.max(e);
        }
    }
    let mean = sum / n as f32;
    // (the eyes' edges are resolved better in the export than one draft pixel averages them)
    assert!(mean < 0.002 && worst < 0.12, "mean difference {mean}, worst {worst}");
}

/// Closed eyes (no whites, no iris): Whites and Iris leave the photo alone; the under-eye sliders still work.
#[test]
fn closed_eyes_change_only_under_the_eye() {
    let img = picture(W, H);
    let closed = face(&[Part::FacialSkin]);
    let out = retouched(&img, &eyes(100.0, 100.0, 0.0, 0.0), vec![closed.clone()]);
    assert!(out.data.iter().zip(&img.data).all(|(a, b)| same(*a, *b)));
    let out = retouched(&img, &eyes(0.0, 0.0, 100.0, 0.0), vec![closed]);
    let p = (EYES[0].0, EYES[0].1 + HALF * 0.75);
    assert!(luma(at(&out, p.0, p.1)) > luma(at(&img, p.0, p.1)));
}

/// Glasses: the parser takes the lenses out of Facial Skin and puts them in Others. Skin seen through them is
/// still lifted; a dark frame is not.
#[test]
fn glasses_skin_through_the_lens_is_retouched_and_the_frame_is_not() {
    let frame = |_: f64, ny: f64| (ny - (EYES[0].1 + HALF * 1.6)).abs() < 0.006;
    let img = Rgb32f::from_fn(W, H, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / W as f64, (y as f64 + 0.5) / H as f64);
        if frame(nx, ny) { [0.03, 0.03, 0.03] } else { colour_at(nx, ny) }
    });
    let lens = |nx: f64, ny: f64| EYES.iter().map(|c| ellipse(nx, ny, (c.0, c.1 + HALF * 0.3), HALF * 1.9, HALF * 2.0, 0.05)).fold(0.0, f32::max);
    let mut glasses = full();
    let bounds = glasses.bounds;
    glasses.masks = vec![
        (Part::FacialSkin, mask(bounds, |x, y| (1.0 - lens(x, y)) * (1.0 - whites_at(x, y).max(iris_at(x, y))))),
        (Part::Others, mask(bounds, |x, y| lens(x, y) * (1.0 - whites_at(x, y).max(iris_at(x, y))))),
        (Part::EyeWhites, mask(bounds, |x, y| whites_at(x, y) * (1.0 - iris_at(x, y)))),
    ];
    let out = retouched(&img, &eyes(0.0, 0.0, 100.0, 0.0), vec![glasses]);
    let p = (EYES[0].0, EYES[0].1 + HALF * 0.75);
    assert!(luma(at(&out, p.0, p.1)) > luma(at(&img, p.0, p.1)) * 1.1, "skin through the lens not lifted");
    for (i, (a, b)) in img.data.iter().zip(&out.data).enumerate() {
        let (nx, ny) = (((i % W) as f64 + 0.5) / W as f64, ((i / W) as f64 + 0.5) / H as f64);
        if frame(nx, ny) {
            assert!(same(*a, *b), "the frame changed at {nx}, {ny}");
        }
    }
}

/// No faces, a face without masks or landmarks (no face model), a face cut by the edge, a 1 × 1 and a flat grey
/// photo: no panic, nothing that is not a number, and the part of a cut face inside the photo still retouched.
#[test]
fn odd_inputs() {
    let r = eyes(100.0, 100.0, 100.0, 100.0);
    let img = picture(W, H);
    for faces in [Vec::new(), vec![Face::boxed(NormRect::new(0.2, 0.1, 0.8, 0.9))]] {
        assert!(retouched(&img, &r, faces).data.iter().zip(&img.data).all(|(a, b)| same(*a, *b)));
    }
    // the photo's top-left quarter: the left eye and part of its under-eye are inside, the rest is cut away
    let cut = Rgb32f::from_fn(W / 2, H / 2, |x, y| img.data[y * W + x]);
    let mut shifted = full();
    shifted.bounds = NormRect::new(0.4, 0.2, 1.6, 1.8);
    shifted.landmarks = shifted.landmarks.iter().map(|(x, y)| (x * 2.0, y * 2.0)).collect();
    for (_, m) in &mut shifted.masks {
        m.area = NormRect::new(m.area.x0 * 2.0, m.area.y0 * 2.0, m.area.x1 * 2.0, m.area.y1 * 2.0);
    }
    let out = retouched(&cut, &r, vec![shifted]);
    assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    assert!(out.data.iter().zip(&cut.data).any(|(a, b)| !same(*a, *b)), "the cut face's visible eye was not retouched");
    for (w, h) in [(1, 1), (2, 1)] {
        let tiny = Rgb32f::from_fn(w, h, |_, _| [0.4, 0.3, 0.2]);
        let out = retouched(&tiny, &r, vec![full()]);
        assert!(out.data.iter().flatten().all(|v| v.is_finite()));
    }
    let grey = Rgb32f::from_fn(W, H, |_, _| [0.18; 3]);
    let out = retouched(&grey, &r, vec![full()]);
    assert!(out.data.iter().flatten().all(|v| v.is_finite() && *v >= 0.0));
    // flat grey has no dark circle and no shading: under the eyes it stays grey
    let p = at(&out, EYES[0].0, EYES[0].1 + HALF);
    assert!((luma(p) - 0.18).abs() < 0.01, "{p:?}");
}
