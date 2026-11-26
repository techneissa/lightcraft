//! The masks `cargo xtask mask-score` scores (fork, issue #256, `fork/mask-score.md`): each photo's AI mask made by the
//! app's own commands (`mask.add`, `mask.objectPoint`, the zoomed-in `mask.refineDetail` pass the app runs after a
//! selection), rendered as the app draws it and written as a grey picture.
//!
//! `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example mask_score -- JOB METHOD ITEMS.json OUT_DIR`
//!
//! `JOB`: `subject`, `hair`, `sky` or `click`. `METHOD`: for `subject` and `hair` the Subject mask of BiRefNet with the
//! hair edge `birefnet` (Current: BiRefNet alone), `matting` (BiRefNet Matting) or `vision` (Apple Vision); for `sky`
//! and `click`, `sam3`. `ITEMS.json`: `[{id, photo, truth, ignore?}]`, paths of pictures; the truth is read for `click`
//! only (it places the clicks).
//!
//! Writes `OUT_DIR/<id>.png` (`click`: `<id>@1.png`, `@3`, `@5`, the mask after that many clicks) and
//! `OUT_DIR/predict.json`: the engine each model ran on, and per photo the seconds, whether anything was found and any
//! error. A photo where the model finds nothing gets an empty mask, as in the app; any other failure is an `error` and
//! no mask.
use std::path::{Path, PathBuf};
use std::time::Instant;

use lightcraft_develop::MaskShape;
use lightcraft_engine::Session;
use lightcraft_engine::catalog::PhotoId;
use lightcraft_maskscore::{Mask, next_click};
use lightcraft_pipeline::{MaskView, Overlay};
use serde_json::{Value, json};

/// The click counts whose masks are kept.
const CLICKS_SCORED: [usize; 3] = [1, 3, 5];

struct Item {
    id: String,
    photo: PathBuf,
    truth: PathBuf,
    ignore: Option<PathBuf>,
}

#[derive(Default)]
struct Made {
    /// Seconds of each model request (one per mask, or one per click).
    seconds: Vec<f64>,
    found: bool,
    model: Option<String>,
    clicks: Vec<Value>,
}

fn items(path: &Path) -> Result<Vec<Item>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let list: Vec<Value> = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    list.iter()
        .map(|v| {
            let text = |k: &str| v[k].as_str().ok_or_else(|| format!("item without `{k}`: {v}"));
            Ok(Item {
                id: text("id")?.to_string(),
                photo: text("photo")?.into(),
                truth: text("truth")?.into(),
                ignore: v["ignore"].as_str().map(PathBuf::from),
            })
        })
        .collect()
}

fn grey(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.into_luma8();
    Ok((img.width() as usize, img.height() as usize, img.into_raw()))
}

/// The mask `mask_id` as the app draws it, at the photo's size.
fn render(s: &mut Session, photo: PhotoId, mask_id: u32, w: usize, h: usize) -> Result<Vec<u8>, String> {
    let job = s.render_job(photo, w, h, false, true).ok_or("no render job")?.with_overlay(Overlay::Mask {
        id: u16::try_from(mask_id).map_err(|_| "mask id")?,
        view: MaskView::WhiteOnBlack,
        color: [255, 255, 255],
        opacity: 100,
    });
    let img = job.run().rendered.map_err(|e| format!("render: {e}"))?.image;
    if (img.width, img.height) != (w, h) {
        return Err(format!("rendered {}×{}, the photo is {w}×{h}", img.width, img.height));
    }
    Ok(img.data.iter().map(|p| p[0]).collect())
}

