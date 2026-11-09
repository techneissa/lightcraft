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
fn a_full_reduce_gives_back_the_original_bits_there_and_nowhere_else() {
    let img = picture(120, 90);
    let faces = [half_skin_face()];
    let smoothed = run(&img, &smooth_skin(), &faces);
    let mut r = smooth_skin();
    assert!(r.refine.push("skin", stroke(RefineMode::Reduce, &[(0.3, 0.5)], 0.08)));
    let out = run(&img, &r, &faces);
    let inside = disc(&out, 0.3, 0.5, 0.06);
    assert!(!inside.is_empty());
    assert!(inside.iter().all(|(i, p)| same_bits(*p, img.data[*i])), "inside a full Reduce: the original pixels");
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
    // a full Reduce over the whole picture: every feature at full strength gives back the original
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
        assert!(out.data.iter().zip(&img.data).all(|(a, b)| same_bits(*a, *b)), "{}: a full Reduce everywhere is the original", f.key);
    }
}

#[test]
fn preview_equals_export() {
    // a draft and an export four times its size: the same pixels go back to the original, and Add smooths the
    // same place
    let faces = [half_skin_face()];
    let mut r = smooth_skin();
    r.refine.push("skin", stroke(RefineMode::Reduce, &[(0.25, 0.4), (0.3, 0.6)], 0.06));
    r.refine.push("skin", stroke(RefineMode::Add, &[(0.7, 0.5)], 0.06));
    let (small, big) = (picture(80, 60), picture(320, 240));
    let (a, b) = (run(&small, &r, &faces), run(&big, &r, &faces));
    for (u, v) in [(0.27, 0.5), (0.25, 0.42), (0.15, 0.2), (0.7, 0.5), (0.72, 0.52), (0.85, 0.2)] {
        let ka = same_bits(at(&a, u, v), at(&small, u, v));
        let kb = same_bits(at(&b, u, v), at(&big, u, v));
        assert_eq!(ka, kb, "({u}, {v}) kept in the draft and the export alike");
    }
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
