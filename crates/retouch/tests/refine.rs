//! The Refine brush (`lightcraft_retouch::refine`, issue #99): Reduce, Restore and Add on every feature, through the
//! step alone.

mod common;

use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::{
    FEATURES, Face, FaceOverride, Features, Input, NormRect, Part, PartMask, Refine, RefineMode, RefineStroke, Retouch, Stage, WholePhoto,
};
use serde_json::json;

fn run(img: &Rgb32f, r: &Retouch, faces: &[Face]) -> Rgb32f {
    let mut out = img.clone();
    let view = WholePhoto { width: img.width, height: img.height };
    let input = Input::new(faces.to_vec());
    for stage in [Stage::Retouch, Stage::Geometry, Stage::Background] {
        lightcraft_retouch::run::apply(&mut out, r, &input, &view, stage);
    }
    out
}

/// A noisy skin-coloured picture (the skin feature smooths it).
fn picture(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let n = ((x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)) % 97) as f32 / 97.0;
        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        [0.45 * (0.8 + 0.3 * n) + 0.05 * u, 0.3 * (0.8 + 0.3 * n), 0.24 * (0.8 + 0.3 * n) + 0.05 * v]
    })
}

/// A face over most of the picture whose Facial Skin mask covers only its left half (`x < 0.5`).
fn half_skin_face() -> Face {
    let left = Plane::from_fn(32, 32, |x, _| if x < 16 { 1.0 } else { 0.0 });
    Face {
        masks: vec![(Part::FacialSkin, PartMask { plane: left, area: NormRect::new(0.1, 0.1, 0.9, 0.9) })],
        landmarks: vec![(0.35, 0.4), (0.65, 0.4), (0.5, 0.55), (0.4, 0.7), (0.6, 0.7)],
        ..Face::boxed(NormRect::new(0.1, 0.1, 0.9, 0.9))
    }
}

fn smooth_skin() -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.skin.smoothness", 100.0));
    r
}

fn stroke(mode: RefineMode, points: &[(f64, f64)], size: f64) -> RefineStroke {
    RefineStroke { mode, points: points.to_vec(), size, feather: 0.0, flow: 100.0, strength: 100.0, pressure: Vec::new() }
}

/// The pixel at normalised (`u`, `v`).
fn at(img: &Rgb32f, u: f64, v: f64) -> [f32; 3] {
    let (x, y) = (((u * img.width as f64) as usize).min(img.width - 1), ((v * img.height as f64) as usize).min(img.height - 1));
    img.data[y * img.width + x]
}

fn same_bits(a: [f32; 3], b: [f32; 3]) -> bool {
    a.map(f32::to_bits) == b.map(f32::to_bits)
}

/// The pixels within `r` (normalised, across) of (`u`, `v`).
fn disc(img: &Rgb32f, u: f64, v: f64, r: f64) -> Vec<(usize, [f32; 3])> {
    let (w, h) = (img.width as f64, img.height as f64);
    img.data
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let (x, y) = (((i % img.width) as f64 + 0.5) / w, ((i / img.width) as f64 + 0.5) / h);
            ((x - u) * w / w.max(h)).powi(2) + ((y - v) * h / w.max(h)).powi(2) <= (r * r)
        })
        .map(|(i, p)| (i, *p))
        .collect()
}

#[test]
fn a_full_reduce_gives_back_the_pixels_from_before_the_feature_there_and_nowhere_else() {
    let img = picture(120, 90);
    let faces = [half_skin_face()];
    let smoothed = run(&img, &smooth_skin(), &faces);
    let mut r = smooth_skin();
    assert!(r.refine.push("skin", stroke(RefineMode::Reduce, &[(0.3, 0.5)], 0.08)));
    let out = run(&img, &r, &faces);
    let inside = disc(&out, 0.3, 0.5, 0.06);
    assert!(!inside.is_empty());
    assert!(inside.iter().all(|(i, p)| same_bits(*p, img.data[*i])), "inside a full Reduce: the pixels from before the feature ran");
    assert!(inside.iter().any(|(i, _)| !same_bits(smoothed.data[*i], img.data[*i])), "the skin feature did change them");
    // away from the stroke the smoothing stays as it was
    for (i, p) in disc(&out, 0.2, 0.2, 0.03) {
        assert!(same_bits(p, smoothed.data[i]));
    }
}

