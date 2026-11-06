//! Masks on the device: many strokes, Erase, Auto Mask, pen pressure and submasks equal the CPU's;
//! an effect drag evaluates no mask and a new stroke stamps one stroke.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_pipeline::masks::{mask_evaluations, strokes_rasterized};
use lightcraft_pipeline::{RenderRequest, SourceInfo, StageCache, render};
use lightcraft_raster::{Rgb32f, Rgba8};

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        let why = lightcraft_gpu::unavailable_reason().unwrap_or_default();
        assert!(!why.contains("kernels failed to build"), "{why}");
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

fn scene(w: usize, h: usize) -> Arc<Rgb32f> {
    Arc::new(lightcraft_scenes::demo_library()[0].render(w, h))
}

fn diff(a: &Rgba8, b: &Rgba8) -> (f64, u8) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut sum, mut max) = (0u64, 0u8);
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            sum += d as u64;
            max = max.max(d);
        }
    }
    (sum as f64 / (a.data.len() * 3) as f64, max)
}

/// `n` strokes of every kind: soft and hard, erasing every sixth, and (with `pressure`) pen pressure on some.
fn strokes(n: usize, auto: bool, pressure: bool) -> Vec<BrushStroke> {
    (0..n)
        .map(|i| {
            let t = i as f64 / n.max(1) as f64;
            let (x, y) = (0.15 + 0.7 * ((t * 7.3) % 1.0), 0.15 + 0.5 * ((t * 3.1) % 1.0));
            BrushStroke {
                points: vec![Point::new(x - 0.12, y), Point::new(x, y + 0.08), Point::new(x + 0.15, y - 0.03)],
                size: 0.02 + 0.03 * (i % 3) as f64,
                feather: if i % 2 == 0 { 0.0 } else { 80.0 },
                flow: 50.0 + 12.0 * (i % 4) as f64,
                density: if i % 5 == 0 { 70.0 } else { 100.0 },
                erase: i % 6 == 5,
                auto_mask: auto && i % 2 == 0,
                pressure: if pressure && i % 2 == 1 { vec![0.3, 1.0, 0.5] } else { Vec::new() },
            }
        })
        .collect()
}

/// A stroke across the photo that lays about `dabs` dabs (a tiny brush, dabs a pixel apart).
fn dense(dabs: usize, y: f64) -> BrushStroke {
    // 720 px wide render: one dab per 0.72 px, so the path is `dabs * 0.72` px long, zigzagging
    let legs = (dabs as f64 * 0.72 / 500.0).ceil() as usize + 1;
    let points = (0..=legs).map(|k| Point::new(if k % 2 == 0 { 0.15 } else { 0.85 }, y + 0.005 * (k % 4) as f64)).collect();
    BrushStroke { points, size: 0.004, feather: 30.0, ..Default::default() }
}

fn comp(op: MaskOp, shape: MaskShape) -> MaskComponent {
    MaskComponent { name: None, op, invert: false, shape }
}

fn with_masks(masks: Vec<Mask>) -> DevelopSettings {
    DevelopSettings { masks, ..Default::default() }
}

fn adjust() -> LocalAdjustments {
    LocalAdjustments { exposure: 1.0, saturation: -40.0, contrast: 20.0, ..Default::default() }
}

/// The GPU's render equals the CPU's: many strokes with Erase, a part past the old 4,096-dab limit, pen
/// pressure, Auto Mask, submasks (a second brush part to subtract, a gradient to intersect), an inverted
/// mask and an Amount.
#[test]
fn many_strokes_pressure_erase_auto_and_submasks_match_the_cpu() {
    if !gpu() {
        return;
    }
    let src = scene(960, 640);
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(720, 720);
    let cases: Vec<(&str, DevelopSettings)> = vec![
        (
            "40 strokes with erase",
            with_masks(vec![Mask {
                components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(40, false, false) })],
                adjust: adjust(),
                ..Default::default()
            }]),
        ),
        (
            "past 4096 dabs",
            with_masks(vec![Mask {
                components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: vec![dense(6000, 0.2), dense(6000, 0.4)] })],
                adjust: adjust(),
                ..Default::default()
            }]),
        ),
        (
            "pen pressure",
            with_masks(vec![Mask {
                components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(12, false, true) })],
                adjust: adjust(),
                ..Default::default()
            }]),
        ),
        (
            "auto mask, 6 strokes",
            with_masks(vec![Mask {
                components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(6, true, false) })],
                adjust: adjust(),
                ..Default::default()
            }]),
        ),
        (
            "submasks and amount",
            with_masks(vec![Mask {
                components: vec![
                    comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(10, false, true) }),
                    comp(MaskOp::Subtract, MaskShape::Brush { strokes: strokes(4, false, false) }),
                    comp(MaskOp::Intersect, MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.7) }),
                ],
                adjust: LocalAdjustments { amount: 60.0, ..adjust() },
                ..Default::default()
            }]),
        ),
        (
            "inverted mask, auto first",
            with_masks(vec![Mask {
                invert: true,
                components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(4, true, false) })],
                adjust: adjust(),
                ..Default::default()
            }]),
        ),
    ];
    let plain = render(&src, &info, &DevelopSettings::default(), &req).image;
    for (name, s) in cases {
        let cpu = render(&src, &info, &s, &req).image;
        assert!(diff(&cpu, &plain).0 > 0.05, "{name}: the mask changes nothing, so the comparison proves nothing");
        let gpu = lightcraft_gpu::render(&src, &info, &s, &req, None)
            .unwrap_or_else(|| panic!("{name}: gpu render failed: {:?}", lightcraft_gpu::last_fallback()))
            .image;
        let (mean, max) = diff(&cpu, &gpu);
        eprintln!("{name:<28} mean {mean:.4}  max {max}");
        assert!(mean < 0.5 && max <= 3, "{name}: mean {mean:.4} LSB, max {max} LSB");
    }
}

