//! Parse a photo and write what portrait retouching gets for each face, for the retouch examples.
//!
//! `cargo run -p lightcraft-parsing --release --example face_input -- in.jpg outdir/ [--set segformer|bisenet|sapiens2]
//! [--models DIR] [--edge 2048]`
//!
//! Writes `outdir/faces.json`: per face its box, its landmarks (YuNet's five, then the face mesh's 478, as the app
//! gives them) and each part mask as `outdir/<face>-<part>.png` (grey, white = the part) with the area it covers, all
//! in fractions of the photo. The masks are the model's, not refined, and a part's planes are merged into one, as the
//! app's retouch step samples them (`crates/engine/src/retouch_input/parts.rs`). The retouch examples read it with
//! `--faces outdir/faces.json`. `--models` as in the `parts` example.

use std::path::PathBuf;

use lightcraft_parsing::{FaceHint, ModelSet, NormRect, Options, Parser, Plane, RgbImage};

/// The face mesh's stand-ins for YuNet's five (the app's `MESH_FIVE`).
const MESH_FIVE: [usize; 5] = [468, 473, 1, 61, 291];
/// A parsed face is the YuNet face it was parsed from when their boxes overlap this much (the app's).
const SAME_FACE_IOU: f32 = 0.5;
/// Most samples in a merged mask (the app's).
const MAX_MERGED: f32 = (1 << 20) as f32;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn yunet(img: &RgbImage) -> Option<Vec<FaceHint>> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join("Library/Application Support/ArtCraft/models/yunet-2023mar/model.onnx");
    let det = lightcraft_faces::yunet::Detector::new(&std::fs::read(path).ok()?).ok()?;
    let faces = det.detect(&img.data, img.width, img.height, &lightcraft_faces::yunet::Options::default()).ok()?;
    Some(
        faces
            .into_iter()
            .map(|f| FaceHint { rect: NormRect::new(f.x0, f.y0, f.x1, f.y1), landmarks: Some(f.landmarks), score: f.score, person_id: None })
            .collect(),
    )
}

fn iou(a: &NormRect, b: &NormRect) -> f32 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let union = a.width() * a.height() + b.width() * b.height() - w * h;
    if union > 0.0 { w * h / union } else { 0.0 }
}

/// A part's planes merged at the finest one's resolution: the area and the grey samples.
fn merged(planes: &[Plane]) -> Option<(NormRect, usize, usize, Vec<u8>)> {
    let first = planes.first()?;
    let rect = planes.iter().fold(first.rect, |u, p| u.union(&p.rect));
    let dx = planes.iter().map(|p| p.width as f32 / p.rect.width()).fold(1.0f32, f32::max);
    let dy = planes.iter().map(|p| p.height as f32 / p.rect.height()).fold(1.0f32, f32::max);
    let (mut w, mut h) = ((rect.width() * dx).ceil().max(1.0), (rect.height() * dy).ceil().max(1.0));
    if !(w * h).is_finite() {
        return None;
    }
    if w * h > MAX_MERGED {
        let k = (MAX_MERGED / (w * h)).sqrt();
        (w, h) = ((w * k).floor().max(1.0), (h * k).floor().max(1.0));
    }
    let (w, h) = (w as usize, h as usize);
    let mut grey = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let px = rect.x0 + (x as f32 + 0.5) / w as f32 * rect.width();
            let py = rect.y0 + (y as f32 + 0.5) / h as f32 * rect.height();
            let v = planes.iter().fold(0.0f32, |a, p| a.max(p.sample(px, py)));
            grey.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    Some((rect, w, h, grey))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(out)) = (args.first(), args.get(1)) else {
        return Err("usage: face_input in.jpg outdir/ [--set segformer|bisenet|sapiens2] [--models DIR] [--edge 2048]".into());
    };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let set = arg(&args, "--set").map_or(Some(ModelSet::Segformer), |s| ModelSet::from_id(&s)).ok_or("unknown --set")?;
    let models = arg(&args, "--models")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LIGHTCRAFT_PARSING_MODELS").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/ArtCraft/models")))
        .ok_or("no models folder")?;
    let edge: u32 = arg(&args, "--edge").map_or(Ok(2048), |e| e.parse())?;

    let mut decoder = image::ImageReader::open(input)?.with_guessed_format()?.into_decoder()?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    let photo = decoded.to_rgb8();
    let long = photo.width().max(photo.height());
    let small = if long > edge { image::imageops::thumbnail(&photo, edge * photo.width() / long, edge * photo.height() / long) } else { photo };
    let img = RgbImage::new(small.width() as usize, small.height() as usize, small.into_raw())?;
    let parser = Parser::load(set, &models)?;
    let hints = yunet(&img).unwrap_or_default();
    let parsing = parser.parse(&img, &hints, &Options::default(), &|| false)?;

    let mut faces = Vec::new();
    for (k, p) in parsing.people.iter().enumerate() {
        let Some(r) = p.face.filter(NormRect::is_valid) else { continue };
        let hint = hints.iter().map(|h| (iou(&r, &h.rect), h)).filter(|(o, _)| *o >= SAME_FACE_IOU).max_by(|a, b| a.0.total_cmp(&b.0));
        let five: Option<Vec<(f32, f32)>> = match hint.and_then(|(_, h)| h.landmarks) {
            Some(l) => Some(l.to_vec()),
            None => MESH_FIVE.iter().map(|&i| p.landmarks.get(i).map(|l| (l.x, l.y))).collect(),
        };
        let landmarks: Vec<[f32; 2]> = match five {
            Some(f) => f.iter().map(|(x, y)| [*x, *y]).chain(p.landmarks.iter().map(|l| [l.x, l.y])).collect(),
            None => Vec::new(),
        };
        let mut masks = serde_json::Map::new();
        for (part, planes) in &p.parts {
            let Some((area, w, h, grey)) = merged(planes) else { continue };
            let name = format!("{k}-{}.png", part.id());
            image::GrayImage::from_raw(w as u32, h as u32, grey).ok_or("bad size")?.save(out.join(&name))?;
            masks.insert(part.id().to_string(), serde_json::json!({"png": name, "area": [area.x0, area.y0, area.x1, area.y1]}));
        }
        faces.push(serde_json::json!({"box": [r.x0, r.y0, r.x1, r.y1], "landmarks": landmarks, "masks": masks}));
    }
    let n = faces.len();
    std::fs::write(out.join("faces.json"), serde_json::to_string(&serde_json::json!({"set": set.label(), "faces": faces}))?)?;
    println!("{n} faces → {}", out.join("faces.json").display());
    Ok(())
}