#[test]
fn a_partial_reduce_weakens_and_restore_takes_it_back() {
    let img = picture(120, 90);
    let faces = [half_skin_face()];
    let smoothed = run(&img, &smooth_skin(), &faces);
    let mut r = smooth_skin();
    r.refine.push("skin", RefineStroke { strength: 50.0, ..stroke(RefineMode::Reduce, &[(0.3, 0.5)], 0.08) });
    let half = run(&img, &r, &faces);
    for (i, p) in disc(&half, 0.3, 0.5, 0.06) {
        let (o, s) = (img.data[i], smoothed.data[i]);
        for k in 0..3 {
            assert!((p[k] - (s[k] + (o[k] - s[k]) * 0.5)).abs() < 1e-5, "half way between");
        }
    }
    r.refine.push("skin", stroke(RefineMode::Restore, &[(0.3, 0.5)], 0.1));
    let back = run(&img, &r, &faces);
    assert!(back.data.iter().zip(&smoothed.data).all(|(a, b)| same_bits(*a, *b)), "Restore gives the automatic result back");
}

#[test]
fn add_applies_the_feature_past_its_automatic_mask() {
    let img = picture(120, 90);
    let faces = [half_skin_face()];
    let plain = run(&img, &smooth_skin(), &faces);
    // the right half has no skin mask: smoothing leaves it alone
    assert!(disc(&plain, 0.7, 0.5, 0.05).iter().all(|(i, p)| same_bits(*p, img.data[*i])));
    let mut r = smooth_skin();
    r.refine.push("skin", stroke(RefineMode::Add, &[(0.7, 0.5)], 0.08));
    let out = run(&img, &r, &faces);
    let added = disc(&out, 0.7, 0.5, 0.05);
    let changed = added.iter().filter(|(i, p)| !same_bits(*p, img.data[*i])).count();
    assert!(changed * 2 > added.len(), "Add smoothed the painted skin: {changed} of {}", added.len());
    // far from the stroke, the right half stays untouched
    assert!(disc(&out, 0.85, 0.15, 0.03).iter().all(|(i, p)| same_bits(*p, img.data[*i])));
    // and a face with no mask at all gets one from Add
    let bare = [Face { masks: Vec::new(), ..half_skin_face() }];
    assert!(run(&img, &smooth_skin(), &bare).data == img.data);
    assert!(disc(&run(&img, &r, &bare), 0.7, 0.5, 0.05).iter().any(|(i, p)| !same_bits(*p, img.data[*i])));
}

#[test]
fn every_feature_gets_reduce_without_code_of_its_own() {
    let img = common::picture(64, 48);
    let faces = [common::face(0.2, 0.15, 0.8, 0.85)];
    // a full Reduce over the whole picture: every feature at full strength gives back the pixels from before it ran
    // (this feature is the only one on, so those are the picture's)
    let cover: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.5), (0.0, 1.0), (1.0, 1.0)];
    for f in FEATURES {
        let mut r = Retouch::default();
        for spec in (f.specs)() {
            r.global.set(spec.id, spec.max);
        }
        if r.global.is_off() {
            continue;
        }
        assert!(r.refine.push(f.key, stroke(RefineMode::Reduce, &cover, 0.5)), "{}", f.key);
        let out = run(&img, &r, &faces);
        assert!(out.data.iter().zip(&img.data).all(|(a, b)| same_bits(*a, *b)), "{}: a full Reduce everywhere gives back the pixels from before the feature", f.key);
    }
}

/// A smooth, teeth-coloured picture: the same scene at every size.
fn smooth(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        [0.78 + 0.08 * u, 0.7 + 0.06 * v, 0.55 + 0.05 * u * v]
    })
}

