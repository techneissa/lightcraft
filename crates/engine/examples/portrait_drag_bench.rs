//! A Portrait slider drag on a synthetic face (fork, `fork/perf-portrait-draft.md`): the draft frame time of each
//! Portrait feature's slider, warm stage cache, on the CPU and (when an adapter exists) the GPU.
//! `cargo run --release -p lightcraft-engine --example portrait_drag_bench` (`N=9` ticks per row, `ONLY=skin,eyes`,
//! `LIGHTCRAFT_GPU=0` for the CPU only, `GPU_ONLY=1` for the GPU only, `LIGHTCRAFT_PROFILE=1` for the stage table of
//! every tick, `DC=100` to add a Background mask recoloured and decontaminated (to that
//! amount; `DC=0` for the same mask without) around the face (`DC_EDGE=0.85`: its edge a vertical line at that
//! share of the width instead, away from the face), as a mask beside the Portrait sliders: `fork/perf-decontam-gpu.md`). Two framings: a face filling the frame and a half-length portrait (a face about a fifth of its height).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::retouch::{Face, Input, NormRect, Part, PartMask};
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask, controls};
use lightcraft_engine::media::develop;
use lightcraft_pipeline::{Quality, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::{Plane, Rgb32f};

/// A face in `bounds` (normalised) on a `w × h` photo, with skin texture, a few blemishes, eyes, lips and teeth, and
/// the part masks a people parse gives (256 px a side over the face's box), its spots found as the engine does.
fn face_photo(w: usize, h: usize, bounds: NormRect) -> (Rgb32f, SourceInfo) {
    let at = |fx: f64, fy: f64| (bounds.x0 + fx * bounds.width(), bounds.y0 + fy * bounds.height());
    let ell = |nx: f64, ny: f64, c: (f64, f64), rx: f64, ry: f64| {
        let d = (((nx - c.0) / (rx * bounds.width())).powi(2) + ((ny - c.1) / (ry * bounds.height())).powi(2)).sqrt();
        ((1.1 - d) / 0.2).clamp(0.0, 1.0) as f32
    };
    let eyes = [at(0.33, 0.4), at(0.67, 0.4)];
    let (mouth, face_c) = (at(0.5, 0.75), at(0.5, 0.5));
    let skin_m = move |x: f64, y: f64| ell(x, y, face_c, 0.48, 0.5);
    let whites = move |x: f64, y: f64| eyes.iter().map(|c| ell(x, y, *c, 0.09, 0.04)).fold(0.0, f32::max);
    let iris = move |x: f64, y: f64| eyes.iter().map(|c| ell(x, y, *c, 0.035, 0.035)).fold(0.0, f32::max);
    let lips = move |x: f64, y: f64| ell(x, y, mouth, 0.16, 0.06);
    let teeth = move |x: f64, y: f64| ell(x, y, mouth, 0.1, 0.025);
    let spots: Vec<(f64, f64)> = (0..12).map(|i| at(0.25 + 0.05 * (i % 6) as f64, 0.55 + 0.08 * (i / 6) as f64)).collect();
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        let tex = 1.0 + 0.05 * (((x * 7 + y * 13) % 5) as f32 - 2.0) / 2.0 + 0.08 * ((x as f32 / 3.1).sin() * (y as f32 / 2.7).sin());
        let blem = spots.iter().map(|c| ell(nx, ny, *c, 0.012, 0.009)).fold(0.0, f32::max);
        let skin = [0.5 * tex * (1.0 - 0.3 * blem), 0.36 * tex * (1.0 - 0.45 * blem), 0.28 * tex * (1.0 - 0.4 * blem)];
        let back = [0.12 + 0.05 * (nx as f32), 0.14, 0.16 + 0.04 * (ny as f32)];
        let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t) };
        let mut c = mix(back, skin, skin_m(nx, ny));
        c = mix(c, [0.62, 0.5, 0.42], whites(nx, ny));
        c = mix(c, [0.1, 0.07, 0.05], iris(nx, ny));
        c = mix(c, [0.42, 0.12, 0.13], lips(nx, ny));
        mix(c, [0.6, 0.55, 0.36], teeth(nx, ny))
    });
    let mask = |f: &dyn Fn(f64, f64) -> f32| {
        let n = 256;
        let plane = Plane::from_fn(n, n, |x, y| {
            f(bounds.x0 + (x as f64 + 0.5) / n as f64 * bounds.width(), bounds.y0 + (y as f64 + 0.5) / n as f64 * bounds.height())
        });
        PartMask { plane, area: bounds }
    };
    let face = Face {
        landmarks: vec![eyes[0], eyes[1], at(0.5, 0.58), at(0.4, 0.75), at(0.6, 0.75)],
        masks: vec![
            (Part::FacialSkin, mask(&|x, y| skin_m(x, y) * (1.0 - whites(x, y).max(lips(x, y))))),
            (Part::EyeWhites, mask(&|x, y| whites(x, y) * (1.0 - iris(x, y)))),
            (Part::Iris, mask(&iris)),
            (Part::Lips, mask(&|x, y| lips(x, y) * (1.0 - teeth(x, y)))),
            (Part::Teeth, mask(&teeth)),
        ],
        ..Face::boxed(bounds)
    };
    let faces = lightcraft_develop::retouch::blemish::with_spots(&src, vec![face]);
    (src, SourceInfo { retouch: Some(Arc::new(Input::new(faces))), ..SourceInfo::default() })
}

