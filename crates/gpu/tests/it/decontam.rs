//! Fork (`fork/perf-decontam-gpu.md`): a decontaminated mask's soft edge on the GPU. It renders as the CPU does
//! (within one 8-bit level), and a drag tick that leaves the image and the mask's alpha alone neither estimates nor
//! uploads the edge again. Skips (passes with a note) when no GPU adapter exists.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_gpu::{Fallback, Fault};
use lightcraft_pipeline::{PixelWindow, Quality, RenderRequest, SourceInfo, StageCache, render};
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
    hair_of(w, h, [0.2, 0.1, 0.05], [0.1, 0.28, 0.7])
}

/// [`hair`] with the colours of the hair and the sky: the same matte over another photo.
fn hair_of(w: usize, h: usize, hair: [f32; 3], sky: [f32; 3]) -> (Arc<Rgb32f>, SegMask) {
    let aspect = w as f64 / h as f64;
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

/// Another photo of the same size and settings, rendered in the same view after the first: its edge is its own (the
/// kept edge's key holds no source pixels, so the kept edges are the photo's).
#[test]
fn another_photo_in_the_view_gets_its_own_edge() {
    if !gpu() {
        return;
    }
    let (w, h) = (320, 214);
    let (a, seg) = hair(w, h);
    let (b, _) = hair_of(w, h, [0.55, 0.45, 0.3], [0.8, 0.85, 0.9]);
    let info = SourceInfo::default();
    let req = RenderRequest::fit(w, h);
    let cache = StageCache::default();
    let s = background(&seg, orange(), 100.0);
    let uploads = lightcraft_gpu::edge_uploads();
    let _ = on_gpu(&a, &info, &s, &req, Some(&cache));
    let in_view = on_gpu(&b, &info, &s, &req, Some(&cache));
    assert_eq!(lightcraft_gpu::edge_uploads() - uploads, 2, "the second photo estimates and uploads its own edge");
    let fresh = on_gpu(&b, &info, &s, &req, None);
    assert_eq!(in_view.data, fresh.data, "the second photo is drawn with the first one's edge");
}

/// A draft render and a full one of the same size are not the same edge, and the draft matches the CPU's draft.
#[test]
fn a_draft_and_a_full_render_keep_their_own_edges() {
    if !gpu() {
        return;
    }
    let (w, h) = (320, 214);
    // (its own photo: a draft takes the Recolor references measured for the same photo and masks by any render, and
    // the other tests render the standard one at the same time)
    let (src, seg) = hair_of(w, h, [0.21, 0.12, 0.06], [0.11, 0.3, 0.66]);
    let info = SourceInfo::default();
    let full = RenderRequest::fit(w, h);
    let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(w, h) };
    let cache = StageCache::default();
    let s = background(&seg, orange(), 100.0);
    let estimates = lightcraft_pipeline::decontam::estimates;
    let at = estimates();
    let _ = on_gpu(&src, &info, &s, &full, Some(&cache));
    let drafted = on_gpu(&src, &info, &s, &draft, Some(&cache));
    assert_eq!(estimates() - at, 2, "a draft is not drawn with the full render's edge");
    let fresh = on_gpu(&src, &info, &s, &draft, None);
    let (apart, n) = worst(&fresh, &drafted);
    assert!(n == 0, "a draft after a full render is {n} pixels ({apart} levels) from a fresh draft");
    let cpu = render(&src, &info, &s, &draft).image;
    let (max, n) = worst(&cpu, &drafted);
    eprintln!("draft: max {max}, {n} pixels differ");
    assert!(max <= 1, "the GPU's draft is {max} levels from the CPU's");
    // (a Process change reads the same way: another edge)
    let mut older = s.clone();
    older.process = lightcraft_develop::Process::V3.version();
    let at = estimates();
    let _ = on_gpu(&src, &info, &s, &draft, Some(&cache));
    let _ = on_gpu(&src, &info, &older, &draft, Some(&cache));
    assert_eq!(estimates() - at, 1, "another process is another edge");
}

/// Two decontaminated masks, each with its own edge (the second one inverted, recoloured blue): as on the CPU, and a
/// tick of either's Amount keeps both edges.
#[test]
fn two_decontaminated_masks_each_keep_an_edge() {
    if !gpu() {
        return;
    }
    let (w, h) = (320, 214);
    let (src, seg) = hair(w, h);
    let info = SourceInfo::default();
    let req = RenderRequest::fit(w, h);
    let mut s = background(&seg, orange(), 100.0);
    let mut second = s.masks[0].clone();
    second.id = 2;
    second.invert = true;
    second.adjust = LocalAdjustments { exposure: -0.3, recolor: 100.0, recolor_r: 0.2, recolor_g: 0.4, recolor_b: 1.0, ..Default::default() };
    second.decontaminate = 80.0;
    s.masks.push(second);
    let cpu = render(&src, &info, &s, &req).image;
    let cache = StageCache::default();
    let (estimates, uploads) = (lightcraft_pipeline::decontam::estimates(), lightcraft_gpu::edge_uploads());
    let first = on_gpu(&src, &info, &s, &req, Some(&cache));
    let (max, n) = worst(&cpu, &first);
    eprintln!("two masks: max {max}, {n} pixels differ");
    assert!(max <= 1, "two decontaminated masks: the GPU is {max} levels from the CPU");
    assert_eq!(lightcraft_gpu::edge_uploads() - uploads, 2, "each mask uploads its own edge");
    s.masks[0].adjust.amount = 50.0;
    s.masks[1].adjust.exposure = 0.2;
    let tick = on_gpu(&src, &info, &s, &req, Some(&cache));
    assert_eq!(lightcraft_gpu::edge_uploads() - uploads, 2, "a tick uploaded an edge again");
    assert_eq!(lightcraft_pipeline::decontam::estimates() - estimates, 2, "a tick estimated an edge again");
    assert_eq!(tick.data, on_gpu(&src, &info, &s, &req, None).data, "kept edges render as fresh ones");
}

