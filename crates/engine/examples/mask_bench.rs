//! Brush mask cost, on the CPU pipeline and (when an adapter exists) on `lightcraft-gpu`:
//! `cargo run --release -p lightcraft-engine --example mask_bench`
//! (`N=3` runs per row, minimum wall-clock; `STROKES=1,30,300` stroke counts; `SIZE=2560` source
//! long edge; `ONLY=<word>` keeps the rows whose name has the word: `drag`, `new stroke`,
//! `overlay`, `hash`; `CONFIG=<word>` keeps the configurations whose name has the word: `one`,
//! `submasks`, `dense`, `pressure`, `auto`; `ALL=1` runs Auto Mask at every stroke count, which
//! is slow on the CPU).
//!
//! Every row is one render of a ~2.5 MP loupe (1920 × 1280) with a warm stage cache, as the loupe
//! does while editing:
//! - `drag`: a global Exposure tick over a photo whose mask holds N strokes (the whole mask is
//!   evaluated again unless the renderer keeps it).
//! - `new stroke`: the mask held N strokes at the last render; one more stroke has been added.
//!   A cost that grows with N is the full re-rasterization of the brush part.
//! - `overlay`: the mask overlay switched on and off again, one render per flip.
//! - `hash`: the settings hash64 and clone the loupe took every frame (UI thread, CPU only).
//! - `effect`: one effect-slider tick (a mask's Exposure, and the Amount slider) with `MASKS=6`
//!   overlapping masks, every mask on one small spot and then the same masks over the whole frame
//!   (radial gradients, and small brush blobs against whole-frame linear gradients). The cost of
//!   the small spot should be a fraction of the full-frame one.
//!
//! Configurations: `one brush part` (N strokes in one component), `submasks` (the same strokes
//! plus a second brush part and a gradient intersect), `dense` (a stroke of ~10,000 dabs, past
//! the GPU kernel's 4,096), `pressure` (pen pressure on every stroke) and `auto` (Auto Mask
//! strokes).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_engine::media::develop;
use lightcraft_geom::Point;
use lightcraft_pipeline::{MaskView, Overlay, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, fit};

/// A small deterministic generator (the strokes are the same on every run and machine).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as f64 / (1u64 << 31) as f64
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Normal,
    Dense,
    Pressure,
    Auto,
}

/// One stroke: a wandering path of `points` samples `step` apart (long-edge units), starting
/// anywhere on the photo. Normal strokes span about 40 % of the long edge.
fn stroke(rng: &mut Rng, kind: Kind, erase: bool) -> BrushStroke {
    let (points, step, size) = match kind {
        Kind::Dense => (240, 0.004, 0.004),
        _ => (12, 0.035, 0.03),
    };
    let mut p = Point::new(0.1 + 0.8 * rng.next(), 0.1 + 0.8 * rng.next());
    let mut heading = rng.next() * std::f64::consts::TAU;
    let mut path = Vec::with_capacity(points);
    for _ in 0..points {
        path.push(p);
        heading += (rng.next() - 0.5) * 0.9;
        let q = Point::new(p.x + step * heading.cos(), p.y + step * heading.sin());
        // (turn back from the border instead of leaving the photo)
        if q.x < 0.02 || q.x > 0.98 || q.y < 0.02 || q.y > 0.66 {
            heading += std::f64::consts::PI;
        } else {
            p = q;
        }
    }
    let pressure =
        if kind == Kind::Pressure { (0..points).map(|i| 0.25 + 0.75 * ((i as f32 * 0.6).sin() * 0.5 + 0.5)).collect() } else { Vec::new() };
    BrushStroke { points: path, size, erase, auto_mask: kind == Kind::Auto, pressure, ..BrushStroke::default() }
}

fn strokes(rng: &mut Rng, kind: Kind, n: usize) -> Vec<BrushStroke> {
    // every sixth stroke erases, as a painter's would
    (0..n).map(|i| stroke(rng, kind, i % 6 == 5)).collect()
}

fn component(op: MaskOp, shape: MaskShape) -> MaskComponent {
    MaskComponent { name: None, op, invert: false, shape }
}

/// The photo's settings with one mask of the configuration's shape holding `strokes`.
fn settings(strokes: Vec<BrushStroke>, submasks: bool) -> DevelopSettings {
    let mut parts = vec![component(MaskOp::Add, MaskShape::Brush { strokes })];
    if submasks {
        let mut rng = Rng(7);
        parts.push(component(MaskOp::Add, MaskShape::Brush { strokes: self::strokes(&mut rng, Kind::Normal, 6) }));
        parts.push(component(MaskOp::Intersect, MaskShape::Linear { start: Point::new(0.5, 0.05), end: Point::new(0.5, 0.6) }));
    }
    let mask = Mask { id: 1, components: parts, adjust: LocalAdjustments { exposure: 0.6, ..LocalAdjustments::default() }, ..Mask::default() };
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.2;
    s.masks = vec![mask];
    s
}

