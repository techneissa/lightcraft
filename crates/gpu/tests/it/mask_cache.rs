//! Masks on the device: many strokes, Erase, Auto Mask, pen pressure and submasks equal the CPU's;
//! an effect drag evaluates no mask and a new stroke stamps one stroke.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk, mask_evaluations, strokes_rasterized};
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

/// The masks' alphas the loupe tints (`mask_planes`): the device's equal the CPU's for strokes, a gradient, an Amount and a window,
/// they come from the alphas the photo's render kept (no mask is evaluated again), and they are a mask's alpha, not an image.
#[test]
fn the_device_gives_the_masks_alphas_the_cpu_does() {
    if !gpu() {
        return;
    }
    let src = scene(960, 640);
    let info = SourceInfo { raw: true, ..Default::default() };
    let masks = vec![
        Mask {
            id: 3,
            components: vec![comp(MaskOp::Add, MaskShape::Brush { strokes: strokes(6, false, false) })],
            adjust: LocalAdjustments { amount: 60.0, ..adjust() },
            ..Default::default()
        },
        Mask {
            id: 9,
            components: vec![comp(
                MaskOp::Add,
                MaskShape::Radial { center: Point::new(0.5, 0.4), rx: 0.2, ry: 0.15, angle: 0.0, feather: 60.0, invert: false },
            )],
            adjust: adjust(),
            ..Default::default()
        },
        // (hidden: no plane)
        Mask {
            id: 4,
            visible: false,
            components: vec![comp(MaskOp::Add, MaskShape::Linear { start: Point::new(0.5, 0.1), end: Point::new(0.5, 0.9) })],
            ..Default::default()
        },
    ];
    let s = with_masks(masks);
    let window = lightcraft_pipeline::PixelWindow { x: 100, y: 60, w: 300, h: 200 };
    for (name, req) in
        [("whole frame", RenderRequest::fit(720, 720)), ("window", RenderRequest { window: Some(window), ..RenderRequest::fit(720, 720) })]
    {
        let cache = StageCache::default();
        // the photo's render first, as the loupe does: it keeps the alphas the planes are read from
        lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
        let before = mask_evaluations();
        let all = PlanesAsk { want: MaskWant::All, known: None };
        let out = lightcraft_gpu::mask_planes(&src, &info, &s, &req, Some(&cache), &all).expect("gpu planes");
        let gpu_planes = out.planes.expect("planes");
        assert_eq!(mask_evaluations() - before, 0, "{name}: the planes evaluated a mask again");
        let cpu = lightcraft_pipeline::mask_planes(&src, &info, &s, &req, None, &all);
        assert_eq!(cpu.signature, out.signature, "{name}: both renderers sign the planes alike");
        let cpu_planes = cpu.planes.expect("planes");
        let ids = |p: &[(u32, lightcraft_raster::Plane)]| p.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        assert_eq!(ids(&gpu_planes), vec![3, 9], "{name}: the visible masks, in order");
        assert_eq!(ids(&gpu_planes), ids(&cpu_planes));
        for ((id, g), (_, c)) in gpu_planes.iter().zip(&cpu_planes) {
            assert_eq!((g.width, g.height), (c.width, c.height), "{name}: mask {id}");
            let max = g.data.iter().zip(&c.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(max < 0.02, "{name}: mask {id} differs by {max}");
            assert!(g.data.iter().any(|v| *v > 0.5), "{name}: mask {id} selects something");
        }
        if name == "window" {
            assert_eq!((gpu_planes[0].1.width, gpu_planes[0].1.height), (window.w, window.h), "cut to the window");
        }
        // a selected mask that is hidden still has its plane: the device has it not, the CPU evaluates it, and both agree
        let hidden = PlanesAsk { want: MaskWant::One(4), known: None };
        let g = lightcraft_gpu::mask_planes(&src, &info, &s, &req, Some(&cache), &hidden).expect("hidden").planes.expect("planes");
        let c = lightcraft_pipeline::mask_planes(&src, &info, &s, &req, None, &hidden).planes.expect("planes");
        assert_eq!(g.len(), 1, "{name}: the hidden mask's plane");
        assert!(g[0].1.data.iter().any(|v| *v > 0.5), "{name}: it selects something");
        let worst = g[0].1.data.iter().zip(&c[0].1.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.02, "{name}: the hidden mask differs by {worst}");
        // one mask reads back one plane; planes the asker holds read back nothing
        let (_, b0) = lightcraft_gpu::readbacks();
        let one = PlanesAsk { want: MaskWant::One(9), known: None };
        let got = lightcraft_gpu::mask_planes(&src, &info, &s, &req, Some(&cache), &one).expect("one");
        let (_, b1) = lightcraft_gpu::readbacks();
        let n = (gpu_planes[0].1.width * gpu_planes[0].1.height) as u64;
        assert_eq!(b1 - b0, n * 4, "{name}: one mask's plane is read back, not the stack");
        assert_eq!(got.planes.expect("planes").len(), 1);
        let held = PlanesAsk { want: MaskWant::One(9), known: Some(got.signature) };
        let again = lightcraft_gpu::mask_planes(&src, &info, &s, &req, Some(&cache), &held).expect("held");
        assert!(again.planes.is_none() && again.signature == got.signature, "{name}: held planes are not made again");
        assert_eq!(lightcraft_gpu::readbacks().1, b1, "{name}: and nothing is read back for them");
        // the Amount scales the plane (60 % at most)
        let top = gpu_planes[0].1.data.iter().copied().fold(0.0f32, f32::max);
        assert!(top <= 0.6 + 1e-3, "{name}: the Amount applies: {top}");
    }
}

/// A mask the device has no kernel for (a stored Subject selection) is evaluated on the CPU from the image read back once; with
/// its alpha kept, the effect ticks that follow read back only the finished picture and its histogram, never the image again.
#[test]
fn a_cached_cpu_mask_reads_no_image_back_on_an_effect_tick() {
    if !gpu() {
        return;
    }
    let src = scene(960, 640);
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(720, 720);
    let side = 64;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as f32 / side as f32 - 0.5, (i / side) as f32 / side as f32 - 0.5);
            (0.2 - x.hypot(y)) * 40.0
        })
        .collect();
    let seg = lightcraft_develop::SegMask::from_logits(side, &logits);
    let mask = Mask {
        id: 1,
        components: vec![comp(MaskOp::Add, MaskShape::Subject { seg: Some(seg), detail: Vec::new(), edge: 0.0 })],
        adjust: adjust(),
        ..Default::default()
    };
    let mut s = with_masks(vec![mask]);
    let cache = StageCache::default();
    let (w, h) = (720u64, 480u64);
    let (n0, b0) = lightcraft_gpu::readbacks();
    lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
    let (_, first) = lightcraft_gpu::readbacks();
    // (the first render reads the image for the mask: 12 bytes a pixel, 16 with the log luminance, besides the output)
    assert!(first - b0 >= w * h * 12, "the first render reads the image back for the CPU mask: {} bytes", first - b0);
    for k in 1..4 {
        s.masks[0].adjust.exposure = 0.2 * k as f64;
        s.light.exposure = 0.05 * k as f64;
        let (n, b) = lightcraft_gpu::readbacks();
        let before = mask_evaluations();
        lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
        let (n2, b2) = lightcraft_gpu::readbacks();
        assert_eq!(mask_evaluations() - before, 0, "tick {k} evaluated the mask again");
        // the output picture (4 bytes a pixel) and the histogram's few bins
        assert!(b2 - b <= w * h * 4 + 64 * 1024, "tick {k} read {} bytes back ({} readbacks)", b2 - b, n2 - n);
    }
    assert!(n0 <= lightcraft_gpu::readbacks().0);
}

