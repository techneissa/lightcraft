//! SCRATCH for issue #438, never merged: brush strokes painted one at a time onto a stored mask (a kept brush part),
//! f32 against u16, on the GPU and the CPU. `u16_strokes <photo> <outdir>`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lightcraft_develop::{BrushStroke, DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_geom::Point;
use lightcraft_pipeline::mask_store::{self, Format};
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk, strokes_rasterized};
use lightcraft_pipeline::{OutputDepth, OutputSpace, StageCache};
use lightcraft_raster::Plane;
use serde_json::json;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as f64 / (1u64 << 31) as f64
    }
}

/// Stroke `k` of a set: soft, overlapping the others in the middle of the frame.
fn stroke(r: &mut Rng, flow: f64, erase: bool, size: f64) -> BrushStroke {
    let (a, b) = ((0.2 + 0.3 * r.next(), 0.2 + 0.6 * r.next()), (0.5 + 0.3 * r.next(), 0.2 + 0.6 * r.next()));
    let points: Vec<Point> = (0..20).map(|i| i as f64 / 19.0).map(|t| Point::new(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)).collect();
    BrushStroke { points: points.into(), size, feather: 100.0, flow, density: 100.0, erase, ..Default::default() }
}

fn set(name: &str) -> Vec<BrushStroke> {
    let mut r = Rng(7);
    (0..50)
        .map(|k| {
            let flow = 3.0 + 40.0 * r.next();
            match name {
                // 50 soft paint strokes
                "paint" => stroke(&mut r, flow, false, 0.18),
                // every third one erases
                "paint-erase" => stroke(&mut r, flow, k % 3 == 2, 0.18),
                // one heavy stroke, then 49 light erasers over it: every erase multiplies what was stored
                "erase-worst" => {
                    if k == 0 {
                        stroke(&mut r, 100.0, false, 0.9)
                    } else {
                        let f = 1.0 + 3.0 * r.next();
                        stroke(&mut r, f, true, 0.5)
                    }
                }
                _ => panic!("set {name}"),
            }
        })
        .collect()
}

fn settings(strokes: &[BrushStroke]) -> DevelopSettings {
    let adjust = LocalAdjustments { exposure: 4.0, shadows: 100.0, amount: 100.0, ..Default::default() };
    let shape = MaskShape::Brush { strokes: strokes.to_vec() };
    let m = Mask {
        id: 1,
        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
        adjust,
        on_photo: true,
        ..Default::default()
    };
    let mut s = DevelopSettings { masks: vec![m], ..Default::default() };
    s.light.shadows = 100.0;
    s
}

fn alpha_diff(a: &Plane, b: &Plane) -> (f64, f64, f64) {
    let (mut max, mut changed, mut sum) = (0f64, 0usize, 0f64);
    for (x, y) in a.data.iter().zip(&b.data) {
        let d = f64::from((x - y).abs()) * 65535.0;
        max = max.max(d);
        sum += d;
        if d >= 0.5 {
            changed += 1;
        }
    }
    let n = a.data.len() as f64;
    (max, 100.0 * changed as f64 / n, sum / n)
}