/// A face whose Teeth mask covers its left half only.
fn half_teeth_face() -> Face {
    let left = Plane::from_fn(32, 32, |x, _| if x < 16 { 1.0 } else { 0.0 });
    Face { masks: vec![(Part::Teeth, PartMask { plane: left, area: NormRect::new(0.1, 0.1, 0.9, 0.9) })], ..half_skin_face() }
}

/// The picture sampled at normalised (`u`, `v`), bilinear between pixel centres.
fn sample(img: &Rgb32f, u: f64, v: f64) -> [f32; 3] {
    let (fx, fy) = ((u * img.width as f64 - 0.5).clamp(0.0, (img.width - 1) as f64), (v * img.height as f64 - 0.5).clamp(0.0, (img.height - 1) as f64));
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let px = |x: usize, y: usize| img.data[y * img.width + x];
    let mut out = [0.0; 3];
    for k in 0..3 {
        let top = px(x0, y0)[k] * (1.0 - tx) + px(x1, y0)[k] * tx;
        let bottom = px(x0, y1)[k] * (1.0 - tx) + px(x1, y1)[k] * tx;
        out[k] = top * (1.0 - ty) + bottom * ty;
    }
    out
}

/// A draft and an export four times its size sample the same strokes: every pixel of the draft is the export's
/// there, to within a level of 255, with Reduce (taking the whitening back on the left) and Add (whitening where the
/// mask missed, on the right) both changing pixels.
#[test]
fn preview_equals_export() {
    let faces = [half_teeth_face()];
    let mut plain = Retouch::default();
    assert!(plain.global.set("retouch.teethLips.teethWhitening", 100.0));
    let mut r = plain.clone();
    // (soft brushes, as painted: a hard edge is one grid cell wide and a draft's pixels fall differently on it)
    let soft = |mode, points: &[(f64, f64)]| RefineStroke { feather: 100.0, ..stroke(mode, points, 0.07) };
    r.refine.push("teethLips", soft(RefineMode::Reduce, &[(0.25, 0.4), (0.3, 0.6)]));
    r.refine.push("teethLips", soft(RefineMode::Add, &[(0.7, 0.5), (0.72, 0.62)]));
    let (small, big) = (smooth(80, 60), smooth(320, 240));
    let (a, b) = (run(&small, &r, &faces), run(&big, &r, &faces));
    let base = run(&small, &plain, &faces);
    let (mut reduced, mut added) = (0, 0);
    let (mut worst, mut at_worst) = (0.0f32, (0, 0));
    for y in 0..small.height {
        for x in 0..small.width {
            let (u, v) = ((x as f64 + 0.5) / small.width as f64, (y as f64 + 0.5) / small.height as f64);
            let (p, q, o) = (a.data[y * small.width + x], sample(&b, u, v), base.data[y * small.width + x]);
            let d = (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0, f32::max);
            if d > worst {
                (worst, at_worst) = (d, (x, y));
            }
            let moved = (0..3).map(|k| (p[k] - o[k]).abs()).fold(0.0, f32::max) > 0.01;
            if moved && u < 0.5 {
                reduced += 1;
            }
            if moved && u > 0.5 {
                added += 1;
            }
        }
    }
    assert!(reduced > 20 && added > 20, "Reduce and Add both change the draft ({reduced}, {added})");
    assert!(worst <= 1.0 / 255.0, "the draft differs from the export by {worst} at {at_worst:?}");
}