/// A slider drag over masks (a mask's own effect, its Amount, the global Exposure) evaluates no mask on the
/// device and renders what a fresh render does.
#[test]
fn an_effect_drag_evaluates_no_mask_on_the_device() {
    if !gpu() {
        return;
    }
    let src = scene(960, 640);
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(720, 720);
    let masks = vec![
        Mask {
            id: 1,
            components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(8, false, false) })],
            adjust: adjust(),
            ..Default::default()
        },
        Mask {
            id: 2,
            components: vec![comp(
                MaskOp::Add,
                MaskShape::Radial { center: Point::new(0.5, 0.4), rx: 0.2, ry: 0.15, angle: 0.0, feather: 60.0, invert: false },
            )],
            adjust: LocalAdjustments { exposure: -0.5, color_hue: 30.0, color_sat: 40.0, ..Default::default() },
            ..Default::default()
        },
    ];
    let mut s = with_masks(masks);
    let cache = StageCache::default();
    lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
    // the kept alphas count with the view's stages, and clearing them frees them
    let (w, h) = (720usize, 480usize);
    let with_alphas = lightcraft_gpu::stage_bytes(&cache);
    cache.clear();
    assert_eq!(lightcraft_gpu::stage_bytes(&cache), 0);
    lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
    assert_eq!(lightcraft_gpu::stage_bytes(&cache), with_alphas);
    assert!(with_alphas >= 2 * w * h * 4, "two masks' alphas are held: {with_alphas} bytes");
    let mut evaluated = 0;
    for k in 1..7 {
        match k % 3 {
            0 => s.masks[0].adjust.exposure = 0.1 * k as f64,
            1 => s.masks[1].adjust.amount = 100.0 - 9.0 * k as f64,
            _ => s.light.exposure = 0.05 * k as f64,
        }
        let before = mask_evaluations();
        let warm = lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
        evaluated += mask_evaluations() - before;
        let fresh = lightcraft_gpu::render(&src, &info, &s, &req, None).expect("gpu");
        assert_eq!(warm.image, fresh.image, "tick {k}");
    }
    assert_eq!(evaluated, 0, "a slider drag evaluated {evaluated} masks on the device");
    // what a mask is made of does count
    s.masks[1].refine = 40.0;
    let before = mask_evaluations();
    lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
    assert_eq!(mask_evaluations() - before, 1);
}

/// Adding a stroke to a brush part stamps that stroke, not the strokes the mask already holds; an undo is free.
#[test]
fn a_new_stroke_stamps_one_stroke_on_the_device() {
    if !gpu() {
        return;
    }
    let src = scene(960, 640);
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(720, 720);
    let all = strokes(31, false, true);
    let at = |n: usize| {
        with_masks(vec![Mask {
            components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: all[..n].to_vec() })],
            adjust: adjust(),
            ..Default::default()
        }])
    };
    let cache = StageCache::default();
    let stamped = |s: &DevelopSettings| {
        let before = strokes_rasterized();
        let img = lightcraft_gpu::render(&src, &info, s, &req, Some(&cache)).expect("gpu").image;
        (strokes_rasterized() - before, img)
    };
    let (n, first) = stamped(&at(30));
    assert_eq!(n, 30);
    let (n, extended) = stamped(&at(31));
    assert_eq!(n, 1, "one more stroke stamped {n} strokes");
    let fresh = lightcraft_gpu::render(&src, &info, &at(31), &req, None).expect("gpu").image;
    assert_eq!(extended, fresh, "stamping over the held plane equals stamping everything");
    let (n, undone) = stamped(&at(30));
    assert_eq!(n, 0, "back to a held state: no stroke");
    assert_eq!(undone, first);
}
