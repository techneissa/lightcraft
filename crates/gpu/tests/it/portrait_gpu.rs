//! Fork (`fork/perf-portrait-gpu.md`, issue #241): Portrait on the GPU. The image before the retouch step is made on the
//! device and the step retouches the band of rows that holds every face's reads; for every slider of every face
//! feature the GPU render matches the CPU's within the equivalence bounds, a zoomed window equals the whole frame, and
//! a block with every slider at 0 renders the bytes of a photo without one.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{FEATURES, Face, Input, NormRect, Scope, Stage};
use lightcraft_pipeline::{PixelWindow, RenderRequest, SourceInfo, render};
use lightcraft_raster::{Rgb32f, Rgba8};

use super::portrait_drag::{edit, portrait};

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        let why = lightcraft_gpu::unavailable_reason().unwrap_or_default();
        assert!(!why.contains("kernels failed to build"), "{why}");
        lightcraft_testkit::skip!(Gpu, "no GPU adapter");
    }
    ok
}

/// Every slider of every face feature of the retouch stage, at its top.
fn sliders() -> Vec<(&'static str, f64)> {
    FEATURES
        .iter()
        .filter(|f| f.scope == Scope::Face && f.stage == Stage::Retouch)
        .flat_map(|f| (f.specs)())
        .filter(|s| s.max > 0.0)
        .map(|s| (s.id, s.max))
        .collect()
}

/// `info`'s faces with a face mesh after their five points (Wrinkles needs one).
fn with_mesh(info: &SourceInfo) -> SourceInfo {
    let faces = info.retouch.iter().flat_map(|i| i.faces.iter()).map(|f| {
        let r = f.bounds;
        let at = |(fx, fy): (f64, f64)| (r.x0 + fx * r.width(), r.y0 + fy * r.height());
        let mesh = (0..478).map(|i| at(lightcraft_develop::retouch::wrinkles::areas::frontal(i)));
        Face { landmarks: f.landmarks.iter().copied().chain(mesh).collect(), ..f.clone() }
    });
    SourceInfo { retouch: Some(Arc::new(Input::new(faces.collect()))), ..SourceInfo::default() }
}

fn gpu_render(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest) -> Rgba8 {
    lightcraft_gpu::try_render(src, info, s, req, None).expect("a GPU render without the CPU fallback").image
}

/// Mean and largest difference of `a` and `b` (8-bit, RGB) inside `win`.
fn diff_in(a: &Rgba8, b: &Rgba8, (ax, ay): (usize, usize), win: PixelWindow) -> (f64, u8) {
    let (mut sum, mut most, mut n) = (0u64, 0u8, 0u64);
    for y in 0..win.h {
        for x in 0..win.w {
            let (p, q) = (a.get(ax + x, ay + y), b.get(x, y));
            for k in 0..3 {
                let d = p[k].abs_diff(q[k]);
                sum += u64::from(d);
                most = most.max(d);
                n += 1;
            }
        }
    }
    (sum as f64 / n.max(1) as f64, most)
}

/// The GPU render of each slider of each face feature matches the CPU's within the equivalence bounds (mean under half
/// a level, at most 3 levels: `docs/gpu-pipeline.md`), on a face with five points and on one with a mesh.
#[test]
fn every_face_slider_matches_the_cpu() {
    if !gpu() {
        return;
    }
    let (w, h) = (480, 360);
    let (src, five) = portrait(w, h, NormRect::new(0.3, 0.12, 0.7, 0.88));
    let mesh = with_mesh(&five);
    let req = RenderRequest::fit(w, h);
    let all = PixelWindow { x: 0, y: 0, w, h };
    let mut failures = Vec::new();
    for (id, v) in sliders() {
        for (with, info) in [("five points", &five), ("a mesh", &mesh)] {
            let mut s = edit();
            s.retouch.global.set(id, v);
            let (cpu, gpu) = (render(&src, info, &s, &req).image, gpu_render(&src, info, &s, &req));
            let (mean, most) = diff_in(&cpu, &gpu, (0, 0), all);
            if mean >= 0.5 || most > 3 {
                failures.push(format!("{id} ({with}): mean {mean:.3}, most {most} levels"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Zoom never changes a retouch on the GPU either: for each slider of each face feature, a zoomed render of a window
/// over the face gives the whole frame's pixels there, to a level (as `a_zoomed_window_gives_the_whole_frames_retouch`
/// checks on the CPU).
#[test]
fn a_zoomed_window_gives_the_whole_frames_retouch_on_the_gpu() {
    if !gpu() {
        return;
    }
    let (w, h) = (480, 360);
    let (src, five) = portrait(w, h, NormRect::new(0.3, 0.12, 0.7, 0.88));
    let mesh = with_mesh(&five);
    let req = RenderRequest::fit(w, h);
    // (the left eye and the cheek under it; the mouth's left corner)
    let wins = [PixelWindow { x: 170, y: 110, w: 70, h: 90 }, PixelWindow { x: 200, y: 240, w: 60, h: 50 }];
    let mut failures = Vec::new();
    for (id, v) in sliders() {
        for (with, info) in [("five points", &five), ("a mesh", &mesh)] {
            let mut s = edit();
            s.retouch.global.set(id, v);
            let whole = gpu_render(&src, info, &s, &req);
            for win in wins {
                let part = gpu_render(&src, info, &s, &RenderRequest { window: Some(win), ..req });
                assert_eq!((part.width, part.height), (win.w, win.h));
                let (_, most) = diff_in(&whole, &part, (win.x, win.y), win);
                if most > 1 {
                    failures.push(format!("{id} ({with}) in {win:?}: the window differs from the whole frame by {most}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Off and 0 change nothing: a block with every Portrait slider at 0, and one with sliders on but the Portrait eye off,
/// render on the GPU the bytes of the same photo without a block.
#[test]
fn every_slider_at_0_renders_the_photo_without_portrait() {
    if !gpu() {
        return;
    }
    let (w, h) = (480, 360);
    let (src, info) = portrait(w, h, NormRect::new(0.3, 0.12, 0.7, 0.88));
    let req = RenderRequest::fit(w, h);
    let plain = gpu_render(&src, &info, &edit(), &req);
    let mut zero = edit();
    for (id, _) in sliders() {
        zero.retouch.global.set(id, 1.0);
        zero.retouch.global.set(id, 0.0);
    }
    assert!(gpu_render(&src, &info, &zero, &req).data == plain.data, "every slider at 0");
    let mut hidden = edit();
    for (id, v) in sliders() {
        hidden.retouch.global.set(id, v);
    }
    hidden.set_section_enabled("portrait", false);
    assert!(gpu_render(&src, &info, &hidden, &req).data == plain.data, "the Portrait eye off");
}
