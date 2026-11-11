//! Try the blemish feature on a picture without building the app:
//!
//! `cargo run -p lightcraft-retouch --release --example blemish -- in.jpg out.jpg [--amount 100] [--keep 1]
//! [--mask facialSkin-0.png] [--face x0,y0,x1,y1] [--people people.json] [--fit 2048] [--marks marks.png] [--spots spots.csv] [--runs 1]`
//!
//! - `--amount` (0…100) and `--keep` (1: Keep Moles and Freckles, the default; 0: remove them too).
//! - `--mask` is a grey picture of the Facial Skin mask over the whole photo, at any size (white = skin): the
//!   people parser's example writes one per person (`cargo run -p lightcraft-parsing --release --example parts --
//!   in.jpg outdir/`, then `outdir/masks/facialSkin-0.png`). Repeat it, each after its `--face`, for more faces.
//! - `--face` is the face's box in fractions of the photo; without one, a mask's face is the box of its pixels
//!   above half.
//! - `--people` is the parser example's `people.json`: each face (in order) gets its person's landmarks (the five,
//!   then the face mesh), as the app's faces have them; Blemishes reads them to tell stubble below the nose.
//! - `--fit` shrinks the photo to that long edge first (1936 is the draft's size of a 3:2 photo). Spots are found
//!   on the photo shrunk to 2048 px, as the app finds them on the people parse's picture, and repaired at `--fit`
//!   (the whole photo when not given), as every render repairs them.
//! - `--marks` writes the photo with each spot circled: red a blemish, blue a mole, green a freckle; thin when it
//!   is kept at these settings.
//! - `--spots` writes each spot (place, radius, kind, weight at these settings) to a CSV file.
//! - `--runs` repeats the repair (the slider's work) and prints the fastest run.
//!
//! Runs Stray Hair's strand search first, as the app does (a spot on a strand is a hair stump or a crease).
//! Prints the time spots took to find, each face's spots by kind, how many are removed and the share of the skin
//! repaired. The picture is worked on in linear light (sRGB primaries; the app works in linear Rec.2020).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::process::ExitCode;

use lightcraft_color::transfer::{decode_srgb8, encode_srgb8};
use lightcraft_raster::{Plane, Rgb32f};
use lightcraft_retouch::blemish::{Blemish, with_spots};
use lightcraft_retouch::input::SpotKind;
use lightcraft_retouch::{Face, Features, Input, NormRect, Part, PartMask, Retouch, Stage, WholePhoto};

fn main() -> ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The mesh points of the five landmarks every face has first (eyes, nose tip, mouth corners), as the app picks them.
const MESH_FIVE: [usize; 5] = [468, 473, 1, 61, 291];

const USAGE: &str = "usage: blemish in.jpg out.jpg [--amount N] [--keep 0|1] [--face x0,y0,x1,y1] [--mask mask.png] [--people people.json] [--fit EDGE] \
                     [--marks marks.png] [--spots spots.csv] [--runs N]";

