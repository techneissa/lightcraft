//! Issue #372: after spots and retouching, a later edit (a slider, an effect, a brush mask, a stroke) drawn as a drag's
//! drafts at other sizes and then at rest makes no linear stage again (spots, retouching, white balance, noise
//! reduction): every draft takes the image the full render kept, and the full render after the drag still finds it.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use lightcraft_gpu::{drafts_resampled, linear_stages_made};
use lightcraft_pipeline::spots_keep::spots_drawn;
use lightcraft_pipeline::{PixelWindow, Quality, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::{Rgb32f, Rgba8};

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// Heal, Clone, Remove, Content-Aware and a LaMa spot (no model here: it takes the Classic fill, but its drafts still
/// had keys of their own).
fn spots() -> Vec<Spot> {
    let at = |mode, x, y, fill| Spot {
        mode,
        points: vec![Point::new(x, y)],
        size: 0.012,
        feather: 50.0,
        opacity: 100.0,
        fill,
        source_offset: matches!(mode, SpotMode::Heal | SpotMode::Clone).then_some(Point::new(0.04, 0.0)),
        ..Default::default()
    };
    vec![
        at(SpotMode::Heal, 0.2, 0.3, SpotFill::Classic),
        at(SpotMode::Clone, 0.6, 0.4, SpotFill::Classic),
        at(SpotMode::Remove, 0.4, 0.6, SpotFill::Classic),
        at(SpotMode::Fill, 0.25, 0.75, SpotFill::Classic),
        at(SpotMode::Remove, 0.7, 0.7, SpotFill::Lama),
    ]
}

fn brush() -> Mask {
    let shape = MaskShape::Brush {
        strokes: vec![BrushStroke { points: vec![Point::new(0.3, 0.4), Point::new(0.5, 0.45)].into(), size: 0.05, ..Default::default() }],
    };
    let adjust = lightcraft_develop::LocalAdjustments { exposure: 0.5, ..Default::default() };
    Mask { id: 1, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], adjust, ..Default::default() }
}

fn draft(size: usize) -> RenderRequest {
    RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(size, size) }
}

type Edit = fn(&mut DevelopSettings, f64);

