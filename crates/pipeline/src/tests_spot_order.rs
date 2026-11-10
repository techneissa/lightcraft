//! Issue #184: the Classic spots are drawn on the resampled source, before the white balance, so Temperature and Tint
//! draw none of them again; `spots_after_wb` restores the old order, after it.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot, SpotMode, WbMode};
use lightcraft_geom::Point;
use lightcraft_raster::{Rgb32f, Rgba8};

use crate::spots_keep::{spots_after_wb_here, spots_drawn};
use crate::{Quality, RenderRequest, SourceInfo, StageCache, render, render_cached};

fn scene() -> Arc<Rgb32f> {
    Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640))
}

/// Heal, Clone, Remove (an automatic source) and Content-Aware spots over the photo.
fn spots() -> Vec<Spot> {
    [(0.15, 0.2), (0.3, 0.25), (0.45, 0.3), (0.6, 0.55), (0.75, 0.6), (0.2, 0.7), (0.5, 0.75), (0.85, 0.3)]
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let mode = [SpotMode::Heal, SpotMode::Clone, SpotMode::Remove, SpotMode::Fill][i % 4];
            Spot {
                mode,
                points: vec![Point::new(x, y)],
                size: 0.012,
                feather: 50.0,
                opacity: 100.0,
                source_offset: (mode != SpotMode::Remove).then_some(Point::new(0.04, 0.0)),
                ..Default::default()
            }
        })
        .collect()
}

fn warm(s: &mut DevelopSettings, temp: f64, tint: f64) {
    (s.wb.mode, s.wb.temp, s.wb.tint) = (WbMode::Custom, temp, tint);
}

/// The spots `s` draws through `cache`.
fn drawn(cache: &StageCache, src: &Arc<Rgb32f>, s: &DevelopSettings, req: &RenderRequest) -> usize {
    let before = spots_drawn();
    render_cached(src, &SourceInfo::default(), s, req, cache);
    spots_drawn() - before
}

fn worst(a: &Rgba8, b: &Rgba8) -> u8 {
    a.data.iter().zip(&b.data).fold(0, |w, (p, q)| (0..3).map(|k| p[k].abs_diff(q[k])).fold(w, u8::max))
}

#[test]
fn temperature_and_tint_draw_no_spot_again() {
    let src = scene();
    let full = RenderRequest::fit(960, 960);
    let draft = RenderRequest { quality: Quality::Draft, ..full };
    for (req, what) in [(full, "full"), (draft, "draft")] {
        for after_wb in [false, true] {
            spots_after_wb_here(Some(after_wb));
            let cache = StageCache::isolated();
            let mut s = DevelopSettings { spots: spots(), ..Default::default() };
            assert!(drawn(&cache, &src, &s, &req) > 0, "{what}: the first render draws the spots");
            warm(&mut s, 4300.0, 0.0);
            let temperature = drawn(&cache, &src, &s, &req);
            warm(&mut s, 4300.0, 25.0);
            let tint = drawn(&cache, &src, &s, &req);
            if after_wb {
                assert!(temperature > 0 && tint > 0, "{what}, old order: Temperature and Tint draw the spots again");
            } else {
                assert_eq!((temperature, tint), (0, 0), "{what}: Temperature and Tint draw no spot");
            }
        }
    }
    spots_after_wb_here(None);
}

/// The old order is the render before issue #184: white balance, then the spots on the white-balanced image.
#[test]
fn the_old_order_draws_the_spots_on_the_white_balanced_image() {
    let src = scene();
    let info = SourceInfo::default();
    let mut s = DevelopSettings { spots: spots(), ..Default::default() };
    warm(&mut s, 3600.0, 12.0);
    let req = RenderRequest::fit(960, 960);
    for after_wb in [false, true] {
        spots_after_wb_here(Some(after_wb));
        let p = crate::plan(&src, &info, &s, &req);
        assert_eq!(p.spots_after_wb, after_wb);
        let mut img = p.frame.sample(&src, p.w, p.h);
        let mut by_hand = img.clone();
        crate::lin_cpu_pre_retouch(&mut img, &info, &p);
        let spot = |i: &mut Rgb32f| crate::spots::apply(i, &p.settings.spots, &p.frame, p.px_per_long);
        if after_wb {
            crate::local::white_balance(&mut by_hand, &info, &s);
            spot(&mut by_hand);
        } else {
            spot(&mut by_hand);
            crate::local::white_balance(&mut by_hand, &info, &s);
        }
        assert!(img.data == by_hand.data, "after_wb {after_wb}: the spots and the white balance in that order");
        // (and the view's kept layers give the same)
        let mut kept = p.frame.sample(&src, p.w, p.h);
        crate::spots_keep::lin_cpu_pre_retouch(&mut kept, &info, &p, None);
        assert!(kept.data == by_hand.data, "after_wb {after_wb}: the kept layers draw the same");
    }
    spots_after_wb_here(None);
}

/// Heal and Clone blend in linear light, which the white balance (a 3×3 matrix) keeps: a spot drawn before it matches
/// its surroundings as one drawn after it does.
#[test]
fn heal_and_clone_match_their_surroundings_in_either_order() {
    let src = scene();
    let info = SourceInfo::default();
    let mut s = DevelopSettings {
        spots: spots().into_iter().filter(|sp| matches!(sp.mode, SpotMode::Heal | SpotMode::Clone)).collect(),
        ..Default::default()
    };
    warm(&mut s, 3200.0, 20.0);
    let req = RenderRequest::fit(960, 960);
    spots_after_wb_here(Some(true));
    let old = render(&src, &info, &s, &req).image;
    spots_after_wb_here(Some(false));
    let new = render(&src, &info, &s, &req).image;
    spots_after_wb_here(None);
    let w = worst(&old, &new);
    assert!(w <= 2, "Heal and Clone are {w} levels from the old order");
}