/// Faces A (left) and B (right) have no Teeth mask; whitening is on for A alone. A's own stroke starts in A's box and
/// runs toward B, its centre over B: it is A's, so it applies to A; for every face it would be B's, and B is off.
#[test]
fn a_picked_face_owns_its_add_stroke() {
    let img = smooth(160, 80);
    let a = Face::boxed(NormRect::new(0.05, 0.15, 0.4, 0.85));
    let b = Face::boxed(NormRect::new(0.6, 0.15, 0.95, 0.85));
    let faces = [a, b];
    let along = [(0.3, 0.5), (0.45, 0.5), (0.8, 0.5)];
    let mut picked = Retouch::default();
    let own = picked.own_mut_in(&faces, 0, None).unwrap();
    assert!(own.settings.set("retouch.teethLips.teethWhitening", 100.0));
    own.refine.push("teethLips", stroke(RefineMode::Add, &along, 0.05));
    let out = run(&img, &picked, &faces);
    assert!(disc(&out, 0.3, 0.5, 0.03).iter().any(|(i, p)| !same_bits(*p, img.data[*i])), "the picked face's stroke applies to it");
    // the same stroke for every face goes to the face nearest its centre, B, which is off
    let mut shared = Retouch::default();
    let own = shared.own_mut_in(&faces, 0, None).unwrap();
    assert!(own.settings.set("retouch.teethLips.teethWhitening", 100.0));
    shared.refine.push("teethLips", stroke(RefineMode::Add, &along, 0.05));
    let out = run(&img, &shared, &faces);
    assert!(out.data == img.data, "for every face the stroke is B's, and B is off");
}

/// An Add stroke further than a face's size from every face is left out of a render.
#[test]
fn an_add_stroke_far_from_every_face_does_nothing() {
    let img = smooth(160, 80);
    let faces = [Face::boxed(NormRect::new(0.05, 0.1, 0.25, 0.4))];
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.teethLips.teethWhitening", 100.0));
    r.refine.push("teethLips", stroke(RefineMode::Add, &[(0.9, 0.9), (0.95, 0.9)], 0.04));
    assert!(run(&img, &r, &faces).data == img.data);
    let mut near = smooth_teeth();
    near.refine.push("teethLips", stroke(RefineMode::Add, &[(0.3, 0.3), (0.32, 0.3)], 0.04));
    assert!(run(&img, &near, &faces).data != img.data, "a stroke beside the face works");
}

fn smooth_teeth() -> Retouch {
    let mut r = Retouch::default();
    assert!(r.global.set("retouch.teethLips.teethWhitening", 100.0));
    r
}

/// Strokes over the work cap are refused by the budget check, and a render leaves them out instead of hanging.
#[test]
fn strokes_over_the_work_cap_are_left_out() {
    let img = smooth(160, 80);
    let faces = [half_teeth_face()];
    let zig: Vec<(f64, f64)> = (0..20_000).map(|i| if i % 2 == 0 { (0.1, 0.1) } else { (0.9, 0.9) }).collect();
    let mut r = smooth_teeth();
    r.refine.push("teethLips", stroke(RefineMode::Reduce, &[(0.5, 0.5), (0.52, 0.5)], 0.001));
    assert!(lightcraft_retouch::refine::check_budget(&r).is_ok());
    r.refine.push("teethLips", stroke(RefineMode::Reduce, &zig, 0.5));
    let reason = lightcraft_retouch::refine::check_budget(&r).unwrap_err();
    assert!(reason.contains("teethLips"), "{reason}");
    assert!(run(&img, &r, &faces).data == run(&img, &smooth_teeth(), &faces).data, "the strokes are left out");
}

#[test]
fn a_face_with_its_own_strokes_keeps_them_to_itself() {
    let img = picture(160, 80);
    let left = Face { masks: vec![(Part::FacialSkin, full_mask(0.05, 0.1, 0.45, 0.9))], ..Face::boxed(NormRect::new(0.05, 0.1, 0.45, 0.9)) };
    let right = Face { masks: vec![(Part::FacialSkin, full_mask(0.55, 0.1, 0.95, 0.9))], ..Face::boxed(NormRect::new(0.55, 0.1, 0.95, 0.9)) };
    let faces = [left.clone(), right.clone()];
    let mut r = smooth_skin();
    let own = r.own_mut_in(&faces, 1, None).unwrap();
    own.refine.push("skin", stroke(RefineMode::Reduce, &[(0.0, 0.5), (1.0, 0.5)], 0.5));
    let out = run(&img, &r, &faces);
    // the right face's run is taken back; the left face's is not
    assert!(disc(&out, 0.75, 0.5, 0.05).iter().all(|(i, p)| same_bits(*p, img.data[*i])));
    assert!(disc(&out, 0.25, 0.5, 0.05).iter().any(|(i, p)| !same_bits(*p, img.data[*i])));
}

