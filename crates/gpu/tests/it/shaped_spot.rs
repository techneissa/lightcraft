//! Fork feature (`fork/object-remove.md`): a photo with a shaped Remove spot (in an object's or a mask's shape), or a
//! Content-Aware one, draws that spot on the CPU, and the GPU path and the CPU path give the same picture. Issue #176:
//! only the spot stage runs on the CPU; the rest of the photo stays on the GPU (it used to render whole on the CPU, so
//! every edit, zoom and pan of a photo with one such spot was a CPU render).

use lightcraft_develop::{DevelopSettings, SegMask, Spot, SpotArea, SpotMode};
use lightcraft_geom::Point;
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

/// The GPU's render of `s` and the CPU's differ by at most a few levels.
fn agrees(src: &std::sync::Arc<lightcraft_raster::Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, what: &str) {
    let gpu = match lightcraft_gpu::try_render(src, info, s, req, None) {
        Ok(r) => r.image,
        Err(e) => panic!("{what}: the GPU renders the photo, its spot stage on the CPU ({e:?})"),
    };
    let cpu = render(src, info, s, req).image;
    let worst = cpu.data.iter().zip(&gpu.data).flat_map(|(p, q)| (0..3).map(move |k| p[k].abs_diff(q[k]))).max().unwrap_or(0);
    assert!(worst <= 3, "{what}: CPU and GPU differ by {worst} levels");
}

#[test]
fn a_shaped_or_content_aware_spot_renders_on_the_gpu_and_both_paths_agree() {
    if !lightcraft_gpu::available() {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
        return;
    }
    let src = std::sync::Arc::new(lightcraft_scenes::demo_library()[3].render(600, 400));
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(480, 480);
    let with = |spot: Spot| DevelopSettings { spots: vec![spot], ..Default::default() };
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    assert_ne!(render(&src, &info, &with(shaped(false)), &req).image.data, plain.data, "the spot changes the picture");
    agrees(&src, &info, &with(shaped(false)), &req, "a shaped spot");
    let fill = Spot { mode: SpotMode::Fill, points: vec![Point::new(0.45, 0.5)], size: 0.03, ..Default::default() };
    agrees(&src, &info, &with(fill), &req, "a Content-Aware spot");
    // a LaMa one too (its fill runs on the CPU either way)
    agrees(&src, &info, &with(Spot { fill: lightcraft_develop::SpotFill::Lama, ..shaped(false) }), &req, "a LaMa shaped spot");
    // hidden, it has no effect
    agrees(&src, &info, &with(shaped(true)), &req, "a hidden shaped spot");
}
