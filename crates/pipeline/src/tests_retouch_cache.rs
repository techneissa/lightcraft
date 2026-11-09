//! Fork (`fork/perf-portrait-draft.md`): a Portrait slider tick starts from the image before the retouch step, and the
//! step changes pixels only inside the boxes it declares ([`retouch_step::write_boxes`]).

use std::sync::Arc;

use lightcraft_develop::retouch::{FEATURES, Face, Input, NormRect, Part, PartMask, Scope};
use lightcraft_develop::{DevelopSettings, Spot, SpotMode, controls};
use lightcraft_geom::Point;
use lightcraft_raster::{Plane, Rgb32f};

use crate::{PixelWindow, Quality, RenderRequest, SourceInfo, StageCache, render, render_cached, retouch_step};

/// One slider of each Portrait feature a drag moves.
const SLIDERS: [&str; 5] =
    ["retouch.skin.smoothness", "retouch.eyes.whites", "retouch.skinTone.evenTone", "retouch.teethLips.teethWhitening", "retouch.blemish.amount"];

/// A face in `bounds` on a `w × h` photo (skin with texture and a few blemishes, eyes, lips, teeth) with its part
/// masks and five landmarks, its spots found as the engine finds them.
fn portrait(w: usize, h: usize, bounds: NormRect) -> (Arc<Rgb32f>, SourceInfo) {
    let at = |fx: f64, fy: f64| (bounds.x0 + fx * bounds.width(), bounds.y0 + fy * bounds.height());
    let ell = move |x: f64, y: f64, c: (f64, f64), rx: f64, ry: f64| {
        let d = (((x - c.0) / (rx * bounds.width())).powi(2) + ((y - c.1) / (ry * bounds.height())).powi(2)).sqrt();
        ((1.1 - d) / 0.2).clamp(0.0, 1.0) as f32
    };
    let eyes = [at(0.33, 0.4), at(0.67, 0.4)];
    let (mouth, centre) = (at(0.5, 0.75), at(0.5, 0.5));
    let skin = move |x: f64, y: f64| ell(x, y, centre, 0.48, 0.5);
    let whites = move |x: f64, y: f64| eyes.iter().map(|c| ell(x, y, *c, 0.09, 0.04)).fold(0.0, f32::max);
    let iris = move |x: f64, y: f64| eyes.iter().map(|c| ell(x, y, *c, 0.035, 0.035)).fold(0.0, f32::max);
    let lips = move |x: f64, y: f64| ell(x, y, mouth, 0.16, 0.06);
    let teeth = move |x: f64, y: f64| ell(x, y, mouth, 0.1, 0.025);
    let dots: Vec<(f64, f64)> = (0..8).map(|i| at(0.28 + 0.06 * (i % 4) as f64, 0.55 + 0.08 * (i / 4) as f64)).collect();
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        let tex = 1.0 + 0.05 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0 + 0.08 * ((x as f32 / 3.1).sin() * (y as f32 / 2.7).sin());
        let dot = dots.iter().map(|c| ell(nx, ny, *c, 0.02, 0.015)).fold(0.0, f32::max);
        let face = [0.5 * tex * (1.0 - 0.3 * dot), 0.36 * tex * (1.0 - 0.45 * dot), 0.28 * tex * (1.0 - 0.4 * dot)];
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t) };
        let mut c = mix([0.12 + 0.05 * nx as f32, 0.14, 0.16], face, skin(nx, ny));
        c = mix(c, [0.62, 0.5, 0.42], whites(nx, ny));
        c = mix(c, [0.1, 0.07, 0.05], iris(nx, ny));
        c = mix(c, [0.42, 0.12, 0.13], lips(nx, ny));
        mix(c, [0.6, 0.55, 0.36], teeth(nx, ny))
    });
    let mask = |f: &dyn Fn(f64, f64) -> f32| {
        let n = 96;
        let plane = Plane::from_fn(n, n, |x, y| {
            f(bounds.x0 + (x as f64 + 0.5) / n as f64 * bounds.width(), bounds.y0 + (y as f64 + 0.5) / n as f64 * bounds.height())
        });
        PartMask { plane, area: bounds }
    };
    let face = Face {
        landmarks: vec![eyes[0], eyes[1], at(0.5, 0.58), at(0.4, 0.75), at(0.6, 0.75)],
        masks: vec![
            (Part::FacialSkin, mask(&|x, y| skin(x, y) * (1.0 - whites(x, y).max(lips(x, y))))),
            (Part::EyeWhites, mask(&|x, y| whites(x, y) * (1.0 - iris(x, y)))),
            (Part::Iris, mask(&iris)),
            (Part::Lips, mask(&|x, y| lips(x, y) * (1.0 - teeth(x, y)))),
            (Part::Teeth, mask(&teeth)),
        ],
        ..Face::boxed(bounds)
    };
    let faces = lightcraft_develop::retouch::blemish::with_spots(&src, vec![face]);
    (Arc::new(src), SourceInfo { retouch: Some(Arc::new(Input::new(faces))), ..SourceInfo::default() })
}

