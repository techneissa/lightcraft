//! Fork (`fork/perf-portrait-draft.md`): a Portrait slider tick on the GPU starts from the image before the retouch
//! step (kept with the view's stages), and redoes noise reduction, the planes and the per-pixel stage only in the
//! tiles the step's boxes reach, with the bits of a whole render.

use std::sync::Arc;

use lightcraft_develop::retouch::{Face, Input, NormRect, Part, PartMask};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_gpu::tiles_rendered;
use lightcraft_pipeline::retouch_step::pre_retouch_runs;
use lightcraft_pipeline::{Quality, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::{Plane, Rgb32f};

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        let why = lightcraft_gpu::unavailable_reason().unwrap_or_default();
        assert!(!why.contains("kernels failed to build"), "{why}");
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// One slider of each Portrait feature a drag moves.
const SLIDERS: [&str; 5] =
    ["retouch.skin.smoothness", "retouch.eyes.whites", "retouch.skinTone.evenTone", "retouch.teethLips.teethWhitening", "retouch.blemish.amount"];

/// A face in `bounds` on a `w × h` photo with its part masks, five landmarks and its spots.
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

/// A raw's defaults (sharpening, the joint colour noise reduction), tone and clarity: every plane a drag redoes.
fn edit() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.detail.sharpen_amount = 40.0;
    s.detail.nr_color = 25.0;
    s.light.highlights = -30.0;
    s.light.shadows = 20.0;
    s.effects.clarity = 15.0;
    s
}

/// A whole render of `s` that does not depend on the retouch step's boxes: no stages, the retouched image uploaded
/// whole.
fn whole(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest) -> lightcraft_raster::Rgba8 {
    lightcraft_gpu::retouch_full_upload(true);
    let img = lightcraft_gpu::render(src, info, s, req, None).expect("gpu").image;
    lightcraft_gpu::retouch_full_upload(false);
    img
}

/// Render `s` with `cache`: (output tiles computed, image before-the-step runs), checked against [`whole`]: a box
/// that misses a pixel the step wrote leaves that pixel unretouched, and the tiles around it stale.
fn tick(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, cache: &StageCache, what: &str) -> (usize, usize) {
    let (tiles, runs) = (tiles_rendered(), pre_retouch_runs());
    let img = lightcraft_gpu::render(src, info, s, req, Some(cache)).expect("gpu").image;
    let counts = (tiles_rendered() - tiles, pre_retouch_runs() - runs);
    let whole = whole(src, info, s, req);
    let differ = img.data.iter().zip(&whole.data).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{what}: pixels that differ from a whole render");
    counts
}

#[test]
fn a_portrait_slider_tick_redoes_only_the_face_with_a_whole_render_s_pixels() {
    if !gpu() {
        return;
    }
    // (a draft of the loupe's size)
    let (w, h) = (1920, 1280);
    let req = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(w, h) };
    let all = w.div_ceil(lightcraft_gpu::TILE) * h.div_ceil(lightcraft_gpu::TILE);
    // a half-length portrait: the face is a fifth of the frame's height
    let (src, info) = portrait(w, h, NormRect::new(0.44, 0.12, 0.56, 0.32));
    for id in SLIDERS {
        let cache = StageCache::default();
        let mut s = edit();
        controls::set(&mut s, id, 40.0);
        let (n, runs) = tick(&src, &info, &s, &req, &cache, id);
        assert_eq!((n, runs), (all, 1), "{id}: the first render is whole and makes the image before the step");
        for v in [41.0, 45.0, 30.0] {
            controls::set(&mut s, id, v);
            let (n, runs) = tick(&src, &info, &s, &req, &cache, id);
            assert_eq!(runs, 0, "{id} = {v}: the image before the step is kept");
            // (Eyes reads 0.7 of the face's size around it, the planes reach further: about 60 % of the frame)
            assert!(n > 0 && n * 10 < all * 7, "{id} = {v}: {n} of {all} tiles");
        }
        // back at 0 the stage runs on the device again
        controls::set(&mut s, id, 0.0);
        assert_eq!(tick(&src, &info, &s, &req, &cache, id).1, 0, "{id} = 0");
    }
    // a face filling the frame: the tiles are most of it, the image before the step is still kept
    let (src, info) = portrait(w, h, NormRect::new(0.2, 0.02, 0.8, 0.98));
    let cache = StageCache::default();
    let mut s = edit();
    controls::set(&mut s, "retouch.skin.smoothness", 40.0);
    tick(&src, &info, &s, &req, &cache, "face-filling");
    controls::set(&mut s, "retouch.skin.smoothness", 45.0);
    let (_, runs) = tick(&src, &info, &s, &req, &cache, "face-filling tick");
    assert_eq!(runs, 0, "face-filling: the image before the step is kept");
}

/// A frame of one noise tile with one view's stages: a Skin slider taken to 0 and back renders what fresh renders do
/// (the noise is measured apart from the retouching only while it is on).
#[test]
fn a_one_tile_frame_measures_its_noise_as_a_fresh_render_does() {
    if !gpu() {
        return;
    }
    let (clean, info) = portrait(360, 240, NormRect::new(0.3, 0.1, 0.7, 0.9));
    // (colour noise, for the noise reduction to measure, and a large clone the tile measurement leaves out)
    let mut noisy = (*clean).clone();
    for (i, p) in noisy.data.iter_mut().enumerate() {
        let n = |k: u32| ((i as u32).wrapping_mul(2_654_435_761).wrapping_add(k.wrapping_mul(40_503)) >> 16) as f32 / 65_535.0 - 0.5;
        p[0] = (p[0] * (1.0 + 0.3 * n(1))).max(0.0);
        p[2] = (p[2] * (1.0 + 0.3 * n(2))).max(0.0);
    }
    let src = Arc::new(noisy);
    let req = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(360, 240) };
    let cache = StageCache::default();
    let mut s = edit();
    s.spots = vec![lightcraft_develop::Spot {
        mode: lightcraft_develop::SpotMode::Clone,
        points: vec![lightcraft_geom::Point::new(0.15, 0.5)],
        size: 0.12,
        source_offset: Some(lightcraft_geom::Point::new(0.35, 0.0)),
        ..Default::default()
    }];
    for v in [40.0, 0.0, 40.0, 0.0] {
        controls::set(&mut s, "retouch.skin.smoothness", v);
        tick(&src, &info, &s, &req, &cache, &format!("Skin {v}"));
    }
}