fn image_diff(a: &[f32], b: &[f32], step: f32) -> (f32, f64) {
    let (mut max, mut changed) = (0f32, 0usize);
    for (x, y) in a.chunks_exact(3).zip(b.chunks_exact(3)) {
        let d = (0..3).map(|c| (x[c] - y[c]).abs()).fold(0f32, f32::max);
        max = max.max(d);
        if d >= step {
            changed += 1;
        }
    }
    (max, 100.0 * changed as f64 / (a.len() / 3) as f64)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let photo = &args[0];
    let mut s = Session::new().with_fs().with_system_clock();
    let r = s.execute("library.import", &json!({"paths": [std::fs::canonicalize(photo).unwrap()]})).unwrap();
    let id = r["imported"][0].as_u64().expect("imported");
    s.execute("library.select", &json!({"ids": [id], "active": id})).unwrap();
    s.execute("app.gpu", &json!({"enabled": true, "wait": true})).unwrap();
    // the loupe's size: strokes are painted on the preview; `SIZE=N` for another
    let long: usize = std::env::var("SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(2560);
    let job = s.export_job(PhotoId(id), long, long, OutputSpace::Srgb, OutputDepth::U8).unwrap();
    let source = job.source.load_source().unwrap();
    let src = source.image.clone();
    let info = source.info.clone().unwrap_or_else(|| job.info.clone());
    // the picture at the render's size, so every render has the same input
    let src = {
        let (w, h) = (src.width, src.height);
        let k = (long as f64 / w.max(h) as f64).min(1.0);
        let fit =
            lightcraft_raster::resample::fit(&src, (w as f64 * k) as usize, (h as f64 * k) as usize, lightcraft_raster::resample::Filter::Lanczos3);
        std::sync::Arc::new(fit)
    };
    let req = job.request;

    for gpu in [true, false] {
        lightcraft_gpu::set_enabled(gpu);
        let path = if gpu { "GPU" } else { "CPU" };
        let depths: &[(OutputDepth, &str)] =
            if gpu { &[(OutputDepth::U8, "8-bit")] } else { &[(OutputDepth::U8, "8-bit"), (OutputDepth::U16, "16-bit")] };
        let image = |st: &DevelopSettings, depth: OutputDepth, cache: Option<&StageCache>| -> Vec<f32> {
            let req = lightcraft_pipeline::RenderRequest { depth, ..req };
            let r = if gpu {
                lightcraft_gpu::render(&src, &info, st, &req, cache).expect("gpu render")
            } else {
                match cache {
                    Some(c) => lightcraft_pipeline::render_cached(&src, &info, st, &req, c),
                    None => lightcraft_pipeline::render(&src, &info, st, &req),
                }
            };
            match r.deep.map(|d| d.samples) {
                Some(lightcraft_pipeline::DeepSamples::U16(v)) => v.iter().map(|x| f32::from(*x)).collect(),
                _ => r.image.data.iter().flat_map(|c| [c[0], c[1], c[2]]).map(f32::from).collect(),
            }
        };
        let planes = |st: &DevelopSettings, cache: Option<&StageCache>| -> Plane {
            let ask = PlanesAsk { want: MaskWant::All, known: None };
            let out = if gpu {
                lightcraft_gpu::mask_planes(&src, &info, st, &req, cache, &ask).expect("gpu planes")
            } else {
                lightcraft_pipeline::mask_planes(&src, &info, st, &req, cache, &ask)
            };
            out.planes.expect("planes").remove(0).1
        };
        for name in ["paint", "paint-erase", "erase-worst"] {
            let strokes = set(name);
            // f32 and u16, each painted one stroke at a time onto what its own cache keeps
            let (c32, c16) = (StageCache::default(), StageCache::default());
            let before = strokes_rasterized();
            for n in 1..=strokes.len() {
                let st = settings(&strokes[..n]);
                mask_store::set(Format::F32);
                let a = planes(&st, Some(&c32));
                mask_store::set(Format::U16);
                let b = planes(&st, Some(&c16));
                if matches!(n, 1 | 5 | 10 | 25 | 50) {
                    let (max, changed, mean) = alpha_diff(&a, &b);
                    println!("ALPHA\t{path}\t{name}\t{n}\t{max:.2}\t{changed:.3}\t{mean:.4}\t{}x{}", a.width, a.height);
                }
            }
            println!("STAMPED\t{path}\t{name}\t{} (100 = each stroke once per format)", strokes_rasterized() - before);
            let st = settings(&strokes);
            for &(depth, dname) in depths {
                // the picture from the stroke-by-stroke caches, against f32; and against an export's all-at-once u16
                mask_store::set(Format::F32);
                let a = image(&st, depth, Some(&c32));
                let once32 = image(&st, depth, None);
                mask_store::set(Format::U16);
                let b = image(&st, depth, Some(&c16));
                let once16 = image(&st, depth, None);
                let (m, c) = image_diff(&a, &b, 1.0);
                let (m32, c32_) = image_diff(&once32, &a, 1.0);
                let (m16, c16_) = image_diff(&once32, &once16, 1.0);
                println!(
                    "IMAGE\t{path}\t{name}\t{dname}\tstroke-by-stroke u16 vs f32: max {m} changed {c:.4}%\tat once u16 vs f32: max {m16} changed {c16_:.4}%\tf32 stroke-by-stroke vs at once: max {m32} changed {c32_:.4}%"
                );
            }
        }
    }
    lightcraft_gpu::set_enabled(true);
}
