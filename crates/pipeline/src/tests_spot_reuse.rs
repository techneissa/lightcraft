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
    let (src, cache) = (scene(), StageCache::isolated());
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
fn a_pan_back_and_another_scale_keep_the_spots() {
    let (src, cache) = (scene(), StageCache::isolated());
    let s = DevelopSettings { spots: spots(), ..Default::default() };
    let small = |x: usize, y: usize| RenderRequest { window: Some(PixelWindow { x, y, w: 240, h: 160 }), ..RenderRequest::fit(720, 720) };
    view(&cache, &src, &s, &window(100, 100));
    view(&cache, &src, &s, &window(500, 300));
    // another scale (a pinch level): its own spots, once
    assert!(view(&cache, &src, &s, &small(50, 60)).0 > 0, "a new scale draws its spots once");
    view(&cache, &src, &s, &small(380, 230));
    assert_eq!(view(&cache, &src, &s, &small(60, 70)).0, 0, "and pans in it draw none");
    // back at the first scale, after the stage cache let its windows go
    for (x, y) in [(100, 100), (500, 300), (110, 95)] {
        let (drawn, pasted, worst) = view(&cache, &src, &s, &window(x, y));
        assert_eq!((drawn, pasted), (0, true), "back to {x}, {y}: the spots are pasted");
        assert!(worst <= 1, "back to {x}, {y}: {worst} levels from the whole frame");
    }
}

#[test]
fn a_view_let_go_finds_its_spots_again_and_another_photo_never_takes_them() {
    let s = DevelopSettings { spots: spots(), ..Default::default() };
    let first = StageCache::isolated();
    let src = scene();
    view(&first, &src, &s, &window(100, 100));
    let store = first.spot_finals.clone();
    drop(first);
    // (the zoom's view let go at Fit, and made again)
    let again = StageCache { spot_finals: store.clone(), ..StageCache::isolated() };
    assert_eq!(view(&again, &src, &s, &window(110, 105)).0, 0, "a new view of the photo pastes its spots");
    // another photo of the same size with the same settings draws its own
    let other = Arc::new(lightcraft_scenes::demo_library()[1].render(960, 640));
    let theirs = StageCache { spot_finals: store, ..StageCache::isolated() };
    let (drawn, pasted, worst) = view(&theirs, &other, &s, &window(110, 105));
    assert!(drawn > 0 && !pasted, "another photo draws its spots");
    assert!(worst <= 1, "with its own pixels: {worst} levels from its whole frame");
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
fn a_pan_or_a_spot_edit_measures_the_noise_model_once() {
    let (src, cache) = (scene(), StageCache::isolated());
    let mut s = DevelopSettings { spots: spots(), process: lightcraft_develop::ProcessVersion(6), ..Default::default() };
    s.detail.nr_color = 40.0;
    let measured = |s: &DevelopSettings, req: &RenderRequest| {
        let before = crate::noise_measured();
        render_cached(&src, &SourceInfo::default(), s, req, &cache);
        crate::noise_measured() - before
    };
    assert_eq!(measured(&s, &window(100, 100)), 1, "the first window measures it");
    for (x, y) in [(500, 300), (300, 200), (150, 150)] {
        assert_eq!(measured(&s, &window(x, y)), 0, "a pan to {x}, {y} takes it");
    }
    s.spots[0].size = 0.02;
    assert_eq!(measured(&s, &window(100, 100)), 0, "a spot edit takes it");
    s.light.exposure = 0.5;
    assert_eq!(measured(&s, &window(120, 100)), 0, "Exposure takes it");
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

/// Review F1: a window picks an automatic source for each spot without one from a 512-pixel copy of the whole photo;
/// a pan or a zoom must not search again, and spots that read no source (Content-Aware, LaMa) are never searched.
#[test]
fn a_pan_or_a_zoom_searches_no_source() {
    // (it renders a LaMa spot: the fill installed is process-wide, so take the learned-fill tests' turn)
    let _turn = crate::tests_learned_fill::TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (src, cache) = (scene(), StageCache::isolated());
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    // every spot without a source: Remove and Heal find one, Content-Aware and LaMa read none
    // (moved a little: the sources found are kept for the process, and other tests search these spots)
    for sp in &mut s.spots {
        sp.source_offset = None;
        sp.points.iter_mut().for_each(|p| p.y += 0.007);
    }
    s.spots.push(Spot { fill: lightcraft_develop::SpotFill::Lama, ..spot(SpotMode::Remove, &[(0.7, 0.2)], None) });
    let searches = |s: &DevelopSettings, req: &RenderRequest| {
        let before = crate::spots::sources_picked();
        render_cached(&src, &SourceInfo::default(), s, req, &cache);
        crate::spots::sources_picked() - before
    };
    let searched = s.spots.iter().filter(|sp| !matches!(sp.mode, SpotMode::Fill) && sp.fill != lightcraft_develop::SpotFill::Lama).count();
    assert_eq!(searches(&s, &window(100, 100)), searched, "the first window searches each spot that reads a source, once");
    for (x, y) in [(500, 300), (300, 200), (120, 260)] {
        assert_eq!(searches(&s, &window(x, y)), 0, "a pan to {x}, {y} searches nothing");
    }
    let half = RenderRequest { window: Some(PixelWindow { x: 50, y: 60, w: 240, h: 160 }), ..RenderRequest::fit(720, 720) };
    assert_eq!(searches(&s, &half), 0, "another scale searches nothing");
    s.light.exposure = 0.5;
    assert_eq!(searches(&s, &window(130, 110)), 0, "Exposure searches nothing");
}

/// Review F2: a render of the whole frame keeps no finished spots (it has no window to say which are exact); a window of
/// that frame afterwards must still show them.
#[test]
fn a_window_after_the_whole_frame_still_shows_its_spots() {
    let (src, cache) = (scene(), StageCache::isolated());
    // (every spot with its source: the window's plan then changes nothing in them, and both renders share a key)
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    for sp in &mut s.spots {
        sp.source_offset = sp.source_offset.or(Some(Point::new(0.0, 0.05)));
    }
    render_cached(&src, &SourceInfo::default(), &s, &RenderRequest::fit(960, 960), &cache);
    let (drawn, pasted, worst) = view(&cache, &src, &s, &window(100, 100));
    assert!(drawn > 0 && !pasted, "the window draws its spots: nothing was kept for it");
    assert!(worst <= 1, "{worst} levels from the whole frame");
}

/// Review F4: one view renders one photo after another; a second photo of the same size and white balance must measure
/// its own noise model, not take the first one's.
#[test]
fn another_photo_measures_its_own_noise_model() {
    let cache = StageCache::isolated();
    let mut s = DevelopSettings { process: lightcraft_develop::ProcessVersion(6), ..Default::default() };
    s.detail.nr_color = 40.0;
    let (a, b) = (scene(), Arc::new(lightcraft_scenes::demo_library()[1].render(960, 640)));
    let measured = |src: &Arc<Rgb32f>| {
        let before = crate::noise_measured();
        render_cached(src, &SourceInfo::default(), &s, &window(100, 100), &cache);
        crate::noise_measured() - before
    };
    assert_eq!(measured(&a), 1);
    assert_eq!(measured(&b), 1, "the second photo measures its own");
}