/// A view's stages outlive a move to another photo (Auto Sync copies the edits to a photo of the same size). The alphas kept
/// for masks that read the image (Sky, Luminance and Color Range, Refine Edges, Auto Mask) belong to the picture they were
/// evaluated on: the next photo, or the same photo with a new source (a sharper level, the Denoise mix), shows its own.
#[test]
fn kept_alphas_never_outlive_their_source() {
    if !gpu() {
        return;
    }
    let first = scene(960, 640);
    let other = Arc::new(lightcraft_scenes::demo_library()[1].render(960, 640));
    // the same photo again, with different pixels (a sharper source arriving)
    let sharper = Arc::new(Rgb32f::from_fn(960, 640, |x, y| {
        let p = first.data[y * 960 + x];
        let n = ((x * 7 + y * 13) % 11) as f32 * 0.01;
        [p[0] + n, p[1] + n * 0.5, p[2]]
    }));
    let info = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(720, 720);
    let radial = MaskShape::Radial { center: Point::new(0.5, 0.45), rx: 0.35, ry: 0.3, angle: 0.0, feather: 60.0, invert: false };
    let one = |shape: MaskShape, refine: f64| {
        with_masks(vec![Mask { id: 1, components: vec![comp(MaskOp::Add, shape)], adjust: adjust(), refine, ..Default::default() }])
    };
    let cases: Vec<(&str, DevelopSettings)> = vec![
        ("sky", one(MaskShape::sky(), 0.0)),
        ("luminance range", one(MaskShape::LuminanceRange { lo: 0.2, hi: 0.8, lo_feather: 0.2, hi_feather: 0.2 }, 0.0)),
        ("color range", one(MaskShape::ColorRange { samples: vec![[0.5, 0.02, 0.02], [0.4, -0.03, 0.05]], refine: 50.0 }, 0.0)),
        ("refine edges", one(radial, 60.0)),
        ("auto mask", one(MaskShape::Brush { strokes: strokes(4, true, false) }, 0.0)),
    ];
    for (name, s) in &cases {
        for (label, next) in [("another photo", &other), ("a new source of the photo", &sharper)] {
            let cache = StageCache::default();
            lightcraft_gpu::render(&first, &info, s, &req, Some(&cache)).expect("gpu");
            let warm = lightcraft_gpu::render(next, &info, s, &req, Some(&cache)).expect("gpu").image;
            let fresh = lightcraft_gpu::render(next, &info, s, &req, None).expect("gpu").image;
            let (mean, max) = diff(&warm, &fresh);
            assert!(max == 0, "{name} on {label}: the kept alpha of the earlier picture shows (mean {mean:.4}, max {max})");
            // and the planes the loupe tints
            let ask = PlanesAsk { want: MaskWant::All, known: None };
            let planes = lightcraft_gpu::mask_planes(next, &info, s, &req, Some(&cache), &ask).expect("planes").planes.expect("planes");
            let fresh = lightcraft_gpu::mask_planes(next, &info, s, &req, None, &ask).expect("planes").planes.expect("planes");
            assert_eq!(planes[0].1, fresh[0].1, "{name} on {label}: the planes are the earlier picture's");
        }
    }
}