fn run(args: &[String]) -> Result<(), String> {
    let mut files = Vec::new();
    let mut global = Features::default();
    let _ = global.set("retouch.blemish.amount", 100.0);
    let mut boxes: Vec<NormRect> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let (mut fit, mut runs, mut marks) = (0usize, 1usize, None);
    let mut meshes: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut spots_csv: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| USAGE.to_string());
        match a.as_str() {
            "--amount" | "--keep" => {
                let id = if a == "--amount" { "retouch.blemish.amount" } else { "retouch.blemish.keepMoles" };
                let v: f64 = value()?.parse().map_err(|_| USAGE.to_string())?;
                global.set(id, v);
            }
            "--face" => {
                let v: Vec<f64> = value()?.split(',').filter_map(|n| n.trim().parse().ok()).collect();
                let [x0, y0, x1, y1] = v.as_slice() else { return Err(USAGE.into()) };
                boxes.push(NormRect::new(*x0, *y0, *x1, *y1));
            }
            "--mask" => {
                let path = value()?;
                let grey = image::open(path).map_err(|e| format!("{path}: {e}"))?.to_luma8();
                let (w, h) = (grey.width() as usize, grey.height() as usize);
                let plane = Plane { width: w, height: h, data: grey.pixels().map(|p| f32::from(p.0[0]) / 255.0).collect() };
                let bounds = if boxes.len() > faces.len() { boxes.pop() } else { box_of(&plane, 0.5) }.ok_or(format!("{path} is empty"))?;
                let area = box_of(&plane, 0.0).ok_or(format!("{path} is empty"))?;
                let (x0, y0) = ((area.x0 * w as f64) as usize, (area.y0 * h as f64) as usize);
                let (cw, ch) = ((area.width() * w as f64).round() as usize, (area.height() * h as f64).round() as usize);
                let plane = Plane::from_fn(cw, ch, |x, y| plane.data.get((y0 + y) * w + x0 + x).copied().unwrap_or(0.0));
                faces.push(Face { masks: vec![(Part::FacialSkin, PartMask { plane, area })], ..Face::boxed(bounds) });
            }
            "--people" => {
                let path = value()?;
                let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
                let people = json.get("people").and_then(|p| p.as_array()).cloned().unwrap_or_default();
                meshes = people
                    .iter()
                    .map(|p| {
                        let mesh: Vec<(f64, f64)> = p
                            .get("mesh")
                            .and_then(|m| m.as_array())
                            .map(|m| m.iter().filter_map(|q| Some((q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))).collect())
                            .unwrap_or_default();
                        let five: Vec<(f64, f64)> = MESH_FIVE.iter().filter_map(|i| mesh.get(*i).copied()).collect();
                        if five.len() == 5 { five.into_iter().chain(mesh).collect() } else { Vec::new() }
                    })
                    .collect();
            }
            "--fit" => fit = value()?.parse().map_err(|_| USAGE.to_string())?,
            "--runs" => runs = value()?.parse::<usize>().map_err(|_| USAGE.to_string())?.max(1),
            "--marks" => marks = Some(value()?.clone()),
            "--spots" => spots_csv = Some(value()?.clone()),
            _ => files.push(a.clone()),
        }
    }
    faces.extend(boxes.into_iter().map(Face::boxed));
    for (f, mesh) in faces.iter_mut().zip(meshes) {
        f.landmarks = mesh;
    }
    let [input, output] = files.as_slice() else { return Err(USAGE.into()) };
    let rgb = image::open(input).map_err(|e| format!("{input}: {e}"))?.to_rgb8();
    let src = linear(&fitted(&rgb, fit));
    let (w, h) = (src.width, src.height);
    let parse = linear(&fitted(&rgb, 2048));
    // as the app: Stray Hair's strands first (Blemishes leaves spots on a strand), then the spots
    let t = std::time::Instant::now();
    let faces = lightcraft_retouch::hair::with_strands(&parse, faces);
    let strands_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = std::time::Instant::now();
    let faces = with_spots(&parse, faces);
    let find_ms = t.elapsed().as_secs_f64() * 1e3;
    eprintln!("strands: {strands_ms:.1} ms (Stray Hair's search, which the app runs anyway)");
    let settings = global.blemish.clone();
    report(&faces, &settings, find_ms, parse.width, parse.height);
    let r = Retouch::with_global(global);
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
    eprintln!("repair: {w}×{h} ({:.1} MP), {best:.2} ms (fastest of {runs})", (w * h) as f64 / 1e6);
    save(&img, output)?;
    if let Some(path) = spots_csv {
        write_spots(&input, &settings, &path)?;
    }
    if let Some(path) = marks {
        save(&marked(&src, &input, &settings), &path)?;
    }
    Ok(())
}

/// `rgb` shrunk to `edge` on its long side (as it is when it is smaller, or `edge` is 0).
fn fitted(rgb: &image::RgbImage, edge: usize) -> image::RgbImage {
    if edge == 0 || rgb.width().max(rgb.height()) as usize <= edge {
        return rgb.clone();
    }
    let k = edge as f64 / f64::from(rgb.width().max(rgb.height()));
    let (w, h) = ((f64::from(rgb.width()) * k).round() as u32, (f64::from(rgb.height()) * k).round() as u32);
    image::imageops::resize(rgb, w.max(1), h.max(1), image::imageops::FilterType::Triangle)
}

