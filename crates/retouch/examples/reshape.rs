//! Try Face Reshape on a picture without building the app:
//!
//! ```text
//! cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/
//! cargo run -p lightcraft-retouch --release --example reshape -- in.jpg out.jpg --people outdir/people.json \
//!   --set slimFace=60 --set eyeSize=40 [--marks]
//! ```
//!
//! `--people` reads the face meshes the people parser's example writes (`people.json`, each face's `mesh`);
//! `--face x0,y0,x1,y1` instead puts a stand-in mesh in that box ([`lightcraft_retouch::reshape::sketch_mesh`]).
//! `--set <slider>=<value>` sets a slider (`slimFace`, `jaw`, `chin`, `doubleChin`, `nose`, `forehead`, `eyeSize`;
//! −100…100) for every face. `--marks` draws the points the sliders move. The picture is resampled through the warp
//! as the app's geometry step does it (bilinear), and the time the warp took to build is printed.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::Rgb32f;
use lightcraft_retouch::reshape::{Field, MESH, MESH_POINTS, Reshape, kernels, sketch_mesh};
use lightcraft_retouch::{Features, NormRect};

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Every face's landmarks (YuNet's five, then the mesh) from the parser example's `people.json`.
fn people(path: &str) -> Result<Vec<Vec<(f64, f64)>>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let list = v.get("people").and_then(|p| p.as_array()).ok_or_else(|| format!("{path}: no people"))?;
    Ok(list
        .iter()
        .filter_map(|p| {
            let mesh: Vec<(f64, f64)> = p.get("mesh")?.as_array()?.iter().filter_map(|q| Some((q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))).collect();
            if mesh.len() < MESH_POINTS {
                return None;
            }
            let five = [468, 473, 1, 61, 291].map(|i| mesh.get(i).copied().unwrap_or((0.5, 0.5)));
            Some(five.into_iter().chain(mesh).collect())
        })
        .collect())
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = || "usage: reshape in.jpg out.jpg (--people people.json | --face x0,y0,x1,y1)... [--set slider=value]... [--marks]".to_string();
    let (mut files, mut faces, mut marks) = (Vec::new(), Vec::new(), false);
    let mut features = Features::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--people" => faces.extend(people(it.next().ok_or_else(usage)?)?),
            "--face" => {
                let v: Vec<f64> = it.next().ok_or_else(usage)?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = v.as_slice() else { return Err(usage()) };
                faces.push(sketch_mesh(NormRect::new(*x0, *y0, *x1, *y1)));
            }
            "--set" => {
                let (k, v) = it.next().and_then(|s| s.split_once('=')).ok_or_else(usage)?;
                let v: f64 = v.parse().map_err(|_| usage())?;
                if !features.set(&format!("retouch.reshape.{k}"), v) {
                    return Err(format!("no slider {k}"));
                }
            }
            "--marks" => marks = true,
            _ => files.push(a.clone()),
        }
    }
    let [input, output] = files.as_slice() else { return Err(usage()) };
    let rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut img = Rgb32f::new(w, h);
    for (px, p) in img.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let s: &Reshape = &features.reshape;
    let list: Vec<(&[(f64, f64)], &Reshape)> = faces.iter().map(|f| (f.as_slice(), s)).collect();
    let t = std::time::Instant::now();
    let field = Field::build(&list, w as f64, h as f64);
    eprintln!("reshape: {w}×{h}, {} faces, warp built in {:.2} ms", faces.len(), t.elapsed().as_secs_f64() * 1e3);
    let mut out = match &field {
        Some(f) => {
            let mut out = img.clone();
            for [x0, y0, x1, y1] in f.areas() {
                let (px0, py0) = ((x0 * w as f64).floor().max(0.0) as usize, (y0 * h as f64).floor().max(0.0) as usize);
                let (px1, py1) = ((x1 * w as f64).ceil().min(w as f64) as usize, (y1 * h as f64).ceil().min(h as f64) as usize);
                for y in py0..py1 {
                    for x in px0..px1 {
                        let (sx, sy) = f.source((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
                        let v = img.sample_bilinear((sx * w as f64) as f32, (sy * h as f64) as f32);
                        if let Some(px) = out.data.get_mut(y * w + x) {
                            *px = v;
                        }
                    }
                }
            }
            out
        }
        None => img,
    };
    if marks {
        let long = w.max(h) as f64;
        for f in &faces {
            let mesh: Vec<[f64; 2]> = f.iter().skip(MESH).map(|(x, y)| [x * w as f64 / long, y * h as f64 / long]).collect();
            let all = Reshape { slim_face: 1.0, jaw: 1.0, chin: 1.0, double_chin: 1.0, nose: 1.0, forehead: 1.0, eye_size: 0.0 };
            for k in kernels(&mesh, &all) {
                let (cx, cy) = ((k.centre[0] * long) as isize, (k.centre[1] * long) as isize);
                for d in -3..=3_isize {
                    for (x, y) in [(cx + d, cy), (cx, cy + d)] {
                        if x >= 0
                            && y >= 0
                            && (x as usize) < w
                            && (y as usize) < h
                            && let Some(px) = out.data.get_mut(y as usize * w + x as usize)
                        {
                            *px = [0.0, 1.0, 0.3];
                        }
                    }
                }
            }
        }
    }
    let bytes: Vec<u8> = out.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let img = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    img.save(output).map_err(|e| format!("{output}: {e}"))
}