/// A matte soft everywhere: every pixel is an edge pixel.
fn foggy(w: usize, h: usize) -> (Arc<Rgb32f>, SegMask) {
    let (src, _) = hair(w, h);
    let side = 128;
    let logits: Vec<f32> = (0..side * side).map(|i| 0.6 * ((i % side) as f32 * 0.21).sin() * ((i / side) as f32 * 0.17).cos()).collect();
    (src, SegMask::from_logits(side, &logits))
}

/// An edge over the device's buffer limit fails that render on the GPU (the CPU draws it), and the next render, with
/// room, is on the GPU again.
#[test]
fn an_edge_over_the_buffer_limit_falls_back_to_the_cpu() {
    if !gpu() {
        return;
    }
    let (w, h) = (160, 107);
    let (src, seg) = foggy(w, h);
    let info = SourceInfo::default();
    let req = RenderRequest::fit(w, h);
    let s = background(&seg, orange(), 100.0);
    // (the output and the mask's plane fit; an edge pixel is 7 words against the plane's 1)
    lightcraft_gpu::inject_fault(Fault::Limit((w * h * 3 * 4 + 1024) as u64));
    match lightcraft_gpu::try_render(&src, &info, &s, &req, None) {
        Err(Fallback::Failed(why)) => assert!(why.contains("decontaminated"), "{why}"),
        Err(Fallback::NotTried) => panic!("the render must be tried on the GPU"),
        Ok(_) => panic!("an edge over the limit must fall back"),
    }
    assert!(lightcraft_gpu::available(), "a limit is no device failure");
    // (the CPU renders it, and the GPU does again once there is room)
    let cpu = render(&src, &info, &s, &req).image;
    let (max, _) = worst(&cpu, &on_gpu(&src, &info, &s, &req, None));
    assert!(max <= 1, "the GPU render after the limit is {max} levels from the CPU's");
}

/// A zoomed window drawn on the GPU is the same pixels as that part of the GPU's whole render: the edge is estimated
/// over the window and the halo around it, not from the window's own origin.
#[test]
fn a_zoomed_window_is_a_crop_of_the_whole_render() {
    if !gpu() {
        return;
    }
    let (w, h) = (480, 320);
    let (src, seg) = hair(w, h);
    let info = SourceInfo::default();
    let s = background(&seg, orange(), 100.0);
    let big = RenderRequest::fit(960, 640);
    let win = PixelWindow { x: 300, y: 120, w: 320, h: 200 };
    let whole = on_gpu(&src, &info, &s, &big, None);
    let part = on_gpu(&src, &info, &s, &RenderRequest { window: Some(win), ..big }, None);
    assert_eq!((part.width, part.height), (win.w, win.h));
    let crop = whole.crop(win.x, win.y, win.w, win.h);
    let (max, n) = worst(&crop, &part);
    eprintln!("window vs crop: max {max}, {n} pixels differ");
    assert!(max <= 1, "the window is {max} levels from the whole render's crop");
}

/// A Background matte that is the sky right of the vertical line at normalised `x` (soft over a few pixels).
fn band_seg(x: f64) -> SegMask {
    let side = 256;
    let logits: Vec<f32> = (0..side * side).map(|i| (((i % side) as f64 + 0.5) / side as f64 - x) as f32 * 400.0).collect();
    SegMask::from_logits(side, &logits)
}

/// A Portrait slider tick changes the image where the retouch step writes. An edge that estimate reads nothing there
/// keeps its estimate (no estimate, no upload) and the tick renders as a whole render does; one beside the face is
/// estimated again.
#[test]
fn a_portrait_tick_keeps_an_edge_the_retouching_does_not_reach() {
    use super::portrait_drag::{edit, portrait, whole};
    use lightcraft_develop::controls;
    use lightcraft_develop::retouch::NormRect;
    if !gpu() {
        return;
    }
    let (w, h) = (1920, 1280);
    let req = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(w, h) };
    // a small face at the left, the matte's edge at 80 % of the width (far from it) or just beside it (at 24 %)
    let (src, info) = portrait(w, h, NormRect::new(0.08, 0.12, 0.2, 0.32));
    for (edge_at, kept) in [(0.8, true), (0.24, false)] {
        let cache = StageCache::default();
        let mut s = edit();
        s.masks = background(&band_seg(edge_at), LocalAdjustments { exposure: 0.4, ..orange() }, 100.0).masks;
        controls::set(&mut s, "retouch.skin.smoothness", 40.0);
        let (estimates, uploads) = (lightcraft_pipeline::decontam::estimates(), lightcraft_gpu::edge_uploads());
        let _ = lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
        assert_eq!(lightcraft_pipeline::decontam::estimates() - estimates, 1, "the first render estimates the edge");
        controls::set(&mut s, "retouch.skin.smoothness", 55.0);
        let tick = lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu").image;
        let again = lightcraft_pipeline::decontam::estimates() - estimates - 1;
        let sent = lightcraft_gpu::edge_uploads() - uploads - 1;
        assert_eq!((again, sent), if kept { (0, 0) } else { (1, 1) }, "matte edge at {edge_at}: estimated again, uploaded again");
        let fresh = whole(&src, &info, &s, &req);
        assert_eq!(tick.data, fresh.data, "matte edge at {edge_at}: a tick renders as a whole render");
    }
}
