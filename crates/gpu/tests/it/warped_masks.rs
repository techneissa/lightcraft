//! Linear and radial masks through a lens or perspective warp on the device (issue #212): their alphas equal the
//! CPU's within a quarter of an 8-bit level, over the whole frame and in a window, and the render reads no image back
//! for them.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk};
use lightcraft_pipeline::{PixelWindow, RenderRequest, SourceInfo};

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// Barrel distortion and a perspective fix, with a soft and a hard gradient of each kind.
fn settings(masks: bool) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.optics.distortion = -30.0;
    s.geometry.vertical = 25.0;
    s.geometry.rotate = 4.0;
    if !masks {
        return s;
    }
    let shapes = [
        MaskShape::Linear { start: Point::new(0.3, 0.1), end: Point::new(0.6, 0.7) },
        MaskShape::Linear { start: Point::new(0.5, 0.5), end: Point::new(0.5005, 0.5005) },
        MaskShape::Radial { center: Point::new(0.45, 0.55), rx: 0.22, ry: 0.15, angle: 20.0, feather: 50.0, invert: false },
        MaskShape::Radial { center: Point::new(0.7, 0.3), rx: 0.1, ry: 0.06, angle: -35.0, feather: 0.0, invert: true },
    ];
    s.masks = shapes
        .into_iter()
        .enumerate()
        .map(|(i, shape)| Mask {
            id: i as u32 + 1,
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
            adjust: LocalAdjustments { exposure: 1.0, ..Default::default() },
            ..Default::default()
        })
        .collect();
    s
}

#[test]
fn warped_gradients_on_the_device_equal_the_cpu() {
    if !gpu() {
        return;
    }
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(1500, 1000));
    let info = SourceInfo { raw: true, ..Default::default() };
    let s = settings(true);
    let win = PixelWindow { x: 613, y: 157, w: 517, h: 389 };
    for (name, req) in
        [("whole frame", RenderRequest::fit(1500, 1500)), ("window", RenderRequest { window: Some(win), ..RenderRequest::fit(1500, 1500) })]
    {
        let all = PlanesAsk { want: MaskWant::All, known: None };
        let gpu = lightcraft_gpu::mask_planes(&src, &info, &s, &req, None, &all).expect("gpu planes").planes.expect("planes");
        let cpu = lightcraft_pipeline::mask_planes(&src, &info, &s, &req, None, &all).planes.expect("planes");
        assert_eq!(gpu.len(), 4, "{name}");
        for ((id, g), (_, c)) in gpu.iter().zip(&cpu) {
            let worst = g.data.iter().zip(&c.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst * 255.0 <= 0.25, "{name}: mask {id} differs by {} levels", worst * 255.0);
            assert!(c.data.iter().any(|v| *v > 0.5) && c.data.iter().any(|v| *v < 0.5), "{name}: mask {id} has an edge here");
        }
        // the masks read nothing back: a render with them reads back what one without them does
        let read = |s: &DevelopSettings| {
            let b0 = lightcraft_gpu::readbacks().1;
            lightcraft_gpu::try_render(&src, &info, s, &req, None).expect("gpu render");
            lightcraft_gpu::readbacks().1 - b0
        };
        assert_eq!(read(&s), read(&settings(false)), "{name}: the masks read the image back");
    }
}
