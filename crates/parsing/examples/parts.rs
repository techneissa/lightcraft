//! Parse a photo and write each part mask as a picture.
//!
//! `cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/ [--set segformer|bisenet|sapiens2]
//! [--models DIR] [--edge 2048] [--no-yunet] [--cells 1.5] [--eps 0.0002]`
//!
//! Writes `outdir/<part>.png` (the photo dimmed, the part in colour, one colour per person), `outdir/all.png` (every
//! part in its own colour), `outdir/landmarks.png`, `outdir/people.json` (with each face's mesh points, which the
//! Face Reshape example reads) and each person's masks as grey pictures (`outdir/masks/<part>-<person>.png`, white =
//! the part; the retouch examples read them) and everybody's (`outdir/masks/<part>-all.png`), and prints the time each step took.
//! Faces come from the YuNet detector in the shared models folder when it is there (as in the app), else from the
//! body model. `--models` defaults to `LIGHTCRAFT_PARSING_MODELS`, else `~/Library/Application Support/ArtCraft/models`.

use std::path::PathBuf;
use std::time::Instant;

use lightcraft_parsing::render::{self, Select};
use lightcraft_parsing::{FaceHint, ModelSet, NormRect, Options, Parser, Part, RgbImage};

const COLOURS: [[u8; 3]; 8] =
    [[255, 64, 64], [64, 160, 255], [80, 220, 80], [255, 200, 40], [220, 80, 255], [40, 230, 230], [255, 140, 60], [160, 160, 255]];

