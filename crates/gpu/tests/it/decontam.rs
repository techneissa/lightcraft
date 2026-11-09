//! Fork (`fork/perf-decontam-gpu.md`): a decontaminated mask's soft edge on the GPU. It renders as the CPU does
//! (within one 8-bit level), and a drag tick that leaves the image and the mask's alpha alone neither estimates nor
//! uploads the edge again. Skips (passes with a note) when no GPU adapter exists.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_pipeline::{PixelWindow, RenderRequest, SourceInfo, StageCache, render};
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

/// The sky's share at normalised `(nx, ny)`: none inside a head, all far from it, strands of every density along
/// its top.
fn sky_at(nx: f64, ny: f64, aspect: f64) -> f64 {
    let (dx, dy) = ((nx - 0.5) * aspect, ny - 0.62);
    let r = (dx * dx + dy * dy).sqrt();
    let ang = dy.atan2(dx);
    let top = (-dy / r.max(1e-6)).max(0.0);
    let s = 0.5 + 0.5 * (ang * 60.0).sin() * (ang * 17.0 + 1.3).cos();
    let rim = 0.3 + 0.08 * top * s;
    ((r - 0.3) / (rim - 0.3 + 0.01)).clamp(0.0, 1.0)
}

/// Dark hair against a blue sky, the Background matte soft along the strands (`w × h`).
fn hair(w: usize, h: usize) -> (Arc<Rgb32f>, SegMask) {
    let aspect = w as f64 / h as f64;
    let (hair, sky) = ([0.2f32, 0.1, 0.05], [0.1f32, 0.28, 0.7]);
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let a = sky_at((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64, aspect) as f32;
        let tex = 0.85 + 0.15 * ((x * 7 + y * 3) % 5) as f32 / 4.0;
        std::array::from_fn(|c| (1.0 - a) * hair[c] * tex + a * sky[c])
    });
    let side = 256;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let a = sky_at(((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64, aspect).clamp(0.003, 0.997);
            (a / (1.0 - a)).ln() as f32
        })
        .collect();
    (Arc::new(src), SegMask::from_logits(side, &logits))
}

/// A Background mask with `adjust`, decontaminated at `d`.
fn background(seg: &SegMask, adjust: LocalAdjustments, d: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    let shape = MaskShape::Background { seg: Some(seg.clone()), detail: Vec::new(), edge: 0.0 };
    s.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
        adjust,
        decontaminate: d,
        ..Default::default()
    });
    s
}

fn orange() -> LocalAdjustments {
    LocalAdjustments {
        exposure: 0.4,
        temp: 20.0,
        saturation: 30.0,
        recolor: 100.0,
        recolor_r: 1.0,
        recolor_g: 0.55,
        recolor_b: 0.1,
        ..Default::default()
    }
}

/// The largest difference of any channel, and how many pixels differ at all.
fn worst(a: &Rgba8, b: &Rgba8) -> (u8, usize) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let max = a.data.iter().zip(&b.data).flat_map(|(p, q)| (0..3).map(move |c| p[c].abs_diff(q[c]))).max().unwrap_or(0);
    (max, a.data.iter().zip(&b.data).filter(|(p, q)| p != q).count())
}

fn on_gpu(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, cache: Option<&StageCache>) -> Rgba8 {
    lightcraft_gpu::try_render(src, info, s, req, cache).expect("a GPU render, not the CPU fallback").image
}

/// Scene and colour edits apart, on a rendered photo and on a raw's tone map with Contrast, whole and in a zoomed
/// window: the GPU draws the decontaminated edge as the CPU does, within one level.
#[test]
fn a_decontaminated_edge_renders_as_on_the_cpu() {
    if !gpu() {
        return;
    }
    let (w, h) = (480, 320);
    let (src, seg) = hair(w, h);
    let raw = SourceInfo { raw: true, ..Default::default() };
    let whole = RenderRequest::fit(w, h);
    let window = RenderRequest { window: Some(PixelWindow { x: 300, y: 120, w: 320, h: 200 }), ..RenderRequest::fit(960, 640) };
    let colour_only = LocalAdjustments { saturation: -60.0, hue: 20.0, ..Default::default() };
    let cases: [(&str, LocalAdjustments, &SourceInfo, &RenderRequest); 4] = [
        ("scene and colour", orange(), &SourceInfo::default(), &whole),
        ("colour only", colour_only, &SourceInfo::default(), &whole),
        ("raw tone map", orange(), &raw, &whole),
        ("zoomed window", orange(), &SourceInfo::default(), &window),
    ];
    for (name, adjust, info, req) in cases {
        let mut s = background(&seg, adjust, 100.0);
        if info.raw {
            s.process = lightcraft_develop::Process::V4.version();
            s.light.contrast = 60.0;
        }
        let cpu = render(&src, info, &s, req).image;
        let gpu = on_gpu(&src, info, &s, req, None);
        let (max, n) = worst(&cpu, &gpu);
        eprintln!("{name}: max {max}, {n} pixels differ");
        assert!(max <= 1, "{name}: the GPU's decontaminated edge is {max} levels from the CPU's");
        // (the edge is decontaminated: the render differs from one without)
        let mut off = s.clone();
        off.masks[0].decontaminate = 0.0;
        let (apart, _) = worst(&on_gpu(&src, info, &off, req, None), &gpu);
        assert!(apart >= 8, "{name}: decontamination changed the edge by only {apart} levels");
    }
}

/// A drag tick that changes the mask's Amount and Exposure keeps the edge: not estimated, not uploaded again, and
/// drawn as a fresh render draws it.
#[test]
fn a_mask_slider_tick_keeps_the_edge_on_the_device() {
    if !gpu() {
        return;
    }
    let (w, h) = (320, 214);
    let (src, seg) = hair(w, h);
    let info = SourceInfo::default();
    let req = RenderRequest::fit(w, h);
    let cache = StageCache::default();
    let mut s = background(&seg, orange(), 100.0);
    let (estimates, uploads) = (lightcraft_pipeline::decontam::estimates(), lightcraft_gpu::edge_uploads());
    let first = on_gpu(&src, &info, &s, &req, Some(&cache));
    assert_eq!(lightcraft_pipeline::decontam::estimates() - estimates, 1, "the first render estimates the edge");
    assert_eq!(lightcraft_gpu::edge_uploads() - uploads, 1, "and uploads it");
    s.masks[0].adjust.amount = 60.0;
    s.masks[0].adjust.exposure = 0.7;
    s.light.contrast = 20.0;
    let tick = on_gpu(&src, &info, &s, &req, Some(&cache));
    assert_eq!(lightcraft_pipeline::decontam::estimates() - estimates, 1, "a tick estimated the edge again");
    assert_eq!(lightcraft_gpu::edge_uploads() - uploads, 1, "a tick uploaded the edge again");
    assert_ne!(first.data, tick.data);
    // a mask whose alpha changes is estimated again
    let mut moved = s.clone();
    if let MaskShape::Background { edge, .. } = &mut moved.masks[0].components[0].shape {
        *edge = 30.0;
    }
    let _ = on_gpu(&src, &info, &moved, &req, Some(&cache));
    assert_eq!(lightcraft_pipeline::decontam::estimates() - estimates, 2, "a new alpha is a new edge");
    // (a render without a cache estimates its own)
    assert_eq!(tick.data, on_gpu(&src, &info, &s, &req, None).data, "a kept edge renders as a fresh one");
}
