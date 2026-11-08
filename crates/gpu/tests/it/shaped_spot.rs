//! Fork feature (`fork/object-remove.md`): a photo with a shaped Remove spot (in an object's or a mask's shape) renders
//! on the CPU, as one with a Content-Aware spot does, so the GPU path and the CPU path give the same picture.

use lightcraft_develop::{DevelopSettings, SegMask, Spot, SpotArea, SpotMode};
use lightcraft_geom::Point;
use lightcraft_gpu::Fallback;
use lightcraft_pipeline::{RenderRequest, SourceInfo, render};

fn shaped(hidden: bool) -> Spot {
    // a blob in the middle of a 32-cell grid over [0.3, 0.3]–[0.6, 0.7]
    let logits: Vec<f32> = (0..32 * 32)
        .map(|i| {
            let (x, y) = ((i % 32) as f32 - 15.5, (i / 32) as f32 - 15.5);
            if x.hypot(y) < 10.0 { 8.0 } else { -8.0 }
        })
        .collect();
    let seg = SegMask::from_logits_in(32, &logits, [0.3, 0.3, 0.6, 0.7]);
    Spot {
        mode: SpotMode::Remove,
        points: vec![Point::new(0.45, 0.5)],
        size: 0.005,
        area: Some(SpotArea::new("object", seg)),
        hidden,
        ..Default::default()
    }
}

#[test]
fn a_shaped_spot_renders_on_the_cpu_and_both_paths_agree() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let src = std::sync::Arc::new(lightcraft_scenes::demo_library()[3].render(600, 400));
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(480, 480);
    let with = |hidden: bool| DevelopSettings { spots: vec![shaped(hidden)], ..Default::default() };
    // the GPU leaves it to the CPU, so the app's render (GPU, then CPU) is the CPU's picture
    assert!(matches!(lightcraft_gpu::try_render(&src, &info, &with(false), &req, None), Err(Fallback::NotTried)));
    let cpu = render(&src, &info, &with(false), &req).image;
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    assert_ne!(cpu.data, plain.data, "the spot changes the picture");
    // hidden, it has no effect and the GPU renders the photo, as the CPU does
    let gpu = lightcraft_gpu::try_render(&src, &info, &with(true), &req, None).expect("a hidden shaped spot renders on the GPU").image;
    let cpu = render(&src, &info, &with(true), &req).image;
    let worst = cpu.data.iter().zip(&gpu.data).flat_map(|(p, q)| (0..3).map(move |k| p[k].abs_diff(q[k]))).max().unwrap_or(0);
    assert!(worst <= 3, "CPU and GPU differ by {worst} levels");
}
