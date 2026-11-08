//! Checks every feature's test file runs: off changes nothing, and no input makes it panic.
#![allow(dead_code)]

use lightcraft_raster::Rgb32f;
use lightcraft_retouch::{Face, Feature, Features, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

/// A small picture with some structure (a gradient and a bright square).
pub fn picture(w: usize, h: usize) -> Rgb32f {
    Rgb32f::from_fn(w, h, |x, y| {
        let g = (x + y) as f32 / (w + h).max(1) as f32;
        if (w / 3..2 * w / 3).contains(&x) && (h / 3..2 * h / 3).contains(&y) { [0.8, 0.6, 0.5] } else { [g, g * 0.9, g * 0.8] }
    })
}

/// A face with five landmarks and a mask of every part.
pub fn face(x0: f64, y0: f64, x1: f64, y1: f64) -> Face {
    let (w, h) = (x1 - x0, y1 - y0);
    let at = |fx: f64, fy: f64| (x0 + fx * w, y0 + fy * h);
    Face {
        bounds: NormRect::new(x0, y0, x1, y1),
        upright: None,
        landmarks: vec![at(0.3, 0.4), at(0.7, 0.4), at(0.5, 0.6), at(0.35, 0.8), at(0.65, 0.8)],
        score: 0.9,
        person: None,
        masks: Part::ALL
            .iter()
            .map(|p| (*p, PartMask { plane: lightcraft_raster::Plane::filled(8, 8, 1.0), area: NormRect::new(x0, y0, x1, y1) }))
            .collect(),
    }
}

/// Settings with every slider of `T` at its maximum (`max`) or its default, for every face.
pub fn settings_with<T: Feature>(max: bool) -> Retouch {
    let mut global = Features::default();
    for c in T::CONTROLS {
        let v = if max { c.spec.max } else { c.spec.default };
        assert!(global.set(c.spec.id, v), "{} is not reachable by id", c.spec.id);
    }
    Retouch { global, faces: Vec::new() }
}

fn run(img: &mut Rgb32f, r: &Retouch, input: &Input) {
    let view = WholePhoto { width: img.width, height: img.height };
    for stage in [Stage::Retouch, Stage::Geometry, Stage::Background] {
        lightcraft_retouch::run::apply(img, r, input, &view, stage);
    }
}

/// `T` off (its default, and every slider at its default) leaves every pixel as it was, bit for bit.
pub fn off_changes_nothing<T: Feature>() {
    assert!(T::default().is_off(), "{} must be off by default", T::KEY);
    let input = Input::new(vec![face(0.3, 0.2, 0.7, 0.7)]);
    let before = picture(64, 48);
    for r in [Retouch::default(), settings_with::<T>(false)] {
        let mut img = before.clone();
        run(&mut img, &r, &input);
        assert!(img.data.iter().zip(&before.data).all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits)), "{} off changed pixels", T::KEY);
    }
}

/// `T` at full strength runs on no faces, 50 faces, faces past the edge, a one-pixel photo and an empty one.
pub fn never_panics<T: Feature>() {
    let r = settings_with::<T>(true);
    let many: Vec<Face> =
        (0..50).map(|i| face((i % 10) as f64 / 10.0, (i / 10) as f64 / 5.0, (i % 10) as f64 / 10.0 + 0.1, (i / 10) as f64 / 5.0 + 0.2)).collect();
    let odd = vec![face(-0.5, -0.5, 0.2, 0.2), face(0.9, 0.9, 1.4, 1.6), Face::boxed(NormRect::new(f64::NAN, 0.0, 1.0, 1.0)), Face::default()];
    for (w, h) in [(64, 48), (1, 1), (0, 0), (3, 1)] {
        for faces in [Vec::new(), many.clone(), odd.clone()] {
            let mut img = picture(w, h);
            run(&mut img, &r, &Input::new(faces));
            assert_eq!((img.width, img.height), (w, h));
            assert!(img.data.iter().flatten().all(|v| v.is_finite()), "{} made a pixel that is not a number", T::KEY);
        }
    }
}
