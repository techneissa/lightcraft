//! What switching input shapes costs the converted people-parts models (issue #200): runs the compiled clothes parser
//! and face parser from the models folder on made-up pixels, in a few call patterns, and prints the wall and CPU time
//! per call.
//!
//! ```text
//! cargo run --release -p lightcraft-parsing --example coreml_shapes -- <models folder>
//! ```

use std::path::Path;
use std::time::Instant;

use lightcraft_coreml::{ComputeUnits, Model};

fn cpu_ms() -> f64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    t.tv_sec as f64 * 1e3 + t.tv_nsec as f64 / 1e6
}

fn body_input(ch: usize, cw: usize, h: usize, w: usize) -> Vec<f32> {
    let n = ch * cw;
    let mut x = vec![0.0f32; 4 * n];
    for y in 0..h {
        for xx in 0..w {
            for k in 0..3 {
                x[k * n + y * cw + xx] = ((xx * 7 + y * 13 + k * 31) % 255) as f32 / 255.0 - 0.45;
            }
            x[3 * n + y * cw + xx] = 1.0;
        }
    }
    x
}

/// Run `calls` (model index, canvas h, canvas w, grid h, grid w) `rounds` times; print the mean per call after the
/// first round (the first calls per shape compile).
fn pattern(name: &str, models: &[&Model], calls: &[(usize, usize, usize, usize, usize)], rounds: usize) {
    let inputs: Vec<Vec<f32>> = calls.iter().map(|&(_, ch, cw, h, w)| body_input(ch, cw, h, w)).collect();
    let (mut wall, mut cpu, mut n) = (0.0, 0.0, 0);
    for r in 0..rounds {
        for (&(m, ch, cw, _, _), x) in calls.iter().zip(&inputs) {
            let (t, c) = (Instant::now(), cpu_ms());
            let out = models[m].predict("pixels", &[1, 4, ch, cw], x, "probs");
            let (dw, dc) = (t.elapsed().as_secs_f64() * 1e3, cpu_ms() - c);
            if let Err(e) = out {
                println!("{name}: {e}");
                return;
            }
            if r > 0 {
                wall += dw;
                cpu += dc;
                n += 1;
            }
        }
    }
    let n = f64::from(n.max(1));
    println!("{name:<48} {:>7.1} ms wall {:>7.1} ms CPU a call", wall / n, cpu / n);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).ok_or("usage: coreml_shapes <models folder>")?;
    let dir = Path::new(&dir);
    let body = dir.join("human-parsing-segformer-b3/coreml/human-parsing-segformer-b3.mlmodelc");
    let units = ComputeUnits::CpuAndGpu;
    if std::env::var_os("SHAPES_ALTERNATE").is_some() {
        let m = Model::load(&body, units)?;
        let (l, p) = (body_input(512, 768, 512, 768), body_input(768, 512, 768, 512));
        for i in 0..24 {
            let (t, c) = (Instant::now(), cpu_ms());
            if i % 2 == 0 {
                m.predict("pixels", &[1, 4, 512, 768], &l, "probs")?;
            } else {
                m.predict("pixels", &[1, 4, 768, 512], &p, "probs")?;
            }
            if i % 4 == 3 {
                println!("call {i}: {:.0} ms wall {:.0} ms CPU, footprint {}", t.elapsed().as_secs_f64() * 1e3, cpu_ms() - c, footprint());
            }
        }
        return Ok(());
    }
    if std::env::var_os("SHAPES_MEMORY").is_some() {
        println!("start: {}", footprint());
        let m = Model::load(&body, units)?;
        println!("one clothes parser loaded: {}", footprint());
        for (ch, cw) in [(512usize, 768usize), (512, 768), (768, 512), (576, 768), (704, 1024), (512, 768)] {
            m.predict("pixels", &[1, 4, ch, cw], &body_input(ch, cw, ch, cw), "probs")?;
            println!("after a call at {ch} × {cw}: {}", footprint());
        }
        drop(m);
        println!("dropped: {}", footprint());
        let ms: Vec<Model> = (0..3).map(|_| Model::load(&body, units)).collect::<Result<_, _>>()?;
        for (m, (ch, cw)) in ms.iter().zip([(512usize, 768usize), (768, 512), (576, 768)]) {
            m.predict("pixels", &[1, 4, ch, cw], &body_input(ch, cw, ch, cw), "probs")?;
            println!("instance per shape, after {ch} × {cw}: {}", footprint());
        }
        return Ok(());
    }
    let a = Model::load(&body, units)?;
    let b = Model::load(&body, units)?;
    let land = (512, 768, 512, 768);
    let port = (768, 512, 768, 512);
    let sq_land = (768, 768, 512, 768);
    let sq_port = (768, 768, 768, 512);
    let one = |m: usize, s: (usize, usize, usize, usize)| (m, s.0, s.1, s.2, s.3);
    pattern("one model, landscape only", &[&a], &[one(0, land)], 4);
    pattern("one model, portrait only", &[&a], &[one(0, port)], 4);
    pattern("one model, square canvas only", &[&a], &[one(0, sq_land)], 4);
    pattern("one model, landscape then portrait", &[&a], &[one(0, land), one(0, port)], 4);
    pattern("one model, square canvas, both orientations", &[&a], &[one(0, sq_land), one(0, sq_port)], 4);
    pattern("a model per orientation, alternating", &[&a, &b], &[one(0, land), one(1, port)], 4);
    pattern("one model, 768 then 1024 landscape", &[&a], &[one(0, land), (0, 704, 1024, 704, 1024)], 3);
    // memory: each instance after its first call
    println!("footprint before more instances: {}", footprint());
    let mut extra = Vec::new();
    for (k, &(ch, cw)) in [(512usize, 768usize), (768, 512), (576, 768), (704, 1024)].iter().enumerate() {
        let m = Model::load(&body, units)?;
        m.predict("pixels", &[1, 4, ch, cw], &body_input(ch, cw, ch, cw), "probs")?;
        extra.push(m);
        println!("footprint with {} more clothes-parser instances ({ch} × {cw}): {}", k + 1, footprint());
    }
    drop(extra);
    println!("footprint after dropping them: {}", footprint());
    // the face parser: 1 crop, then 2, then 1 again
    let face = dir.join("face-parsing-segformer-b0/coreml/face-parsing-segformer-b0.mlmodelc");
    let f = Model::load(&face, units)?;
    let crop = |n: usize| (0..n * 3 * 512 * 512).map(|i| ((i * 7) % 255) as f32 / 255.0 - 0.45).collect::<Vec<f32>>();
    for (name, batches) in [
        ("face parser, 1 crop a call", vec![1usize, 1]),
        ("face parser, 1 then 2 crops", vec![1, 2]),
        ("face parser, 8 crops", vec![8]),
        ("face parser, 1 then 8 crops", vec![1, 8]),
    ] {
        let xs: Vec<Vec<f32>> = batches.iter().map(|&n| crop(n)).collect();
        let (mut wall, mut cpu, mut faces) = (0.0, 0.0, 0usize);
        for r in 0..4 {
            for (&n, x) in batches.iter().zip(&xs) {
                let (t, c) = (Instant::now(), cpu_ms());
                f.predict("pixels", &[n, 3, 512, 512], x, "groups")?;
                if r > 0 {
                    wall += t.elapsed().as_secs_f64() * 1e3;
                    cpu += cpu_ms() - c;
                    faces += n;
                }
            }
        }
        let k = faces.max(1) as f64;
        println!("{name:<48} {:>7.1} ms wall {:>7.1} ms CPU a face", wall / k, cpu / k);
    }
    // a group of 26 faces, then a portrait of one, as call patterns
    for (name, calls) in [
        ("26 faces as 8,8,8,2 then 1 (before)", vec![8usize, 8, 8, 2, 1]),
        ("26 faces as 8,8,8,1,1 then 1", vec![8, 8, 8, 1, 1, 1]),
        ("26 faces as 8,8,8,8 (padded) then 1", vec![8, 8, 8, 8, 1]),
        ("26 faces one a call then 1", vec![1; 27]),
    ] {
        let xs: Vec<Vec<f32>> = calls.iter().map(|&n| crop(n)).collect();
        let (mut wall, mut cpu) = (0.0, 0.0);
        for r in 0..4 {
            let (t, c) = (Instant::now(), cpu_ms());
            for (&n, x) in calls.iter().zip(&xs) {
                f.predict("pixels", &[n, 3, 512, 512], x, "groups")?;
            }
            if r > 0 {
                wall += t.elapsed().as_secs_f64() * 1e3;
                cpu += cpu_ms() - c;
            }
        }
        println!("{name:<48} {:>7.1} ms wall {:>7.1} ms CPU a group and portrait", wall / 3.0, cpu / 3.0);
    }
    let mesh = dir.join("facemesh-v2/coreml/facemesh-v2.mlmodelc");
    let m = Model::load(&mesh, units)?;
    let mcrop = |n: usize| (0..n * 3 * 256 * 256).map(|i| ((i * 7) % 255) as f32 / 255.0).collect::<Vec<f32>>();
    for (name, batches) in [("face mesh, 1 crop a call", vec![1usize, 1]), ("face mesh, 8 crops", vec![8]), ("face mesh, 1 then 8 crops", vec![1, 8])]
    {
        let xs: Vec<Vec<f32>> = batches.iter().map(|&n| mcrop(n)).collect();
        let (mut wall, mut cpu, mut faces) = (0.0, 0.0, 0usize);
        for r in 0..4 {
            for (&n, x) in batches.iter().zip(&xs) {
                let (t, c) = (Instant::now(), cpu_ms());
                m.predict("pixels", &[n, 3, 256, 256], x, "mesh")?;
                if r > 0 {
                    wall += t.elapsed().as_secs_f64() * 1e3;
                    cpu += cpu_ms() - c;
                    faces += n;
                }
            }
        }
        let k = faces.max(1) as f64;
        println!("{name:<48} {:>7.1} ms wall {:>7.1} ms CPU a face", wall / k, cpu / k);
    }
    Ok(())
}

/// The process's physical footprint, as `footprint` reports it.
fn footprint() -> String {
    let out = std::process::Command::new("footprint").arg(std::process::id().to_string()).output();
    out.map_or_else(
        |_| "?".into(),
        |o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find(|l| l.contains("Footprint:"))
                .and_then(|l| l.split("Footprint:").nth(1))
                .unwrap_or("?")
                .trim()
                .to_string()
        },
    )
}
