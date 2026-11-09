//! Try the eyes feature on a picture without building the app:
//!
//! `cargo run -p lightcraft-retouch --release --example eyes -- in.jpg out.jpg --faces dir/faces.json
//! [--whites 50] [--iris 50] [--dark 50] [--bags 50] [--width 1936] [--runs 5]`
//!
//! `--faces` reads what the app's retouch step gets for each face (box, landmarks, part masks), written by
//! `cargo run -p lightcraft-parsing --release --example face_input -- in.jpg dir/`. `--face x0,y0,x1,y1` instead
//! gives a bare box (no landmarks, no masks: nothing changes). `--width` scales the picture first (a draft is about
//! 1936 px wide, the loupe 2560), and `--runs` times the step that many times (the slider update; the best run is
//! printed).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;
use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::{Face, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

const USAGE: &str = "usage: eyes in.jpg out.jpg [--faces faces.json | --face x0,y0,x1,y1] [--whites 0..100] [--iris ..] [--dark ..] [--bags ..] [--width px] [--runs n]";

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut faces = Vec::new();
    let mut r = Retouch::default();
    let (mut width, mut runs) = (None, 1usize);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(USAGE.to_string());
        let mut set = |field: &str, v: &str| -> Result<(), String> {
            let v: f64 = v.parse().map_err(|_| USAGE.to_string())?;
            r.global.set(&format!("retouch.eyes.{field}"), v).then_some(()).ok_or(format!("no slider {field}"))
        };
        match a.as_str() {
            "--faces" => faces.extend(read_faces(Path::new(value()?))?),
            "--face" => {
                let v: Vec<f64> = value()?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = v.as_slice() else { return Err(USAGE.into()) };
                faces.push(Face::boxed(NormRect::new(*x0, *y0, *x1, *y1)));
            }
            "--whites" => set("whites", value()?)?,
            "--iris" => set("iris", value()?)?,
            "--dark" => set("darkCircles", value()?)?,
            "--bags" => set("eyeBags", value()?)?,
            "--width" => width = Some(value()?.parse::<u32>().map_err(|_| USAGE.to_string())?),
            "--runs" => runs = value()?.parse::<usize>().map_err(|_| USAGE.to_string())?.max(1),
            _ => files.push(a.clone()),
        }
    }
    let [input, output] = files.as_slice() else { return Err(USAGE.into()) };
    let mut rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    if let Some(nw) = width {
        let nh = (u64::from(rgb.height()) * u64::from(nw) / u64::from(rgb.width().max(1))).max(1) as u32;
        rgb = image::imageops::resize(&rgb, nw, nh, image::imageops::FilterType::Triangle);
    }
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut src = Rgb32f::new(w, h);
    for (px, p) in src.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    let input = Input::new(faces);
    let view = WholePhoto { width: w, height: h };
    let mut best = f64::INFINITY;
    let mut img = src.clone();
    for _ in 0..runs {
        img = src.clone();
        let t = std::time::Instant::now();
        lightcraft_retouch::run::apply(&mut img, &r, &input, &view, Stage::Retouch);
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!("eyes: {w}×{h}, {} faces, {best:.1} ms", input.faces.len());
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("the picture's size changed")?;
    out.save(output).map_err(|e| format!("{output}: {e}"))
}

/// The faces `face_input` wrote: per face a box, landmarks and part masks (grey pictures beside the file).
fn read_faces(path: &Path) -> Result<Vec<Face>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let rect = |v: &serde_json::Value| -> Option<NormRect> {
        let a: Vec<f64> = v.as_array()?.iter().filter_map(serde_json::Value::as_f64).collect();
        let [x0, y0, x1, y1] = a.as_slice() else { return None };
        Some(NormRect::new(*x0, *y0, *x1, *y1))
    };
    let mut faces = Vec::new();
    for f in json["faces"].as_array().ok_or("no faces list")? {
        let bounds = rect(&f["box"]).ok_or("a face without a box")?;
        let landmarks = f["landmarks"]
            .as_array()
            .map(|l| l.iter().filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?))).collect())
            .unwrap_or_default();
        let mut masks = Vec::new();
        for part in Part::ALL {
            let id = serde_json::to_value(part).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let Some(m) = f["masks"].get(&id) else { continue };
            let (Some(png), Some(area)) = (m["png"].as_str(), rect(&m["area"])) else { continue };
            let grey = image::open(dir.join(png)).map_err(|e| format!("{png}: {e}"))?.to_luma8();
            let plane = Plane {
                width: grey.width() as usize,
                height: grey.height() as usize,
                data: grey.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect(),
            };
            masks.push((part, PartMask { plane, area }));
        }
        faces.push(Face { landmarks, masks, ..Face::boxed(bounds) });
    }
    Ok(faces)
}
