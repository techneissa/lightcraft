//! A mask slider drag with Decontaminate on and off (fork, `fork/perf-decontam-gpu.md`): the draft frame time of a
//! Background mask's Exposure drag, warm stage cache, on the CPU and (when an adapter exists) the GPU.
//!
//! `cargo run --release -p lightcraft-engine --example decontam_drag_bench` (a synthetic portrait: a head with
//! ragged hair against a blue sky, the Background mask a matte soft along the strands, recoloured orange).
//! `--features sam -- PHOTO` takes a photo instead, its Background mask from the app's Subject model (models from
//! the shared models folder). `N=9` ticks per row, `LIGHTCRAFT_GPU=0` for the CPU only, `GPU_ONLY=1` for the GPU
//! only, `LIGHTCRAFT_PROFILE=1` for the stage table of every tick, `SAVE=DIR` to write each backend's draft as
//! `DIR/<cpu|gpu>-d<0|100>.png`.
//!
//! A manual tool: not part of the app or of any check; it stops with a message on bad arguments.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, SegMask};
use lightcraft_engine::media::develop;
use lightcraft_pipeline::{Quality, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

/// The sky's share of a pixel at normalised `(nx, ny)` of a `w × h` frame: 0 inside the head, 1 far from it, a
/// band of strands of every density along its top half.
fn sky_at(nx: f64, ny: f64, aspect: f64) -> f64 {
    let (dx, dy) = ((nx - 0.5) * aspect, ny - 0.62);
    let r = (dx * dx + dy * dy).sqrt();
    let ang = dy.atan2(dx);
    // strands: thin and thick, longer at the top
    let top = (-dy / r.max(1e-6)).max(0.0);
    let s = 0.5 + 0.5 * (ang * 180.0).sin() * (ang * 37.0 + 1.3).cos();
    let rim = 0.3 + 0.06 * top * s;
    ((r - 0.3) / (rim - 0.3 + 0.004)).clamp(0.0, 1.0)
}

fn portrait(w: usize, h: usize) -> (Rgb32f, SegMask) {
    let aspect = w as f64 / h as f64;
    let hair = [0.18f32, 0.09, 0.04];
    let sky = [0.12f32, 0.3, 0.75];
    let src = Rgb32f::from_fn(w, h, |x, y| {
        let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
        let a = sky_at(nx, ny, aspect) as f32;
        let tex = 0.85 + 0.15 * ((x * 7 + y * 3) % 5) as f32 / 4.0;
        let sky = [sky[0] * (1.0 - 0.3 * ny as f32), sky[1], sky[2] * (0.8 + 0.2 * nx as f32)];
        std::array::from_fn(|c| (1.0 - a) * hair[c] * tex + a * sky[c])
    });
    // the matte: logits over the frame (the sigmoid of the logit is the sky's share)
    let side = 1024;
    let logits: Vec<f32> = (0..side * side)
        .map(|i| {
            let (nx, ny) = (((i % side) as f64 + 0.5) / side as f64, ((i / side) as f64 + 0.5) / side as f64);
            let a = sky_at(nx, ny, aspect).clamp(0.003, 0.997);
            (a / (1.0 - a)).ln() as f32
        })
        .collect();
    (src, SegMask::from_logits(side, &logits))
}

/// A typical edit with a Background mask recoloured orange and its Exposure lifted, Decontaminate at `d`.
fn edit(seg: Option<SegMask>, d: f64) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.detail.sharpen_amount = 40.0;
    s.detail.nr_color = 25.0;
    s.light.highlights = -30.0;
    s.light.shadows = 25.0;
    s.effects.clarity = 10.0;
    let adjust = LocalAdjustments { exposure: 0.3, recolor: 100.0, recolor_r: 1.0, recolor_g: 0.55, recolor_b: 0.1, ..Default::default() };
    s.masks.push(Mask {
        id: 1,
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Background { seg, detail: Vec::new(), edge: 0.0 },
        }],
        adjust,
        decontaminate: d,
        ..Default::default()
    });
    s
}

#[cfg(feature = "sam")]
fn photo(path: &str) -> (Arc<Rgb32f>, SourceInfo, DevelopSettings) {
    use lightcraft_engine::Session;
    use lightcraft_engine::catalog::PhotoId;
    use serde_json::json;
    let mut s = Session::new().with_fs().with_default_sam3_model().with_default_parsing_models();
    s.segmenter.background = false;
    let full = std::fs::canonicalize(path).expect("photo").to_string_lossy().to_string();
    let r = s.execute("library.import", &json!({"paths": [full]})).expect("import");
    let id = PhotoId(r["imported"][0].as_u64().expect("imported"));
    s.execute("library.select", &json!({"ids": [id.0]})).expect("select");
    s.execute("mask.add", &json!({"kind": "background"})).expect("background mask");
    let src = s.source_now(id, lightcraft_engine::SourceLevel::Preview).expect("decode");
    let info = s.source_info(id);
    let mut settings = (*s.develop_of(id).expect("settings")).clone();
    let seg = match settings.masks.first().and_then(|m| m.components.first()).map(|c| &c.shape) {
        Some(MaskShape::Background { seg, .. }) => seg.clone(),
        _ => None,
    };
    eprintln!("background matte from the model: {}", seg.is_some());
    let base = edit(seg, 0.0);
    settings.masks = base.masks;
    (src, info, settings)
}