fn linear(rgb: &image::RgbImage) -> Rgb32f {
    let mut img = Rgb32f::new(rgb.width() as usize, rgb.height() as usize);
    for (px, p) in img.data.iter_mut().zip(rgb.pixels()) {
        *px = [decode_srgb8(p.0[0]), decode_srgb8(p.0[1]), decode_srgb8(p.0[2])];
    }
    img
}

fn report(faces: &[Face], s: &Blemish, find_ms: f64, w: usize, h: usize) {
    eprintln!("find: {w}×{h}, {} faces, {find_ms:.1} ms", faces.len());
    for (i, f) in faces.iter().enumerate() {
        let Some(spots) = &f.spots else { continue };
        let count = |k: SpotKind| spots.list.iter().filter(|p| p.kind == k).count();
        let r = s.report(spots);
        eprintln!(
            "face {i}: {} spots ({} blemishes, {} moles, {} freckles); removed {}, kept {} moles and {} freckles, {:.2} % of the skin repaired",
            spots.list.len(),
            count(SpotKind::Blemish),
            count(SpotKind::Mole),
            count(SpotKind::Freckle),
            r.removed,
            r.moles,
            r.freckles,
            r.area * 100.0
        );
    }
}

/// `src` with every spot circled by kind (red blemish, blue mole, green freckle).
fn marked(src: &Rgb32f, input: &Input, s: &Blemish) -> Rgb32f {
    let mut img = src.clone();
    let (w, h) = (img.width as f64, img.height as f64);
    for f in &input.faces {
        let Some(spots) = &f.spots else { continue };
        let size = (f.bounds.width() * w).max(f.bounds.height() * h);
        let weights = s.weights(&spots.list);
        for (p, wt) in spots.list.iter().zip(weights) {
            let colour = match p.kind {
                SpotKind::Blemish => [1.0, 0.0, 0.0],
                SpotKind::Mole => [0.0, 0.3, 1.0],
                SpotKind::Freckle => [0.0, 1.0, 0.0],
            };
            let (cx, cy, r) = (p.at.0 * w, p.at.1 * h, p.radius * size * 2.0 + 2.0);
            let thick = if wt > 0.0 { 1.2 } else { 0.5 };
            let (x0, x1) = ((cx - r - 2.0).max(0.0) as usize, ((cx + r + 2.0).max(0.0) as usize).min(img.width));
            let (y0, y1) = ((cy - r - 2.0).max(0.0) as usize, ((cy + r + 2.0).max(0.0) as usize).min(img.height));
            for y in y0..y1 {
                for x in x0..x1 {
                    let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
                    if (d - r).abs() < thick
                        && let Some(px) = img.data.get_mut(y * img.width + x)
                    {
                        *px = colour;
                    }
                }
            }
        }
    }
    img
}

/// Every face's spots as lines of `face,x,y,radius,kind,weight` (normalised photo coordinates; the radius a fraction
/// of the face's size; the weight at these settings).
fn write_spots(input: &Input, s: &Blemish, path: &str) -> Result<(), String> {
    let mut text = String::from("face,x,y,radius,kind,weight\n");
    for (i, f) in input.faces.iter().enumerate() {
        let Some(spots) = &f.spots else { continue };
        for (p, w) in spots.list.iter().zip(s.weights(&spots.list)) {
            text.push_str(&format!("{i},{},{},{},{:?},{w}\n", p.at.0, p.at.1, p.radius, p.kind));
        }
    }
    std::fs::write(path, text).map_err(|e| format!("{path}: {e}"))
}

fn save(img: &Rgb32f, path: &str) -> Result<(), String> {
    let bytes: Vec<u8> = img.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let out = image::RgbImage::from_raw(img.width as u32, img.height as u32, bytes).ok_or("the picture's size changed")?;
    out.save(path).map_err(|e| format!("{path}: {e}"))
}

/// The box of the mask's pixels above `over`, in fractions of the photo.
fn box_of(m: &Plane, over: f32) -> Option<NormRect> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, v) in m.data.iter().enumerate() {
        if *v > over {
            let (x, y) = (i % m.width.max(1), i / m.width.max(1));
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    let (w, h) = (m.width as f64, m.height as f64);
    (x0 < x1).then(|| NormRect::new(x0 as f64 / w, y0 as f64 / h, x1 as f64 / w, y1 as f64 / h))
}