/// A typical edit of a raw: its default sharpening and colour noise reduction, with tone and clarity.
fn typical() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.detail.sharpen_amount = 40.0;
    s.detail.nr_color = 25.0;
    s.light.exposure = 0.3;
    s.light.highlights = -30.0;
    s.light.shadows = 25.0;
    s.effects.clarity = 10.0;
    s
}

/// `DC=<amount>`: a Background mask, its matte soft along an ellipse around `bounds`, recoloured, Decontaminate `amount`.
fn decontaminated_background(bounds: NormRect, amount: f64) -> Mask {
    let side = 512;
    let (cx, cy) = ((bounds.x0 + bounds.x1) / 2.0, (bounds.y0 + bounds.y1) / 2.0);
    let (rx, ry) = (bounds.width() * 0.6, bounds.height() * 0.6);
    let line: Option<f64> = std::env::var("DC_EDGE").ok().and_then(|v| v.parse().ok());
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (nx, ny) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            if let Some(x) = line {
                return ((nx - x) * 400.0).clamp(-20.0, 20.0) as f32;
            }
            let d = (((nx - cx) / rx).powi(2) + ((ny - cy) / ry).powi(2)).sqrt();
            (((d - 1.0) * 60.0).clamp(-20.0, 20.0)) as f32
        })
        .collect();
    let adjust = LocalAdjustments { exposure: 0.3, recolor: 100.0, recolor_r: 1.0, recolor_g: 0.55, recolor_b: 0.1, ..Default::default() };
    Mask {
        id: 1,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Background { seg: Some(SegMask::from_logits(side, &logits)), detail: Vec::new(), edge: 0.0 },
        }],
        adjust,
        decontaminate: amount,
        ..Default::default()
    }
}

fn main() {
    let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let only = std::env::var("ONLY").unwrap_or_default();
    let gpu = lightcraft_engine::gpu::available();
    println!("gpu: {}", lightcraft_engine::gpu::adapter_name().filter(|_| gpu).unwrap_or_else(|| "none".into()));
    let backends: Vec<bool> = match (gpu, std::env::var_os("GPU_ONLY").is_some()) {
        (true, true) => vec![true],
        (true, false) => vec![false, true],
        _ => vec![false],
    };
    // (the loupe's 2560 px preview; a draft of about 2.5 MP)
    let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1920, 1280) };
    let features: [(&str, &str, f64); 6] = [
        ("none (exposure)", "light.exposure", 0.0),
        ("skin", "retouch.skin.smoothness", 40.0),
        ("eyes", "retouch.eyes.whites", 40.0),
        ("skin tone", "retouch.skinTone.evenTone", 40.0),
        ("teeth and lips", "retouch.teethLips.teethWhitening", 40.0),
        ("blemishes", "retouch.blemish.amount", 40.0),
    ];
    for (framing, bounds) in [("face-filling", NormRect::new(0.2, 0.02, 0.8, 0.98)), ("portrait", NormRect::new(0.42, 0.12, 0.58, 0.36))] {
        let (src, info) = face_photo(2560, 1707, bounds);
        let src = Arc::new(src);
        println!("\n{framing}: source {}×{}, draft {}×{}", src.width, src.height, draft.max_w, draft.max_h);
        for (name, id, base) in features {
            if !only.is_empty() && !only.split(',').any(|o| name.contains(o)) {
                continue;
            }
            let mut cols = Vec::new();
            for &g in &backends {
                let cache = StageCache::default();
                let mut s = typical();
                if let Some(amount) = std::env::var("DC").ok().and_then(|v| v.parse().ok()) {
                    s.masks.push(decontaminated_background(bounds, amount));
                }
                if id.starts_with("retouch.") {
                    controls::set(&mut s, id, base);
                }
                // warm: the first render with the feature on, then ticks
                drop(develop(&src, &info, &s, &draft, Some(&cache), g));
                drop(develop(&src, &info, &s, &draft, Some(&cache), g));
                let mut times = Vec::new();
                for k in 0..n {
                    let mut t = s.clone();
                    let v = if id == "light.exposure" { 0.3 + 0.02 * (k + 1) as f64 } else { base + (k + 1) as f64 };
                    controls::set(&mut t, id, v);
                    if lightcraft_pipeline::profiling() {
                        eprintln!("--- {framing} {name} {} tick {k}", if g { "gpu" } else { "cpu" });
                    }
                    let at = Instant::now();
                    drop(develop(&src, &info, &t, &draft, Some(&cache), g));
                    times.push(at.elapsed().as_secs_f64() * 1e3);
                }
                times.sort_by(f64::total_cmp);
                cols.push(format!("{} median {:.1} ms (min {:.1})", if g { "gpu" } else { "cpu" }, times[times.len() / 2], times[0]));
            }
            println!("  {name:<18} {}", cols.join("  |  "));
        }
    }
}
