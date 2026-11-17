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
    // (issue #302) a teal gel with a pattern and haze on one light, a red gel with blinds and a magenta one with leaves on others
    let mut effects = lit(20.0, 0.0);
    {
        let e = &mut effects.relight.lights[0].effects;
        (e.gel_hue, e.gel_strength, e.pattern, e.pattern_scale, e.haze) = (178.0, 80.0, 1.0, 60.0, 50.0);
    }
    effects.relight.lights.push(LightSettings { amount: 70.0, x: 0.75, y: 0.4, depth: 30.0, ..LightSettings::default() });
    effects.relight.lights.push(LightSettings { amount: 60.0, x: 0.5, y: 0.2, depth: 50.0, ..LightSettings::default() });
    {
        let e = &mut effects.relight.lights[1].effects;
        (e.gel_hue, e.gel_strength, e.pattern, e.pattern_rotation) = (0.0, 100.0, 3.0, 20.0);
        let e = &mut effects.relight.lights[2].effects;
        (e.gel_hue, e.gel_strength, e.pattern) = (305.0, 80.0, 4.0);
    }
    let req = RenderRequest::fit(720, 720);
    for (name, s) in [
        ("white light", lit(0.0, 0.0)),
        ("warm, darkened", warm),
        ("cool, cropped", cropped),
        ("after a mask", masked),
        ("warm and cool", pair),
        ("gels, patterns, haze", effects),
    ] {
        let off = render(&src, &SourceInfo { relight: None, ..info.clone() }, &s, &req).image;
        assert_ne!(render(&src, &info, &s, &req).image.data, off.data, "{name}: the light changed nothing");
        close(&src, &info, &s, &req, name);
    }
}

/// The GPU's render of `s` equals the CPU's.
fn close(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, name: &str) {
    let cpu = render(src, info, s, req).image;
    let gpu = lightcraft_gpu::render(src, info, s, req, None).expect("gpu render").image;
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

/// One light of each type (issue #300): the GPU equals the CPU.
#[test]
fn relight_types_match_cpu() {
    if !gpu() {
        return;
    }
    let (src, info) = source();
    let req = RenderRequest::fit(720, 720);
    let numbers: [(&str, &[(&str, f64)]); 5] = [
        ("spot", &[("kind", 1.0), ("spotAimX", 0.7), ("spotAimY", 0.6), ("spotAngle", 25.0)]),
        ("sun", &[("kind", 2.0), ("sunAzimuth", 60.0), ("sunElevation", 30.0)]),
        ("softbox", &[("kind", 3.0), ("boxWidth", 0.4), ("boxHeight", 0.25), ("boxRotation", 30.0)]),
        ("rim", &[("kind", 4.0)]),
        ("fill", &[("kind", 5.0)]),
    ];
    for (name, set) in numbers {
        let mut s = lit(20.0, 0.0);
        for (field, v) in set {
            assert!(lightcraft_develop::relight::set(&mut s, &format!("relight.{field}"), *v));
        }
        let off = render(&src, &SourceInfo { relight: None, ..info.clone() }, &s, &req).image;
        assert_ne!(render(&src, &info, &s, &req).image.data, off.data, "{name}: the light changed nothing");
        close(&src, &info, &s, &req, name);
    }
}
