//! Parse a photo with the SegFormer set on candle and on Core ML (issue #105, `fork/coreml.md`), and print how long
//! each took and how well their part masks agree.
//!
//! `cargo run -p lightcraft-parsing --release --example coreml_compare -- in.jpg [--models DIR] [--edge 2048]`
//!
//! Three runs, each loading the set and then parsing the photo twice (the first parse includes Core ML's first call
//! per input size): candle as the app runs it (faces in batches of 4), candle one face at a time (the reference: on
//! Metal, candle 0.9.2 gives the faces after the first in a batch other numbers), and Core ML. Faces come from YuNet
//! in the shared models folder when it is there, as in the app. Agreement of Core ML with the reference is per part,
//! over every person, on the masks as the app draws them (`render::refined`, at the parsed picture's size, on at 0.5):
//! the share of pixels both call the same, and the intersection over union of the part's pixels.

use std::path::PathBuf;
use std::time::Instant;

use lightcraft_coreml::Runtime;
use lightcraft_parsing::render::{self, Select};
use lightcraft_parsing::{FaceHint, ModelSet, NormRect, Options, Parser, Parsing, Part, RgbImage, Runtimes, best_device};

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

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Load the set on `runtime` and parse twice: (parse, load ms, first parse ms, second parse ms).
fn timed(
    models: &std::path::Path,
    runtime: Runtime,
    batch: usize,
    img: &RgbImage,
    hints: &[FaceHint],
) -> Result<(Parsing, f64, f64, f64), Box<dyn std::error::Error>> {
    let opts = Options { batch, ..Options::default() };
    let t = Instant::now();
    let parser = Parser::load_with(ModelSet::Segformer, models, best_device(), Runtimes { face: runtime, body: runtime })?;
    let load = ms(t);
    let t = Instant::now();
    parser.parse(img, hints, &opts, &|| false)?;
    let first = ms(t);
    let t = Instant::now();
    let parsing = parser.parse(img, hints, &opts, &|| false)?;
    let second = ms(t);
    println!(
        "{runtime:?} (faces per call {batch}): backends {:?}, load {load:.0} ms, parse {first:.0} ms then {second:.0} ms, {} people",
        parser.backends(),
        parsing.people.len()
    );
    Ok((parsing, load, first, second))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else { return Err("usage: coreml_compare in.jpg [--models DIR] [--edge 2048]".into()) };
    let models = arg(&args, "--models")
        .map(PathBuf::from)
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
    let hints = yunet(&img).unwrap_or_default();
    println!("{}: {}×{}, {} faces (YuNet)", input, img.width, img.height, hints.len());

    timed(&models, Runtime::Candle, Options::default().batch, &img, &hints)?;
    let (a, ..) = timed(&models, Runtime::Candle, 1, &img, &hints)?;
    let (b, ..) = timed(&models, Runtime::CoreMl, Options::default().batch, &img, &hints)?;
    println!("| Part | Pixels agreeing | IoU |");
    println!("| --- | ---: | ---: |");
    for part in Part::ALL {
        let ma = render::refined(&a, Select::Everyone(part), &img);
        let mb = render::refined(&b, Select::Everyone(part), &img);
        let (mut same, mut both, mut either) = (0usize, 0usize, 0usize);
        for (x, y) in ma.iter().zip(&mb) {
            let (x, y) = (*x > 0.5, *y > 0.5);
            same += usize::from(x == y);
            both += usize::from(x && y);
            either += usize::from(x || y);
        }
        if either == 0 {
            continue;
        }
        let n = ma.len().max(1) as f64;
        println!("| {} | {:.2} % | {:.3} |", part.id(), same as f64 / n * 100.0, both as f64 / either as f64);
    }
    Ok(())
}