#[cfg(not(feature = "sam"))]
fn photo(_: &str) -> (Arc<Rgb32f>, SourceInfo, DevelopSettings) {
    eprintln!("a photo needs `--features sam` (its Background mask comes from the Subject model)");
    std::process::exit(2);
}

/// `HASHES=1`: a hash of the GPU render of each decontamination-off case (masks of every kind of edit), to show they
/// render byte for byte alike before and after a change to the GPU kernel.
fn hashes(src: &Arc<Rgb32f>, info: &SourceInfo, base: &DevelopSettings) {
    use std::hash::{Hash, Hasher};
    let mut cases: Vec<(&str, DevelopSettings)> = vec![("no masks", DevelopSettings { masks: Vec::new(), ..base.clone() }), ("mask", base.clone())];
    let mut busy = base.clone();
    if let Some(m) = busy.masks.first_mut() {
        let a = &mut m.adjust;
        (a.temp, a.tint, a.saturation, a.hue, a.dehaze, a.moire, a.defringe, a.noise) = (20.0, -15.0, 30.0, 10.0, 25.0, 40.0, 50.0, 20.0);
        (a.clarity, a.texture, a.contrast, a.highlights, a.shadows) = (20.0, 15.0, 10.0, -20.0, 30.0);
        (a.color_sat, a.color_hue) = (50.0, 200.0);
    }
    cases.push(("busy mask", busy.clone()));
    let mut two = busy.clone();
    if let Some(m) = base.masks.first() {
        let mut m2 = m.clone();
        m2.id = 2;
        m2.adjust.exposure = -0.4;
        m2.adjust.recolor = 0.0;
        two.masks.push(m2);
    }
    cases.push(("two masks", two));
    for (name, s) in cases {
        for req in [RenderRequest::fit(1920, 1280), RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1920, 1280) }] {
            let r = develop(src, info, &s, &req, None, true);
            let mut h = std::hash::DefaultHasher::new();
            r.image.data.hash(&mut h);
            println!("{name:>10} {:?}: {:016x}", req.quality, h.finish());
        }
    }
}

fn main() {
    let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(9);
    let gpu = lightcraft_engine::gpu::available();
    println!("gpu: {}", lightcraft_engine::gpu::adapter_name().filter(|_| gpu).unwrap_or_else(|| "none".into()));
    let backends: Vec<bool> = match (gpu, std::env::var_os("GPU_ONLY").is_some()) {
        (true, true) => vec![true],
        (true, false) => vec![false, true],
        _ => vec![false],
    };
    let (src, info, base) = match std::env::args().nth(1) {
        Some(p) => photo(&p),
        None => {
            let (src, seg) = portrait(2560, 1707);
            (Arc::new(src), SourceInfo::default(), edit(Some(seg), 0.0))
        }
    };
    if std::env::var_os("HASHES").is_some() {
        hashes(&src, &info, &base);
        return;
    }
    // (the loupe's draft: about 2.5 MP)
    let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1920, 1280) };
    println!("source {}×{}, draft {}×{}", src.width, src.height, draft.max_w, draft.max_h);
    for d in [0.0, 100.0] {
        let mut cols = Vec::new();
        for &g in &backends {
            let cache = StageCache::default();
            let mut s = base.clone();
            if let Some(m) = s.masks.first_mut() {
                m.decontaminate = d;
            }
            drop(develop(&src, &info, &s, &draft, Some(&cache), g));
            drop(develop(&src, &info, &s, &draft, Some(&cache), g));
            let mut times = Vec::new();
            for k in 0..n {
                let mut t = s.clone();
                if let Some(m) = t.masks.first_mut() {
                    m.adjust.exposure = 0.3 + 0.02 * (k + 1) as f64;
                }
                if lightcraft_pipeline::profiling() {
                    eprintln!("--- decontaminate {d} {} tick {k}", if g { "gpu" } else { "cpu" });
                }
                let at = Instant::now();
                drop(develop(&src, &info, &t, &draft, Some(&cache), g));
                times.push(at.elapsed().as_secs_f64() * 1e3);
            }
            if let Some(dir) = std::env::var_os("SAVE") {
                let r = develop(&src, &info, &s, &draft, None, g);
                let o = lightcraft_engine::export::ExportOptions { format: lightcraft_engine::export::ExportFormat::Png, ..Default::default() };
                let png = lightcraft_engine::export::encode_image(&r.image, &o).expect("png");
                let name = format!("{}-d{d}.png", if g { "gpu" } else { "cpu" });
                std::fs::write(std::path::Path::new(&dir).join(name), png).expect("write");
            }
            times.sort_by(f64::total_cmp);
            cols.push(format!("{} median {:.1} ms (min {:.1})", if g { "gpu" } else { "cpu" }, times[times.len() / 2], times[0]));
        }
        println!("  decontaminate {d:>3}: {}", cols.join("  |  "));
    }
}
