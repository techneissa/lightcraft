//! Issue #372: after spots and retouching, a later edit (a slider, an effect, a brush mask, a stroke) drawn as a drag's
//! drafts at other sizes and then at rest makes no linear stage again (spots, retouching, white balance, noise
//! reduction): every draft takes the image the full render kept, and the full render after the drag still finds it.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, Spot, SpotFill, SpotMode};
use lightcraft_geom::Point;
use lightcraft_gpu::{drafts_resampled, linear_stages_made};
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
/// resampling and its noise reduction at another scale.
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
    let off = mean_difference(&kept, &afresh);
    assert!(off < 2.0, "on average within two levels of 255: {off}");
}

fn mean_difference(a: &Rgba8, b: &Rgba8) -> f64 {
    let sum: u64 = a.data.iter().zip(&b.data).map(|(p, q)| (0..3).map(|k| u64::from(p[k].abs_diff(q[k]))).sum::<u64>()).sum();
    sum as f64 / (a.data.len().max(1) * 3) as f64
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
    let made = linear_stages_made();
    for t in [0.2, 0.4, 0.6] {
        s.light.exposure = t;
        lightcraft_gpu::render(&src, &info, &s, &RenderRequest { quality: Quality::Draft, ..window }, Some(&cache)).expect("gpu");
    }
    lightcraft_gpu::render(&src, &info, &s, &window, Some(&cache)).expect("gpu");
    assert_eq!(linear_stages_made() - made, 0);
}