fn full_mask(x0: f64, y0: f64, x1: f64, y1: f64) -> PartMask {
    PartMask { plane: Plane::filled(8, 8, 1.0), area: NormRect::new(x0, y0, x1, y1) }
}

#[test]
fn strokes_alone_change_nothing_while_the_feature_is_off() {
    let img = picture(60, 40);
    let mut r = Retouch::default();
    r.refine.push("skin", stroke(RefineMode::Add, &[(0.7, 0.5)], 0.1));
    r.refine.push("eyes", stroke(RefineMode::Reduce, &[(0.3, 0.5)], 0.1));
    assert!(r.is_off() && !r.is_default());
    assert!(run(&img, &r, &[half_skin_face()]).data == img.data);
}

#[test]
fn strokes_round_trip_and_malformed_ones_are_left_out() {
    let mut r = smooth_skin();
    r.refine.push("skin", RefineStroke { pressure: vec![0.5, 0.8], ..stroke(RefineMode::Add, &[(0.1, 0.2), (0.3, 0.4)], 0.05) });
    let own = FaceOverride { refine: r.refine.clone(), ..FaceOverride::default() };
    r.faces.push(FaceOverride { face: lightcraft_retouch::FaceKey { bounds: NormRect::new(0.1, 0.1, 0.3, 0.3), ..Default::default() }, ..own });
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["refine"][0]["feature"], "skin");
    assert_eq!(v["refine"][0]["strokes"][0]["mode"], "add");
    assert_eq!(serde_json::from_value::<Retouch>(v).unwrap(), r);
    // a block without strokes writes none
    assert!(serde_json::to_value(smooth_skin()).unwrap().get("refine").is_none());
    let bad = json!({"global": {}, "refine": [
        {"feature": "nope", "strokes": [{"points": [[0.1, 0.1]]}]},
        {"feature": "skin", "strokes": [{"points": []}, {"points": [[1e308, 0.0]]}, "x", {"points": [[0.5, 0.5]], "size": -4, "strength": 1e9}]},
        {"strokes": []}, 7
    ]});
    let r: Retouch = serde_json::from_value(bad).unwrap();
    let kept = r.refine.strokes("skin");
    assert_eq!(kept.len(), 1);
    assert_eq!((kept[0].size, kept[0].strength), (0.001, 100.0));
    assert_eq!(r.refine.count(None), 1);
    assert_eq!(Refine::from_value(&json!("nope")), Refine::default());
}

#[test]
fn hostile_strokes_never_panic() {
    let wild = [
        stroke(RefineMode::Add, &[(-1.0, -1.0), (2.0, 2.0), (-1.0, 2.0)], 0.001),
        stroke(RefineMode::Reduce, &[(0.5, 0.5); 3], 0.5),
        RefineStroke { pressure: vec![f32::NAN, -3.0, 9.0], ..stroke(RefineMode::Restore, &[(0.1, 0.1), (0.9, 0.9), (0.5, 0.1)], 0.02) },
    ];
    let mut r = smooth_skin();
    for s in &wild {
        for f in FEATURES {
            r.refine.push(f.key, s.clone());
        }
    }
    for f in FEATURES {
        for spec in (f.specs)() {
            r.global.set(spec.id, spec.max);
        }
    }
    let many: Vec<Face> = (0..50)
        .map(|i| common::face((i % 10) as f64 / 10.0, (i / 10) as f64 / 5.0, (i % 10) as f64 / 10.0 + 0.1, (i / 10) as f64 / 5.0 + 0.2))
        .collect();
    for (w, h) in [(64, 48), (1, 1), (0, 0), (3, 1)] {
        for faces in [Vec::new(), many.clone(), vec![half_skin_face()]] {
            let out = run(&common::picture(w, h), &r, &faces);
            assert!(out.data.iter().flatten().all(|v| v.is_finite()));
        }
    }
    let _ = Features::default();
}
