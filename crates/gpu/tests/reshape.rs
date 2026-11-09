//! Fork (`fork/retouch-reshape.md`): Face Reshape on the GPU renderer. Its warp is sampled on the CPU (the GPU's warp
//! kernel doesn't know it), so the GPU render shows the same reshaped face as the CPU render, within the CPU ↔ GPU
//! bounds of `tests/it/equivalence.rs`.

use std::sync::Arc;

use lightcraft_develop::retouch::reshape::sketch_mesh;
use lightcraft_develop::retouch::{Face, Input, NormRect};
use lightcraft_develop::{DevelopSettings, controls};
use lightcraft_pipeline::{RenderRequest, SourceInfo, render};
use lightcraft_raster::Rgb32f;

#[test]
fn a_reshaped_face_renders_alike_on_the_gpu() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let (w, h) = (240, 160);
    let src = Arc::new(Rgb32f::from_fn(w, h, |x, y| {
        let line = x % 12 < 2 || y % 10 < 2;
        if line { [0.04, 0.04, 0.05] } else { [0.2 + x as f32 / 600.0, 0.3, 0.45 - y as f32 / 600.0] }
    }));
    let b = NormRect::new(0.35, 0.2, 0.65, 0.75);
    let face = Face { landmarks: sketch_mesh(b), ..Face::boxed(b) };
    let info = SourceInfo { retouch: Some(Arc::new(Input::new_on_photo(vec![face]))), ..SourceInfo::default() };
    let mut s = DevelopSettings::default();
    controls::set(&mut s, "retouch.reshape.slimFace", 100.0);
    controls::set(&mut s, "retouch.reshape.eyeSize", 100.0);
    let req = RenderRequest::fit(w, h);
    let cpu = render(&src, &info, &s, &req).image;
    let gpu = lightcraft_gpu::try_render(&src, &info, &s, &req, None).expect("GPU render").image;
    let plain = lightcraft_gpu::try_render(&src, &info, &DevelopSettings::default(), &req, None).expect("GPU render").image;
    let (mut sum, mut max, mut moved) = (0u64, 0u8, 0usize);
    for ((c, g), p) in cpu.data.iter().zip(&gpu.data).zip(&plain.data) {
        for k in 0..3 {
            let d = c[k].abs_diff(g[k]);
            sum += u64::from(d);
            max = max.max(d);
        }
        moved += usize::from(g.iter().zip(p).any(|(a, b)| a.abs_diff(*b) > 8));
    }
    let mean = sum as f64 / (cpu.data.len() * 3) as f64;
    assert!(mean < 0.5 && max <= 3, "CPU ↔ GPU: mean {mean}, max {max}");
    assert!(moved > 200, "the GPU render shows the reshape: {moved}");
}