fn part_colour(p: Part) -> [u8; 3] {
    match p {
        Part::EntirePerson => [128, 128, 128],
        Part::FacialSkin => [255, 190, 150],
        Part::BodySkin => [200, 120, 90],
        Part::Hair => [120, 60, 20],
        Part::Eyebrows => [60, 30, 10],
        Part::EyeWhites => [255, 255, 255],
        Part::Iris => [40, 120, 255],
        Part::Lips => [230, 30, 80],
        Part::Teeth => [255, 255, 160],
        Part::ClothesTop => [40, 200, 80],
        Part::ClothesBottom => [30, 80, 200],
        Part::Dress => [200, 40, 200],
        Part::Shoes => [255, 120, 0],
        Part::Hat => [0, 200, 200],
        Part::Bag => [150, 100, 255],
        Part::Others => [255, 255, 0],
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn yunet(img: &RgbImage) -> Option<Vec<FaceHint>> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join("Library/Application Support/ArtCraft/models/yunet-2023mar/model.onnx");
    let bytes = std::fs::read(path).ok()?;
    let det = lightcraft_faces::yunet::Detector::new(&bytes).ok()?;
    let faces = det.detect(&img.data, img.width, img.height, &lightcraft_faces::yunet::Options::default()).ok()?;
    Some(
        faces
            .into_iter()
            .map(|f| FaceHint { rect: NormRect::new(f.x0, f.y0, f.x1, f.y1), landmarks: Some(f.landmarks), score: f.score, person_id: None })
            .collect(),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(out)) = (args.first(), args.get(1)) else {
        return Err("usage: parts in.jpg outdir/ [--set segformer|bisenet|sapiens2] [--models DIR] [--edge 2048] [--no-yunet]".into());
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

    let t = Instant::now();
    let mut decoder = image::ImageReader::open(input)?.with_guessed_format()?.into_decoder()?;
    let orientation = image::ImageDecoder::orientation(&mut decoder)?;
    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    let photo = decoded.to_rgb8();
    let small = if photo.width().max(photo.height()) > edge {
        image::imageops::thumbnail(
            &photo,
            edge * photo.width() / photo.width().max(photo.height()),
            edge * photo.height() / photo.width().max(photo.height()),
        )
    } else {
        photo.clone()
    };
    let img = RgbImage::new(small.width() as usize, small.height() as usize, small.into_raw())?;
    println!("read {}×{} → {}×{} in {:.0} ms", photo.width(), photo.height(), img.width, img.height, t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    let parser = Parser::load(set, &models)?;
    println!("loaded {} on {:?} in {:.0} ms", set.label(), parser.device(), t.elapsed().as_secs_f64() * 1e3);

    let t = Instant::now();
    let hints = if args.iter().any(|a| a == "--no-yunet") { Vec::new() } else { yunet(&img).unwrap_or_default() };
    println!("YuNet: {} faces in {:.0} ms", hints.len(), t.elapsed().as_secs_f64() * 1e3);

    for run in 0..2 {
        let t = Instant::now();
        let parsing = parser.parse(&img, &hints, &Options::default(), &|| false)?;
        println!("parse {}: {} people in {:.0} ms", run + 1, parsing.people.len(), t.elapsed().as_secs_f64() * 1e3);
        if run == 1 {
            let radius: f32 = arg(&args, "--cells").map_or(Ok(render::CELLS), |e| e.parse())?;
            let eps: f32 = arg(&args, "--eps").map_or(Ok(render::EPS), |e| e.parse())?;
            write(&out, &img, &parsing, radius, eps)?;
        }
    }
    Ok(())
}

fn write(
    out: &std::path::Path,
    img: &RgbImage,
    parsing: &lightcraft_parsing::Parsing,
    radius: f32,
    eps: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let (w, h) = (img.width, img.height);
    let dim = |i: usize, c: usize| f32::from(img.data.get(i * 3 + c).copied().unwrap_or(0)) * 0.35;
    let mut all: Vec<f32> = (0..w * h * 3).map(|k| dim(k / 3, k % 3)).collect();
    let t = Instant::now();
    for part in Part::ALL {
        let mut pic: Vec<f32> = (0..w * h * 3).map(|k| dim(k / 3, k % 3)).collect();
        let mut any = false;
        for (k, _) in parsing.people.iter().enumerate() {
            let m = render::refined_with(parsing, Select::Person(k, part), img, radius, eps);
            if m.iter().any(|v| *v > 0.0) {
                let grey: Vec<u8> = m.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
                std::fs::create_dir_all(out.join("masks"))?;
                image::GrayImage::from_raw(w as u32, h as u32, grey).ok_or("bad size")?.save(out.join(format!("masks/{}-{k}.png", part.id())))?;
            }
            let c = COLOURS[k % COLOURS.len()];
            for (i, &v) in m.iter().enumerate() {
                if v > 0.0 {
                    any = true;
                    for ch in 0..3 {
                        pic[i * 3 + ch] = pic[i * 3 + ch] * (1.0 - v) + f32::from(c[ch]) * v;
                        all[i * 3 + ch] = all[i * 3 + ch] * (1.0 - v * 0.8) + f32::from(part_colour(part)[ch]) * v * 0.8;
                    }
                }
            }
        }
        if any {
            save(&out.join(format!("{}.png", part.id())), w, h, &pic)?;
            // the part over everybody, as the app's masks draw it
            let m = render::refined_with(parsing, Select::Everyone(part), img, radius, eps);
            let grey: Vec<u8> = m.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
            image::GrayImage::from_raw(w as u32, h as u32, grey).ok_or("bad size")?.save(out.join(format!("masks/{}-all.png", part.id())))?;
        }
    }
    let bg = render::refined_with(parsing, Select::Background, img, radius, eps);
    let pic: Vec<f32> = (0..w * h * 3).map(|k| dim(k / 3, k % 3) * (1.0 - bg[k / 3]) + f32::from(img.data[k]) * bg[k / 3]).collect();
    save(&out.join("background.png"), w, h, &pic)?;
    println!("masks drawn (guided filter, {w}×{h}) in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    save(&out.join("all.png"), w, h, &all)?;
    let mut lm: Vec<f32> = (0..w * h * 3).map(|k| dim(k / 3, k % 3) * 2.0).collect();
    for p in &parsing.people {
        for l in &p.landmarks {
            let (x, y) = ((l.x * w as f32) as isize, (l.y * h as f32) as isize);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)] {
                let (px, py) = (x + dx, y + dy);
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    let i = py as usize * w + px as usize;
                    lm[i * 3..i * 3 + 3].copy_from_slice(&[0.0, 255.0, 120.0]);
                }
            }
        }
    }
    save(&out.join("landmarks.png"), w, h, &lm)?;
    let people: Vec<serde_json::Value> = parsing
        .people
        .iter()
        .map(|p| {
            let areas: serde_json::Map<String, serde_json::Value> = p.parts.keys().map(|k| (k.id().to_string(), serde_json::json!((p.area(*k) * 1e4).round() / 1e4))).collect();
            let planes: serde_json::Map<String, serde_json::Value> =
                p.parts.iter().map(|(k, v)| (k.id().to_string(), serde_json::json!(v.iter().map(|q| format!("{}x{} @ {:.3},{:.3}-{:.3},{:.3}", q.width, q.height, q.rect.x0, q.rect.y0, q.rect.x1, q.rect.y1)).collect::<Vec<_>>()))).collect();
            // (`mesh`: the face mesh's points, which the Face Reshape example reads)
            let mesh: Vec<[f32; 2]> = p.landmarks.iter().map(|l| [l.x, l.y]).collect();
            serde_json::json!({"face": p.face.map(|f| [f.x0, f.y0, f.x1, f.y1]), "faceScore": p.face_score, "landmarks": p.landmarks.len(), "landmarkScore": p.landmark_score, "areas": areas, "planes": planes, "mesh": mesh})
        })
        .collect();
    std::fs::write(
        out.join("people.json"),
        serde_json::to_string_pretty(&serde_json::json!({"set": parsing.model_set, "version": parsing.model_version, "people": people}))?,
    )?;
    Ok(())
}

fn save(path: &std::path::Path, w: usize, h: usize, rgb: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
    let bytes: Vec<u8> = rgb.iter().map(|v| v.clamp(0.0, 255.0) as u8).collect();
    image::RgbImage::from_raw(w as u32, h as u32, bytes).ok_or("bad size")?.save(path)?;
    Ok(())
}
