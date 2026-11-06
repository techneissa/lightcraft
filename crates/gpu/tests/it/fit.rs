//! The display fit on the device (`try_render_fit`): the finished picture shrunk with a Lanczos3 filter before the readback equals the
//! CPU's fit of the same picture (`fit_for_display`: the 8-bit values as floats, `resample::fit`, rounded), within a level of rounding.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{Overlay, PixelWindow, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::resample::{Filter, fit};
use lightcraft_raster::{Image, Rgba8};

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// What `lightcraft_engine::media::fit_for_display` makes.
fn cpu_fit(img: &Rgba8, max_w: usize, max_h: usize) -> Rgba8 {
    let f = Image { width: img.width, height: img.height, data: img.data.iter().map(|p| p.map(f32::from)).collect() };
    let small = fit(&f, max_w.max(1), max_h.max(1), Filter::Lanczos3);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Rgba8 { width: small.width, height: small.height, data: small.data.iter().map(|p| p.map(c)).collect() }
}

fn worst(a: &Rgba8, b: &Rgba8) -> (u8, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut max, mut sum) = (0u8, 0u64);
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..4 {
            let d = p[c].abs_diff(q[c]);
            max = max.max(d);
            sum += u64::from(d);
        }
    }
    (max, sum as f64 / (a.data.len() * 4) as f64)
}

#[test]
fn the_device_fit_equals_the_cpu_fit() {
    if !gpu() {
        return;
    }
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(1400, 933));
    let info = SourceInfo { raw: true, ..Default::default() };
    let s = DevelopSettings::default();
    let req = RenderRequest::fit(1200, 1200);
    let cache = StageCache::default();
    // a fit of 1 %, the loupe's usual (its render is a little larger than it is drawn), a half, and a long, thin one
    for (fw, fh) in [(1188, 792), (1100, 1100), (600, 600), (1200, 200), (333, 777)] {
        let (full, fitted) = lightcraft_gpu::try_render_fit(&src, &info, &s, &req, Some(&cache), (fw, fh)).expect("gpu render");
        let fitted = fitted.unwrap_or_else(|| panic!("{fw}×{fh}: no fit on the device"));
        let want = cpu_fit(&full.image, fw, fh);
        let (max, mean) = worst(&fitted, &want);
        eprintln!("{fw}×{fh} -> {}×{}: max {max}, mean {mean:.5}", fitted.width, fitted.height);
        assert!(max <= 1 && mean < 0.05, "{fw}×{fh}: the device's fit differs from the CPU's by {max} (mean {mean:.4})");
        assert!(fitted.data.iter().all(|p| p[3] == 255), "{fw}×{fh}: alpha");
        // the full picture is the one a render without the fit gives
        let plain = lightcraft_gpu::try_render(&src, &info, &s, &req, Some(&cache)).expect("gpu").image;
        assert_eq!(full.image, plain, "{fw}×{fh}: the full picture is unchanged");
    }
}

// Given a picture that already fits, or a diagnostic overlay (drawn on the CPU after the readback), the device makes no fit: the caller
// fits on the CPU, as before
#[test]
fn the_device_makes_no_fit_where_the_cpu_must() {
    if !gpu() {
        return;
    }
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(1400, 933));
    let info = SourceInfo { raw: true, ..Default::default() };
    let s = DevelopSettings::default();
    let req = RenderRequest::fit(800, 800);
    let fits = lightcraft_gpu::try_render_fit(&src, &info, &s, &req, None, (800, 800)).expect("gpu").1;
    assert!(fits.is_none(), "it already fits");
    let larger = lightcraft_gpu::try_render_fit(&src, &info, &s, &req, None, (4000, 4000)).expect("gpu").1;
    assert!(larger.is_none(), "never enlarged");
    let spots = RenderRequest { overlay: Overlay::Spots(50), ..req };
    assert!(lightcraft_gpu::try_render_fit(&src, &info, &s, &spots, None, (400, 400)).expect("gpu").1.is_none(), "an overlay is drawn after");
    // a window's picture is fitted like any other (what it is cut from is the render's own business)
    let window = RenderRequest { window: Some(PixelWindow { x: 100, y: 100, w: 300, h: 200 }), ..req };
    let (full, fitted) = lightcraft_gpu::try_render_fit(&src, &info, &s, &window, None, (200, 200)).expect("gpu");
    let fitted = fitted.expect("a window fits too");
    assert!(worst(&fitted, &cpu_fit(&full.image, 200, 200)).0 <= 1);
}