fn save(path: &Path, w: usize, h: usize, grey: Vec<u8>) -> Result<(), String> {
    let img = image::GrayImage::from_raw(w as u32, h as u32, grey).ok_or("size")?;
    img.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn execute(s: &mut Session, command: &str, params: Value) -> Result<Value, String> {
    s.execute(command, &params).map_err(|e| e.to_string())
}

/// The app's message for a model that found nothing (`cmd::concepts::not_found`).
fn nothing_found(error: &str) -> bool {
    error.contains("was found in this photo")
}

fn first_mask(s: &Session, photo: PhotoId) -> Result<(u32, Option<String>), String> {
    let d = s.develop_of(photo).ok_or("no settings")?;
    let m = d.masks.first().ok_or("no mask was added")?;
    let model = match m.components.first().map(|c| &c.shape) {
        Some(MaskShape::Subject { seg, .. } | MaskShape::Sky { seg, .. } | MaskShape::Object { seg, .. }) => {
            let seg = seg.as_ref().ok_or("the mask has no selection: the app would draw its rough stand-in")?;
            Some(format!("{}² {}", seg.side, seg.model.as_deref().unwrap_or("sam3")))
        }
        _ => None,
    };
    Ok((m.id, model))
}

/// The zoomed-in pass the app runs once a selection settles (SAM 3's only; a BiRefNet part has none).
fn detail(s: &mut Session, mask: u32) -> Result<(), String> {
    execute(s, "mask.refineDetail", json!({"id": mask})).map(|_| ())
}

fn concept(s: &mut Session, photo: PhotoId, kind: &str, w: usize, h: usize, out: &Path, id: &str) -> Result<Made, String> {
    let t = Instant::now();
    let added = execute(s, "mask.add", json!({"kind": kind}));
    if let Err(e) = &added {
        if !nothing_found(e) {
            return Err(e.clone());
        }
        save(&out.join(format!("{id}.png")), w, h, vec![0; w * h])?;
        return Ok(Made { seconds: vec![t.elapsed().as_secs_f64()], ..Made::default() });
    }
    let (mask, model) = first_mask(s, photo)?;
    detail(s, mask)?;
    let seconds = t.elapsed().as_secs_f64();
    save(&out.join(format!("{id}.png")), w, h, render(s, photo, mask, w, h)?)?;
    Ok(Made { seconds: vec![seconds], found: true, model, ..Made::default() })
}

fn clicks(s: &mut Session, photo: PhotoId, item: &Item, w: usize, h: usize, out: &Path) -> Result<Made, String> {
    let (tw, th, truth) = grey(&item.truth)?;
    let truth = Mask::from_grey(tw, th, &truth).ok_or("truth size")?;
    if (tw, th) != (w, h) {
        return Err(format!("the truth is {tw}×{th}, the photo {w}×{h}"));
    }
    let ignore: Option<Vec<bool>> = match &item.ignore {
        Some(p) => Some(grey(p)?.2.iter().map(|&v| v >= 128).collect()),
        None => None,
    };
    let mut made = Made { found: true, ..Made::default() };
    let mut current = vec![0u8; w * h];
    let mut mask = None;
    for n in 1..=CLICKS_SCORED[CLICKS_SCORED.len() - 1] {
        let selected = Mask::from_grey(w, h, &current).ok_or("mask size")?;
        // (nothing wrong anywhere: no more clicks, the mask stays)
        if let Some(c) = next_click(&selected, &truth, ignore.as_deref()) {
            let (x, y) = ((c.x as f64 + 0.5) / w as f64, (c.y as f64 + 0.5) / h as f64);
            let t = Instant::now();
            let id = match mask {
                None => {
                    // (the first click of an Object selection is an include click)
                    execute(s, "mask.add", json!({"kind": "object", "points": [[x, y]]}))?;
                    let (id, model) = first_mask(s, photo)?;
                    made.model = model;
                    id
                }
                Some(id) => {
                    execute(s, "mask.objectPoint", json!({"id": id, "x": x, "y": y, "exclude": !c.include}))?;
                    id
                }
            };
            mask = Some(id);
            detail(s, id)?;
            made.seconds.push(t.elapsed().as_secs_f64());
            made.clicks.push(json!({"x": c.x, "y": c.y, "include": c.include}));
            current = render(s, photo, id, w, h)?;
        }
        if CLICKS_SCORED.contains(&n) {
            save(&out.join(format!("{}@{n}.png", item.id)), w, h, current.clone())?;
        }
    }
    Ok(made)
}

fn one(s: &mut Session, job: &str, item: &Item, out: &Path) -> Result<Made, String> {
    let path = std::fs::canonicalize(&item.photo).map_err(|e| format!("{}: {e}", item.photo.display()))?;
    let r = execute(s, "library.import", json!({"paths": [path.to_string_lossy()]}))?;
    let photo = r["imported"].get(0).and_then(Value::as_u64).map(PhotoId).ok_or_else(|| format!("not imported: {r}"))?;
    execute(s, "library.select", json!({"ids": [photo.0]}))?;
    let (w, h) = s.catalog.photo(photo).map(|p| (p.width as usize, p.height as usize)).ok_or("no photo")?;
    if w == 0 || h == 0 {
        return Err("the photo has no size".into());
    }
    let made = match job {
        "subject" | "hair" => concept(s, photo, "subject", w, h, out, &item.id),
        "sky" => concept(s, photo, "sky", w, h, out, &item.id),
        _ => clicks(s, photo, item, w, h, out),
    };
    let _ = s.execute("mask.deleteAll", &json!({}));
    made
}

fn session(job: &str, method: &str) -> Result<Session, String> {
    let mut s = Session::new().with_fs().with_default_sam3_model().with_default_parsing_models();
    s.segmenter.background = false;
    match (job, method) {
        ("subject" | "hair", "birefnet" | "matting" | "vision") => {
            execute(&mut s, "segment.selectionModel", json!({"model": "birefnet"}))?;
            let edge = match method {
                "matting" => "birefnet-matting",
                "vision" => "apple-vision",
                _ => "current",
            };
            execute(&mut s, "maskEdges.edge", json!({"kind": "subject", "edge": edge}))?;
        }
        ("sky" | "click", "sam3") => {}
        _ => return Err(format!("no method `{method}` for job `{job}`")),
    }
    Ok(s)
}

fn run(args: &[String]) -> Result<usize, String> {
    let [job, method, list, out] = args else { return Err("usage: mask_score JOB METHOD ITEMS.json OUT_DIR".into()) };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let items = items(Path::new(list))?;
    let mut s = session(job, method)?;
    let mut rows = Vec::new();
    let mut failed = 0;
    for (k, item) in items.iter().enumerate() {
        let row = match one(&mut s, job, item, &out) {
            Ok(m) => json!({"id": item.id, "seconds": m.seconds, "found": m.found, "model": m.model, "clicks": m.clicks}),
            Err(e) => {
                failed += 1;
                eprintln!("{job}/{method} {}: {e}", item.id);
                json!({"id": item.id, "error": e})
            }
        };
        rows.push(row);
        if (k + 1) % 10 == 0 || k + 1 == items.len() {
            println!("{job}/{method}: {}/{}", k + 1, items.len());
        }
    }
    let report = json!({
        "job": job,
        "method": method,
        "backends": {"birefnet": s.segmenter.birefnet_backend(), "sam3": s.segmenter.sam3_backend()},
        "items": rows,
    });
    let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(out.join("predict.json"), text).map_err(|e| e.to_string())?;
    Ok(failed)
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(0) => std::process::ExitCode::SUCCESS,
        Ok(n) => {
            eprintln!("{n} photo(s) failed");
            std::process::ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