/// Each kind of later edit, at a drag step `t` in 0..1.
fn kinds() -> [(&'static str, Edit); 4] {
    [
        ("Exposure", |s, t| s.light.exposure = 0.8 * t),
        ("an effect (Clarity)", |s, t| s.effects.clarity = 50.0 * t),
        ("a brush mask added", |s, t| {
            if s.masks.is_empty() {
                s.masks.push(brush());
            }
            s.masks[0].adjust.exposure = t;
        }),
        ("a brush stroke", |s, t| {
            if s.masks.is_empty() {
                s.masks.push(brush());
            }
            if let MaskShape::Brush { strokes } = &mut s.masks[0].components[0].shape {
                let y = 0.5 + 0.2 * t;
                strokes.push(BrushStroke { points: vec![Point::new(0.2, y), Point::new(0.6, y)].into(), size: 0.04, ..Default::default() });
            }
        }),
    ]
}

/// A drag of each kind after the spots: drafts at a smaller size, then at one smaller still (full-size drafts too slow),
/// then the render at rest. None makes a linear stage; the drafts are made from the full render's image.
#[test]
fn a_later_edits_drafts_and_render_reuse_the_retouched_image() {
    if !gpu() {
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    let rest = RenderRequest::fit(960, 960);
    for (name, edit) in kinds() {
        let cache = StageCache::default();
        let mut s = DevelopSettings { spots: spots(), ..Default::default() };
        lightcraft_gpu::render(&src, &info, &s, &rest, Some(&cache)).expect("gpu");
        let (made, resampled) = (linear_stages_made(), drafts_resampled());
        for (step, size) in [(0.25, 720), (0.5, 720), (0.75, 480), (1.0, 480)] {
            edit(&mut s, step);
            lightcraft_gpu::render(&src, &info, &s, &draft(size), Some(&cache)).expect("gpu");
        }
        let at_rest = lightcraft_gpu::render(&src, &info, &s, &rest, Some(&cache)).expect("gpu").image;
        assert_eq!(linear_stages_made() - made, 0, "{name}: no draft and not the render at rest made the linear stage again");
        assert!(drafts_resampled() > resampled, "{name}: the drafts were made from the full render's image");
        let fresh = lightcraft_gpu::render(&src, &info, &s, &rest, None).expect("gpu").image;
        assert!(at_rest == fresh, "{name}: the render at rest is the one a fresh render gives");
    }
}

/// A draft made from the full render's image shows the same picture as a draft that makes its own, but for the
/// resampling (noise reduction is off in both).
#[test]
fn a_draft_from_the_kept_image_looks_like_one_made_afresh() {
    if !gpu() {
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    let cache = StageCache::default();
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    lightcraft_gpu::render(&src, &info, &s, &RenderRequest::fit(960, 960), Some(&cache)).expect("gpu");
    s.light.exposure = 0.5;
    let resampled = drafts_resampled();
    let kept = lightcraft_gpu::render(&src, &info, &s, &draft(600), Some(&cache)).expect("gpu").image;
    assert!(drafts_resampled() > resampled);
    let afresh = lightcraft_gpu::render(&src, &info, &s, &draft(600), None).expect("gpu").image;
    assert_eq!((kept.width, kept.height), (afresh.width, afresh.height));
    // (measured: a mean of 0.25 levels of 255, 0.12 of it one way, a median of 0 and a 99th percentile of 4; a shift of
    // one level everywhere, or the image in another colour space, fails)
    let (mean, signed, p50, p99) = differences(&kept, &afresh);
    assert!(mean < 0.75 && signed.abs() < 0.5, "the mean difference {mean}, one way {signed}");
    assert!(p50 == 0 && p99 <= 8, "the median difference {p50}, the 99th percentile {p99}");
}

/// Per channel, the two pictures' differences: the mean, the mean signed (`a` − `b`), the median and the 99th
/// percentile, in levels of 255.
fn differences(a: &Rgba8, b: &Rgba8) -> (f64, f64, u8, u8) {
    let mut d: Vec<u8> = Vec::with_capacity(a.data.len() * 3);
    let mut signed = 0i64;
    for (p, q) in a.data.iter().zip(&b.data) {
        for k in 0..3 {
            d.push(p[k].abs_diff(q[k]));
            signed += i64::from(p[k]) - i64::from(q[k]);
        }
    }
    let n = d.len().max(1);
    let mean = d.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
    d.sort_unstable();
    let at = |q: f64| d.get(((n - 1) as f64 * q) as usize).copied().unwrap_or(0);
    (mean, signed as f64 / n as f64, at(0.5), at(0.99))
}

/// Zoomed in, a drag's draft of the window renders at the window's own size: with a LaMa spot on the photo its keys
/// differed from the full render's, so every draft made the window's linear stage again, and the render at rest made it
/// once more.
#[test]
fn a_zoomed_in_draft_with_a_lama_spot_takes_the_full_renders_image() {
    if !gpu() {
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    let window = RenderRequest { window: Some(PixelWindow { x: 500, y: 300, w: 300, h: 200 }), ..RenderRequest::fit(1920, 1920) };
    let cache = StageCache::default();
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    // (the first window grows to hold its spots' reads; the next one pastes the spots it finished: the window as a view
    // renders it from then on)
    for _ in 0..2 {
        lightcraft_gpu::render(&src, &info, &s, &window, Some(&cache)).expect("gpu");
    }
    // (the linear stages the view's renders made, not the fresh ones it is compared with)
    let mut made = 0;
    let mut cached = |s: &DevelopSettings, req: &RenderRequest| {
        let before = linear_stages_made();
        let img = lightcraft_gpu::render(&src, &info, s, req, Some(&cache)).expect("gpu").image;
        made += linear_stages_made() - before;
        img
    };
    let drafted = RenderRequest { quality: Quality::Draft, ..window };
    for t in [0.2, 0.4, 0.6] {
        s.light.exposure = t;
        let kept = cached(&s, &drafted);
        let fresh = lightcraft_gpu::render(&src, &info, &s, &drafted, None).expect("gpu").image;
        assert!(kept == fresh, "the draft at {t} is the one a fresh draft gives");
    }
    let at_rest = cached(&s, &window);
    assert_eq!(made, 0, "no draft and not the render at rest made the window's linear stage");
    let fresh = lightcraft_gpu::render(&src, &info, &s, &window, None).expect("gpu").image;
    assert!(at_rest == fresh, "the window at rest is the one a fresh render gives");
}

/// A draft drawn afresh (no full render yet) is no full render's image: a later, smaller draft is never resampled from
/// it, only from a full render's.
#[test]
fn a_draft_is_never_made_from_another_draft() {
    if !gpu() {
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    let cache = StageCache::default();
    // (no LaMa spot: a draft with one has keys of its own anyway)
    let mut s = DevelopSettings { spots: spots().into_iter().filter(|sp| sp.fill != SpotFill::Lama).collect(), ..Default::default() };
    lightcraft_gpu::render(&src, &info, &s, &draft(720), Some(&cache)).expect("gpu");
    s.light.exposure = 0.5;
    let resampled = drafts_resampled();
    lightcraft_gpu::render(&src, &info, &s, &draft(480), Some(&cache)).expect("gpu");
    assert_eq!(drafts_resampled(), resampled, "the smaller draft draws its own");
    lightcraft_gpu::render(&src, &info, &s, &RenderRequest::fit(960, 960), Some(&cache)).expect("gpu");
    lightcraft_gpu::render(&src, &info, &s, &draft(600), Some(&cache)).expect("gpu");
    assert_eq!(drafts_resampled(), resampled + 1, "once a full render is kept, a draft is made from it");
}

/// Issue #372 as reported: with a LaMa spot on the photo (here without its model: it takes the Classic fill), adding a
/// Heal spot drew every spot again. It draws the Heal spot alone, and the picture is a fresh render's.
#[test]
fn a_heal_spot_added_beside_a_lama_spot_draws_only_itself() {
    if !gpu() {
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    let rest = RenderRequest::fit(960, 960);
    let cache = StageCache::default();
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    lightcraft_gpu::render(&src, &info, &s, &rest, Some(&cache)).expect("gpu");
    s.spots.push(Spot {
        mode: SpotMode::Heal,
        points: vec![Point::new(0.85, 0.2)],
        size: 0.012,
        source_offset: Some(Point::new(0.0, 0.05)),
        ..Default::default()
    });
    let before = spots_drawn();
    let kept = lightcraft_gpu::render(&src, &info, &s, &rest, Some(&cache)).expect("gpu").image;
    assert_eq!(spots_drawn() - before, 1, "the Heal spot alone");
    let fresh = lightcraft_gpu::render(&src, &info, &s, &rest, None).expect("gpu").image;
    assert!(kept == fresh, "the picture is a fresh render's");
}
