//! Issue #176: once spots are drawn, a later edit, zoom or pan reuses them. A window of one scale pastes the spots
//! another window of it finished (`spots_scale`) instead of growing to hold them and drawing them again, and the
//! colour noise model, measured on the whole frame before the spots, is measured again for neither.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_raster::Rgb32f;

use crate::spots_keep::spots_drawn;
use crate::spots_scale::windows_pasted;
use crate::{PixelWindow, RenderRequest, SourceInfo, StageCache, plan, render, render_cached};

fn scene() -> Arc<Rgb32f> {
    Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640))
}

fn spot(mode: SpotMode, points: &[(f64, f64)], source: Option<(f64, f64)>) -> Spot {
    Spot {
        mode,
        points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
        size: 0.012,
        feather: 50.0,
        opacity: 100.0,
        source_offset: source.map(|(dx, dy)| Point::new(dx, dy)),
        ..Default::default()
    }
}

/// Heal, Clone, Remove (an automatic source), Content-Aware and a brushed stroke, over the photo.
fn spots() -> Vec<Spot> {
    let mut v = Vec::new();
    for (i, &(x, y)) in [(0.15, 0.2), (0.3, 0.25), (0.45, 0.3), (0.6, 0.55), (0.75, 0.6), (0.2, 0.7), (0.5, 0.75), (0.85, 0.3)].iter().enumerate() {
        let mode = [SpotMode::Heal, SpotMode::Clone, SpotMode::Remove, SpotMode::Fill][i % 4];
        v.push(spot(mode, &[(x, y)], (mode != SpotMode::Remove).then_some((0.04, 0.0))));
    }
    v.push(spot(SpotMode::Heal, &(0..12).map(|k| (0.3 + 0.01 * k as f64, 0.45)).collect::<Vec<_>>(), Some((0.0, -0.06))));
    v
}

fn window(x: usize, y: usize) -> RenderRequest {
    RenderRequest { window: Some(PixelWindow { x, y, w: 240, h: 160 }), ..RenderRequest::fit(960, 960) }
}

/// Renders `req` through `cache`: the spots it drew, whether it pasted them, and the most its pixels differ from the
/// whole frame's render cut to the window.
fn view(cache: &StageCache, src: &Arc<Rgb32f>, s: &DevelopSettings, req: &RenderRequest) -> (usize, bool, u8) {
    let info = SourceInfo::default();
    let (drawn, pasted) = (spots_drawn(), windows_pasted());
    let kept = render_cached(src, &info, s, req, cache).image;
    let (drawn, pasted) = (spots_drawn() - drawn, windows_pasted() > pasted);
    (drawn, pasted, off_whole(src, s, req, &kept))
}

/// The most `img` (a render of `req`'s window) differs from the whole frame's render there.
fn off_whole(src: &Arc<Rgb32f>, s: &DevelopSettings, req: &RenderRequest, img: &lightcraft_raster::Rgba8) -> u8 {
    let whole = render(src, &SourceInfo::default(), s, &RenderRequest { window: None, ..*req }).image;
    let Some(PixelWindow { x, y, .. }) = req.window else { return u8::MAX };
    let mut worst = 0;
    for yy in 0..img.height {
        for xx in 0..img.width {
            let (a, b) = (img.data[yy * img.width + xx], whole.data[(y + yy) * whole.width + x + xx]);
            worst = (0..3).map(|k| a[k].abs_diff(b[k])).fold(worst, u8::max);
        }
    }
    worst
}

#[test]
fn panning_and_zooming_at_one_scale_draws_no_spot_again() {
    let (src, cache) = (scene(), StageCache::default());
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    let (a, b) = (window(100, 100), window(500, 300));
    let (drawn, pasted, _) = view(&cache, &src, &s, &a);
    assert!(drawn > 0 && !pasted, "the first window draws its spots");
    view(&cache, &src, &s, &b);
    // (a window rendered before is the stage cache's; a new one of the scale, over spots drawn before, pastes them)
    for (x, y) in [(300, 200), (120, 260), (150, 150), (450, 320)] {
        let (drawn, pasted, worst) = view(&cache, &src, &s, &window(x, y));
        assert_eq!((drawn, pasted), (0, true), "a new window at {x}, {y}: the spots are pasted, none drawn");
        assert!(worst <= 1, "a new window at {x}, {y}: {worst} levels from a render that draws them");
    }
    // an edit after the spots (Exposure) keeps them too
    s.light.exposure = 0.5;
    assert_eq!(view(&cache, &src, &s, &a).0, 0, "Exposure draws no spot");
    // a spot's own change draws again
    s.spots[0].size = 0.02;
    assert!(view(&cache, &src, &s, &a).0 > 0, "a changed spot draws again");
}

#[test]
fn a_window_draws_its_spots_as_the_whole_frame_does() {
    // (a Content-Aware spot near a window's edge used to read a few pixels past the grown window: 34 levels off)
    let (src, s) = (scene(), DevelopSettings { spots: spots(), ..Default::default() });
    for (x, y) in [(100, 100), (500, 300), (120, 260), (450, 320)] {
        let req = window(x, y);
        let worst = off_whole(&src, &s, &req, &render(&src, &SourceInfo::default(), &s, &req).image);
        assert!(worst <= 1, "window at {x}, {y}: {worst} levels from the whole frame");
    }
}

#[test]
fn the_noise_model_is_keyed_by_the_whole_frame_alone() {
    let src = scene();
    let info = SourceInfo::default();
    let plain = DevelopSettings::default();
    let spotted = DevelopSettings { spots: spots(), ..Default::default() };
    let key = |s: &DevelopSettings, req: &RenderRequest| plan(&src, &info, s, req).noise_key;
    let fit = RenderRequest::fit(960, 960);
    assert_eq!(key(&plain, &fit), key(&spotted, &fit), "spots are drawn after the frame it measures");
    assert_eq!(key(&spotted, &window(100, 100)), key(&spotted, &window(500, 300)), "every window of a frame shares it");
    assert_ne!(key(&plain, &fit), key(&plain, &window(100, 100)), "the whole frame alone may measure its own image");
    let warmer = DevelopSettings {
        wb: lightcraft_develop::WhiteBalance { mode: lightcraft_develop::WbMode::Custom, temp: 7000.0, ..plain.wb },
        ..plain.clone()
    };
    assert_ne!(key(&plain, &fit), key(&warmer, &fit), "the white balance comes before it");
}
