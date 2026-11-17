//! Relight on the device (fork, issue #288, `fork/relight.md`): the light's planes go up with the photo's masks
//! and the render equals the CPU's within the equivalence bounds (mean < 0.5, max ≤ 3 levels of 8 bits), with a
//! warm light (its colour plane too), a crop and a mask of the photo's own before the light's.

use std::sync::Arc;

use lightcraft_develop::relight::LightSettings;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::{Point, Rect};
use lightcraft_pipeline::{RenderRequest, SourceInfo, relight, render};
use lightcraft_raster::Rgb32f;
use lightcraft_relight::Field;

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// A demo photo with a field: a nearer box at log depth −0.8 in front of a wall that tilts away to the right.
fn source() -> (Arc<Rgb32f>, SourceInfo) {
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(960, 640));
    let (fw, fh) = (240, 160);
    let lz = (0..fw * fh)
        .map(|i| {
            let (u, v) = ((i % fw) as f32 / fw as f32, (i / fw) as f32 / fh as f32);
            if (0.2..0.45).contains(&u) && (0.25..0.7).contains(&v) { -0.8 } else { 0.4 * u }
        })
        .collect();
    let field = Field::from_parts(fw, fh, lz, vec![-2.0; fw * fh]).unwrap();
    (src, SourceInfo { raw: true, relight: Some(relight::Input { field: Arc::new(field), key: 11 }), ..SourceInfo::default() })
}

fn lit(warmth: f64, darken: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.relight.lights[0] = LightSettings { amount: 80.0, x: 0.3, y: 0.45, depth: 15.0, warmth, ..LightSettings::default() };
    s.relight.darken = darken;
    s
}

#[test]
fn relight_matches_cpu() {
    if !gpu() {
        return;
    }
    let (src, info) = source();
    let warm = lit(60.0, 40.0);
    let mut cropped = lit(-40.0, 0.0);
    cropped.crop.geometry.rect = Rect { x0: 0.1, y0: 0.15, x1: 0.8, y1: 0.9 };
    let mut masked = lit(30.0, 0.0);
    masked.masks = vec![Mask {
        id: 1,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Radial { center: Point::new(0.6, 0.5), rx: 0.3, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
        }],
        adjust: LocalAdjustments { exposure: -0.5, ..LocalAdjustments::default() },
        ..Mask::default()
    }];
    // a warm key and a cool fill together (the colour goes up as two planes)
    let mut pair = lit(70.0, 20.0);
    pair.relight.lights.push(LightSettings { amount: 60.0, warmth: -80.0, x: 0.8, y: 0.3, depth: 40.0, softness: 30.0, ..LightSettings::default() });
    let req = RenderRequest::fit(720, 720);
    for (name, s) in
        [("white light", lit(0.0, 0.0)), ("warm, darkened", warm), ("cool, cropped", cropped), ("after a mask", masked), ("warm and cool", pair)]
    {
        let cpu = render(&src, &info, &s, &req).image;
        let off = render(&src, &SourceInfo { relight: None, ..info.clone() }, &s, &req).image;
        assert_ne!(cpu.data, off.data, "{name}: the light changed nothing");
        let gpu = lightcraft_gpu::render(&src, &info, &s, &req, None).expect("gpu render").image;
        assert_eq!((cpu.width, cpu.height), (gpu.width, gpu.height), "{name}");
        let (mut sum, mut max) = (0u64, 0u8);
        for (p, q) in cpu.data.iter().zip(&gpu.data) {
            for c in 0..3 {
                let d = p[c].abs_diff(q[c]);
                sum += u64::from(d);
                max = max.max(d);
            }
        }
        let mean = sum as f64 / (cpu.data.len() * 3) as f64;
        eprintln!("relight {name:<16} mean {mean:.4}  max {max}");
        assert!(mean < 0.5 && max <= 3, "{name}: mean {mean:.4}, max {max}");
    }
}
