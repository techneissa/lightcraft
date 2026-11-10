//! Issue #176 on the device: zoomed in, a pan to another window of the same scale draws no spot again, whether the
//! device draws them (Heal, Clone, Remove) or the CPU does (Content-Aware): the window pastes the spots another window
//! finished, and the picture is the whole frame's.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Spot, SpotMode};
use lightcraft_geom::Point;
use lightcraft_pipeline::spots_keep::spots_drawn;
use lightcraft_pipeline::spots_scale::windows_pasted;
use lightcraft_pipeline::{PixelWindow, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::{Rgb32f, Rgba8};

fn spots(fill: bool) -> Vec<Spot> {
    [(0.15, 0.2), (0.3, 0.25), (0.45, 0.3), (0.6, 0.55), (0.75, 0.6), (0.2, 0.7), (0.5, 0.75), (0.4, 0.45)]
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let mode = if fill && i == 3 { SpotMode::Fill } else { [SpotMode::Heal, SpotMode::Clone, SpotMode::Remove][i % 3] };
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

fn window(x: usize, y: usize) -> RenderRequest {
    RenderRequest { window: Some(PixelWindow { x, y, w: 240, h: 160 }), ..RenderRequest::fit(960, 960) }
}

fn worst(part: &Rgba8, whole: &Rgba8, x: usize, y: usize) -> u8 {
    let mut worst = 0;
    for r in 0..part.height {
        for c in 0..part.width {
            let (a, b) = (part.data[r * part.width + c], whole.data[(y + r) * whole.width + x + c]);
            worst = (0..3).map(|k| a[k].abs_diff(b[k])).fold(worst, u8::max);
        }
    }
    worst
}

#[test]
fn a_pan_draws_no_spot_again_on_the_device() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    for fill in [false, true] {
        let what = if fill { "with a Content-Aware spot" } else { "Heal, Clone and Remove" };
        let s = DevelopSettings { spots: spots(fill), ..Default::default() };
        let cache = StageCache::default();
        let whole = lightcraft_gpu::render(&src, &info, &s, &RenderRequest::fit(960, 960), None).expect("gpu").image;
        for (x, y) in [(100, 100), (500, 300)] {
            lightcraft_gpu::render(&src, &info, &s, &window(x, y), Some(&cache)).expect("gpu");
        }
        // (new windows over spots those two drew)
        for (x, y) in [(110, 110), (90, 95), (480, 310)] {
            let (drawn, pasted) = (spots_drawn(), windows_pasted());
            let part = lightcraft_gpu::render(&src, &info, &s, &window(x, y), Some(&cache)).expect("gpu").image;
            assert_eq!((spots_drawn() - drawn, windows_pasted() - pasted), (0, 1), "{what}: a pan to {x}, {y} pastes the spots");
            let w = worst(&part, &whole, x, y);
            assert!(w <= 3, "{what}: the window at {x}, {y} is {w} levels from the whole frame");
        }
    }
}

/// Issue #184: the Classic spots are drawn before the white balance on the device too, so Temperature and Tint draw
/// none again (the old order, `spots_after_wb`, draws them all), and the device's picture is the CPU's in both orders.
#[test]
fn temperature_and_tint_draw_no_spot_on_the_device() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let src: Arc<Rgb32f> = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let info = SourceInfo { raw: true, ..Default::default() };
    let full = RenderRequest::fit(960, 960);
    let draft = RenderRequest { quality: lightcraft_pipeline::Quality::Draft, ..full };
    for (req, what) in [(full, "full"), (draft, "draft")] {
        for after_wb in [false, true] {
            lightcraft_pipeline::spots_keep::spots_after_wb_here(Some(after_wb));
            let cache = StageCache::default();
            let mut s = DevelopSettings { spots: spots(false), ..Default::default() };
            let drawn = |s: &DevelopSettings| {
                let before = spots_drawn();
                let img = lightcraft_gpu::render(&src, &info, s, &req, Some(&cache)).expect("gpu").image;
                (spots_drawn() - before, img)
            };
            assert!(drawn(&s).0 > 0, "{what}: the first render draws the spots");
            (s.wb.mode, s.wb.temp, s.wb.tint) = (lightcraft_develop::WbMode::Custom, 4300.0, 0.0);
            let (temperature, _) = drawn(&s);
            s.wb.tint = 25.0;
            let (tint, gpu) = drawn(&s);
            if after_wb {
                assert!(temperature > 0 && tint > 0, "{what}, old order: Temperature and Tint draw the spots again");
            } else {
                assert_eq!((temperature, tint), (0, 0), "{what}: Temperature and Tint draw no spot on the device");
            }
            let cpu = lightcraft_pipeline::render(&src, &info, &s, &req).image;
            let w = worst(&gpu, &cpu, 0, 0);
            assert!(w <= 3, "{what}, after_wb {after_wb}: the device is {w} levels from the CPU");
        }
    }
    lightcraft_pipeline::spots_keep::spots_after_wb_here(None);
}
