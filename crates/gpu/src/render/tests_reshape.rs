//! Fork (`fork/retouch-reshape.md`): the device's geometry resample applies Face Reshape itself (`sample_warp`
//! with the field table), and matches the CPU's `Frame::sample`.

use std::sync::Arc;

use lightcraft_develop::retouch::reshape::sketch_mesh;
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_pipeline::{RenderRequest, SourceInfo};
use lightcraft_raster::Rgb32f;

use super::{Cx, sample};

#[test]
fn the_device_resamples_a_reshaped_face_like_the_cpu() {
    // (creating the device writes the crash sentinel another test swaps out)
    let _serial = crate::DEVICE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(gpu) = crate::device() else {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    };
    let _scope = crate::ctx::RenderScope::new(gpu);
    let (w, h) = (300, 200);
    let src = Arc::new(Rgb32f::from_fn(w, h, |x, y| {
        let line = x % 12 < 2 || y % 10 < 2;
        if line { [0.04, 0.04, 0.05] } else { [0.2 + x as f32 / 600.0, 0.3, 0.45 - y as f32 / 600.0] }
    }));
    let b = NormRect::new(0.3, 0.15, 0.62, 0.8);
    let face = Face { landmarks: sketch_mesh(b), ..Face::boxed(b) };
    let info = SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() };
    let mut alone = DevelopSettings::default();
    controls::set(&mut alone, "retouch.reshape.slimFace", 100.0);
    controls::set(&mut alone, "retouch.reshape.eyeSize", 100.0);
    controls::set(&mut alone, "retouch.reshape.chin", -100.0);
    let mut with_lens = alone.clone();
    with_lens.optics.distortion = 40.0;
    with_lens.optics.ca_red = 60.0;
    for (name, s) in [("alone", &alone), ("with a lens distortion and CA", &with_lens)] {
        for (rw, rh) in [(300, 200), (150, 100)] {
            let p = lightcraft_pipeline::plan(&src, &info, s, &RenderRequest::fit(rw, rh));
            assert!(p.frame.warp.as_ref().is_some_and(|w| w.reshape.is_some()), "{name}: a reshape");
            let want = p.frame.sample(&src, p.w, p.h);
            let mut cx = Cx::new(gpu);
            let buf = Arc::new(gpu.upload(&src.data.iter().flatten().copied().collect::<Vec<f32>>()));
            let out = sample(&mut cx, &src, buf, &p);
            let got = cx.read_rgb(&out, p.w, p.h);
            let mut plain = p.frame.clone();
            if let Some(wp) = &mut plain.warp {
                wp.reshape = None;
            }
            let unshaped = plain.sample(&src, p.w, p.h);
            let (mut worst, mut moved) = (0.0f32, 0);
            for ((g, c), u) in got.data.iter().zip(&want.data).zip(&unshaped.data) {
                worst = worst.max(g.iter().zip(c).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max));
                moved += usize::from(g.iter().zip(u).any(|(a, b)| (a - b).abs() > 0.02));
            }
            assert!(worst < 2e-3, "{name} {rw}×{rh}: the device differs from the CPU by {worst}");
            assert!(moved > 100, "{name} {rw}×{rh}: the device shows the reshape ({moved} px)");
        }
    }
}