/// A raw's defaults (sharpening, colour noise reduction), tone, clarity and a Heal spot off the face.
fn edit() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.detail.sharpen_amount = 40.0;
    s.detail.nr_color = 25.0;
    s.light.highlights = -30.0;
    s.effects.clarity = 15.0;
    s.spots = vec![Spot {
        mode: SpotMode::Heal,
        points: vec![Point::new(0.1, 0.8)],
        size: 0.02,
        source_offset: Some(Point::new(0.04, 0.0)),
        ..Default::default()
    }];
    s
}

/// A tick of each Portrait slider reuses the image before the retouch step (no white balance, spots or red eye again)
/// and the noise model, and renders what a render without the cache renders, byte for byte. A change before the step
/// (a spot) makes it again.
#[test]
fn a_portrait_slider_tick_starts_from_the_image_before_retouching() {
    let (src, info) = portrait(360, 240, NormRect::new(0.3, 0.1, 0.7, 0.9));
    let req = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(360, 240) };
    for id in SLIDERS {
        let cache = StageCache::default();
        let mut s = edit();
        controls::set(&mut s, id, 40.0);
        let runs = retouch_step::pre_retouch_runs();
        render_cached(&src, &info, &s, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "{id}: the first render makes the image before the step");
        let mut tick = s.clone();
        controls::set(&mut tick, id, 41.0);
        let got = render_cached(&src, &info, &tick, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "{id}: a tick starts from the kept image");
        assert_eq!(got.image, render(&src, &info, &tick, &req).image, "{id}: the same pixels as a render without the cache");
        // (the render without the cache made its own)
        let runs = retouch_step::pre_retouch_runs() - 1;
        let (a, b) = (crate::plan(&src, &info, &s, &req), crate::plan(&src, &info, &tick, &req));
        assert_eq!((a.pre_retouch_key, a.noise_key), (b.pre_retouch_key, b.noise_key), "{id}");
        assert_ne!((a.pre_nr_key, a.lin_key), (b.pre_nr_key, b.lin_key), "{id}");
        // a tick of another slider (exposure) keeps the linear image; a spot that moved makes the image again
        let mut exposure = tick.clone();
        exposure.light.exposure = 0.4;
        render_cached(&src, &info, &exposure, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 1, "{id}: exposure");
        exposure.spots[0].points[0].x += 0.05;
        let got = render_cached(&src, &info, &exposure, &req, &cache);
        assert_eq!(retouch_step::pre_retouch_runs(), runs + 2, "{id}: a moved spot");
        assert_eq!(got.image, render(&src, &info, &exposure, &req).image, "{id}: after a spot moved");
    }
}

/// Every pixel the step changes lies in one of its boxes, for every slider of every face feature at its ends, in the
/// whole frame and in a zoomed window over part of the face; a picture-wide feature may change any pixel.
#[test]
fn the_step_writes_only_inside_its_boxes() {
    let (src, info) = portrait(300, 200, NormRect::new(0.35, 0.15, 0.6, 0.75));
    let whole = RenderRequest::fit(300, 200);
    let zoomed = RenderRequest { window: Some(PixelWindow { x: 120, y: 60, w: 40, h: 30 }), ..whole };
    let face_specs = FEATURES.iter().filter(|f| f.scope == Scope::Face).flat_map(|f| (f.specs)());
    let mut changed_any = 0;
    for spec in face_specs.filter(|s| !s.id.starts_with("retouch.reshape.")) {
        for v in [spec.min, spec.max].into_iter().filter(|v| *v != spec.default) {
            let mut s = DevelopSettings::default();
            controls::set(&mut s, spec.id, v);
            for req in [whole, zoomed] {
                let p = crate::plan(&src, &info, &s, &req);
                let mut img = p.frame.sample(&src, p.w, p.h);
                crate::lin_cpu_pre_retouch(&mut img, &info, &p);
                let before = img.clone();
                retouch_step::apply(&mut img, &info, &p);
                let boxes = retouch_step::write_boxes(&info, &p).expect("a face feature stays in its boxes");
                for (i, (a, b)) in before.data.iter().zip(&img.data).enumerate() {
                    if a != b {
                        changed_any += 1;
                        let (x, y) = (i % p.w, i / p.w);
                        let inside = boxes.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]);
                        assert!(inside, "{} = {v}: pixel ({x}, {y}) changed outside {boxes:?}", spec.id);
                    }
                }
            }
        }
    }
    assert!(changed_any > 1000, "the features changed pixels: {changed_any}");
    let plain = DevelopSettings::default();
    let off = crate::plan(&src, &info, &plain, &whole);
    assert_eq!(retouch_step::write_boxes(&info, &off), Some(Vec::new()), "off: nowhere");
    for spec in FEATURES.iter().filter(|f| f.scope == Scope::Picture).flat_map(|f| (f.specs)()) {
        let mut s = DevelopSettings::default();
        controls::set(&mut s, spec.id, if spec.max != spec.default { spec.max } else { spec.min });
        assert_eq!(retouch_step::write_boxes(&info, &crate::plan(&src, &info, &s, &whole)), None, "{}: anywhere", spec.id);
    }
}