/// The brush strokes of settings made by [`settings`] (the first component).
fn push_stroke(s: &mut DevelopSettings, add: BrushStroke) {
    if let MaskShape::Brush { strokes } = &mut s.masks[0].components[0].shape {
        strokes.push(add);
    }
}

/// Minimum wall-clock (ms) over `n` runs.
fn best(n: usize, mut f: impl FnMut()) -> f64 {
    let mut b = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        f();
        b = b.min(t.elapsed().as_secs_f64() * 1e3);
    }
    b
}

fn main() {
    let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let long: usize = std::env::var("SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(2560);
    let counts: Vec<usize> =
        std::env::var("STROKES").ok().map(|v| v.split(',').filter_map(|c| c.trim().parse().ok()).collect()).unwrap_or_else(|| vec![1, 30, 300]);
    let only = std::env::var("ONLY").unwrap_or_default();
    let only_config = std::env::var("CONFIG").unwrap_or_default();
    let all = std::env::var_os("ALL").is_some();
    let run = |name: &str| only.is_empty() || only.split(',').any(|o| name.contains(o));
    let want_config = |name: &str| only_config.is_empty() || only_config.split(',').any(|o| name.contains(o));

    let (w, h) = (long, long * 2 / 3);
    let preview = Arc::new(fit(
        &Rgb32f::from_fn(w, h, |x, y| {
            let v = ((x as f32 * 0.011).sin() * (y as f32 * 0.007).cos() * 0.5 + 0.5) * 0.4 + ((x * 31 + y * 17) % 13) as f32 * 0.002;
            [v, v * 0.9, v * 0.7]
        }),
        long,
        long,
        Filter::Box,
    ));
    let info = SourceInfo { raw: true, ..Default::default() };
    let view = RenderRequest::fit(1920, 1280);
    let overlay = Overlay::AllMasks { view: MaskView::Color, color: [255, 40, 40], opacity: 50 };

    let gpu = lightcraft_engine::gpu::available();
    match lightcraft_engine::gpu::adapter_name().filter(|_| gpu) {
        Some(a) => println!("gpu: {a}"),
        None => println!("gpu: none"),
    }
    println!("source {}×{}, loupe 1920×1280, {n} runs each, minimum wall-clock", preview.width, preview.height);
    let backends: &[bool] = if gpu { &[false, true] } else { &[false] };
    let columns = |ms: &[f64]| {
        ms.iter().zip(backends).map(|(m, g)| format!("{}: {m:8.1} ms", if *g { "gpu" } else { "cpu" })).collect::<Vec<_>>().join("  |  ")
    };

    let configs: [(&str, Kind, bool, &[usize]); 5] = [
        ("one brush part", Kind::Normal, false, &counts),
        ("submasks", Kind::Normal, true, &counts),
        ("dense (past 4,096 dabs)", Kind::Dense, false, &[1, 3]),
        ("pen pressure", Kind::Pressure, false, &counts),
        ("auto mask", Kind::Auto, false, &counts),
    ];
    for (name, kind, submasks, list) in configs {
        if !want_config(name) {
            continue;
        }
        println!("\n== {name}");
        for &count in list {
            // (Auto Mask runs a full-frame guided filter per stroke: the largest count is opt-in)
            if kind == Kind::Auto && count > 30 && !all {
                continue;
            }
            let mut rng = Rng(42);
            let base = settings(strokes(&mut rng, kind, count), submasks);
            let extra: Vec<BrushStroke> = (0..8).map(|_| stroke(&mut rng, kind, false)).collect();
            if run("drag") {
                let caches = [StageCache::default(), StageCache::default()];
                let ms: Vec<f64> = backends
                    .iter()
                    .map(|&g| {
                        drop(develop(&preview, &info, &base, &view, Some(&caches[g as usize]), g));
                        let mut k = 0.0;
                        best(n, || {
                            let mut t = base.clone();
                            k += 1.0;
                            t.light.exposure = 0.2 + k * 0.01;
                            drop(develop(&preview, &info, &t, &view, Some(&caches[g as usize]), g));
                        })
                    })
                    .collect();
                println!("{count:>4} strokes  drag        {}", columns(&ms));
            }
            if run("new stroke") {
                let caches = [StageCache::default(), StageCache::default()];
                let ms: Vec<f64> = backends
                    .iter()
                    .map(|&g| {
                        drop(develop(&preview, &info, &base, &view, Some(&caches[g as usize]), g));
                        let mut k = 0;
                        best(n, || {
                            // the mask as it was, plus one new stroke (different each run)
                            let mut t = base.clone();
                            push_stroke(&mut t, extra[k % extra.len()].clone());
                            k += 1;
                            drop(develop(&preview, &info, &t, &view, Some(&caches[g as usize]), g));
                            // (settle back on the old mask so the next run adds to the same N)
                            drop(develop(&preview, &info, &base, &view, Some(&caches[g as usize]), g));
                        })
                    })
                    .collect();
                // (each run is a render with the stroke and one without: both are timed)
                println!("{count:>4} strokes  new stroke+undo {}", columns(&ms));
            }
            if run("overlay") {
                let caches = [StageCache::default(), StageCache::default()];
                let ms: Vec<f64> = backends
                    .iter()
                    .map(|&g| {
                        let on = RenderRequest { overlay, ..view };
                        drop(develop(&preview, &info, &base, &view, Some(&caches[g as usize]), g));
                        // one flip = one render, each way
                        let mut shown = false;
                        best(2 * n, || {
                            shown = !shown;
                            let req = if shown { &on } else { &view };
                            drop(develop(&preview, &info, &base, req, Some(&caches[g as usize]), g));
                        })
                    })
                    .collect();
                println!("{count:>4} strokes  overlay flip {}", columns(&ms));
            }
            if run("hash") {
                // the per-frame cost on the UI thread: clone the settings, hash them (CPU only)
                let hash = best(n * 5, || {
                    std::hint::black_box(base.hash64());
                });
                let clone = best(n * 5, || drop(std::hint::black_box(base.clone())));
                println!("{count:>4} strokes  per-frame hash64 {hash:8.3} ms, clone {clone:8.3} ms");
            }
        }
    }
    if run("effect") && want_config("effect") {
        let masks: usize = std::env::var("MASKS").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
        println!("\n== effect-slider ticks, {masks} overlapping masks");
        for (name, small, brush) in [
            ("radial, one small spot", true, false),
            ("radial, whole frame", false, false),
            ("brush blobs, small spot", true, true),
            ("gradients, whole frame", false, true),
        ] {
            let base = overlapping_masks(masks, small, brush);
            for (label, control) in [("mask exposure tick", 0), ("mask amount tick", 1), ("global exposure tick", 2)] {
                let caches = [StageCache::default(), StageCache::default()];
                let ms: Vec<f64> = backends
                    .iter()
                    .map(|&g| {
                        drop(develop(&preview, &info, &base, &view, Some(&caches[g as usize]), g));
                        let mut k = 0.0;
                        best(n, || {
                            let mut t = base.clone();
                            k += 1.0;
                            match control {
                                0 => t.masks[0].adjust.exposure = 0.6 + 0.02 * k,
                                1 => t.masks[0].adjust.amount = 100.0 - k % 50.0,
                                _ => t.light.exposure = 0.2 + 0.01 * k,
                            }
                            drop(develop(&preview, &info, &t, &view, Some(&caches[g as usize]), g));
                        })
                    })
                    .collect();
                println!("{name:<26} {label:<22} {}", columns(&ms));
            }
        }
    }
}

/// `count` masks that overlap: all on one small spot (about 8 % of the long edge across) or
/// each covering the whole frame. `brush`: small brush blobs, or whole-frame linear gradients;
/// else radial gradients. Each mask has its own effect; the last has a colour overlay.
fn overlapping_masks(count: usize, small: bool, brush: bool) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.2;
    for i in 0..count {
        let shift = 0.004 * i as f64;
        let shape = match (small, brush) {
            (true, false) => MaskShape::Radial {
                center: Point::new(0.5 + shift, 0.33),
                rx: 0.04 + shift,
                ry: 0.03 + shift,
                angle: 0.0,
                feather: 50.0,
                invert: false,
            },
            (false, false) => MaskShape::Radial { center: Point::new(0.5 + shift, 0.33), rx: 2.0, ry: 2.0, angle: 0.0, feather: 50.0, invert: false },
            (true, true) => {
                let blob = BrushStroke {
                    points: vec![Point::new(0.49 + shift, 0.33), Point::new(0.52 + shift, 0.34)],
                    size: 0.02,
                    ..BrushStroke::default()
                };
                MaskShape::Brush { strokes: vec![blob] }
            }
            (false, true) => MaskShape::Linear { start: Point::new(0.5, 0.0 + shift), end: Point::new(0.5, 0.66) },
        };
        let mut adjust =
            LocalAdjustments { exposure: 0.4, contrast: 20.0, highlights: -30.0, saturation: 10.0 * (i % 3) as f64, ..LocalAdjustments::default() };
        if i + 1 == count {
            adjust.color_hue = 30.0;
            adjust.color_sat = 40.0;
        }
        s.masks.push(Mask {
            id: i as u32 + 1,
            name: format!("Mask {}", i + 1),
            components: vec![component(MaskOp::Add, shape)],
            adjust,
            ..Mask::default()
        });
    }
    s
}
