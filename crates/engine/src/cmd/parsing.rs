//! Fork feature (`fork/people-parsing.md`): `parsing.*`, per-person part masks.
//!
//! - `parsing.status`: the model sets, which is chosen, what is downloaded, how downloads stand.
//! - `parsing.modelSet {set}`: choose the set that parses.
//! - `parsing.download {set?, confirm: true}` / `parsing.cancel`: get a set's models (asked first) / stop downloads
//!   and runs.
//! - `parsing.run {ids?, wait?}`: parse photos (from the cache when parsed before) → people, faces, landmarks and the
//!   area of each part.
//! - `parsing.mask {id?, person?, part, path, edge?}`: write one mask as a grey PNG.

use lightcraft_parsing::render::{self, Select};
use lightcraft_parsing::{FaceHint, ModelSet, NormRect, Parsing, Part, RgbImage};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection};
use crate::people_parsing::{Download, Outcome, PARSE_EDGE};
use crate::{Result, Session};

/// A detected face counts as a named face region of the photo when they overlap this much.
const SAME_FACE_IOU: f64 = 0.3;
/// Largest mask `parsing.mask` writes.
const MAX_MASK_EDGE: usize = 8192;

fn set_param(c: &str, p: &Value, s: &Session) -> Result<ModelSet> {
    match p.get("set").filter(|v| !v.is_null()) {
        None => Ok(s.parsing.model_set()),
        Some(v) => v.as_str().and_then(ModelSet::from_id).ok_or_else(|| bad(c, "`set` must be \"segformer\", \"bisenet\" or \"sapiens2\"")),
    }
}

fn download_json(d: Download) -> Value {
    let (state, bytes, total, error) = match d {
        Download::Idle => ("idle", 0, 0, None),
        Download::Running { bytes, total } => ("running", bytes, total, None),
        Download::Done => ("done", 0, 0, None),
        Download::Cancelled => ("cancelled", 0, 0, None),
        Download::Failed(e) => ("failed", 0, 0, Some(e)),
    };
    json!({"state": state, "bytes": bytes, "total": total, "error": error})
}

fn status(s: &Session) -> Value {
    let models = s.parsing.models_dir();
    let sets: Vec<Value> = ModelSet::ALL
        .iter()
        .map(|&set| {
            let files: Vec<Value> = set
                .files()
                .iter()
                .map(|f| json!({"label": f.label, "folder": f.folder, "name": f.name, "bytes": f.size, "licence": f.licence, "installed": models.as_deref().is_some_and(|m| f.installed_in(m))}))
                .collect();
            json!({
                "id": set.id(),
                "label": set.label(),
                "installed": s.parsing.installed(set),
                "bytes": set.size(),
                "missingBytes": models.as_deref().map_or(set.size(), |m| set.missing_size(m)),
                "version": set.version(),
                "files": files,
                "download": download_json(s.parsing.download_state(set)),
            })
        })
        .collect();
    json!({
        "modelSet": s.parsing.model_set().id(),
        "sets": sets,
        "modelsFolder": models.map(|p| p.display().to_string()),
        "cacheFolder": s.parsing.cache_dir().map(|p| p.display().to_string()),
        "busy": s.parsing.busy(),
        "message": s.parsing.message(),
    })
}

fn rect_json(r: &NormRect) -> Value {
    json!([r.x0, r.y0, r.x1, r.y1])
}

/// What a parse holds, without its pixels.
pub(crate) fn summary(p: &Parsing) -> Value {
    let people: Vec<Value> = p
        .people
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let parts: serde_json::Map<String, Value> =
                q.parts.keys().map(|k| (k.id().to_string(), json!((q.area(*k) * 1e5).round() / 1e5))).collect();
            json!({
                "index": i,
                "face": q.face.as_ref().map(rect_json),
                "faceScore": q.face_score,
                "personId": q.person_id,
                "landmarks": q.landmarks.len(),
                "landmarkScore": q.landmark_score,
                "bounds": q.bounds.as_ref().map(rect_json),
                "parts": parts,
            })
        })
        .collect();
    json!({"modelSet": p.model_set, "version": p.model_version, "width": p.width, "height": p.height, "people": people})
}

fn iou(a: &lightcraft_geom::Rect, b: &NormRect) -> f64 {
    let (bx0, by0, bx1, by1) = (f64::from(b.x0), f64::from(b.y0), f64::from(b.x1), f64::from(b.y1));
    let w = (a.x1.min(bx1) - a.x0.max(bx0)).max(0.0);
    let h = (a.y1.min(by1) - a.y0.max(by0)).max(0.0);
    let inter = w * h;
    let union = (a.x1 - a.x0) * (a.y1 - a.y0) + (bx1 - bx0) * (by1 - by0) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// The photo as the parse sees it (upright, uncropped, default settings) and its faces: YuNet's when it is
/// installed, each named after the face region of the photo it matches; none (the parse finds them) without it.
fn prepare(s: &mut Session, id: lightcraft_catalog::PhotoId) -> std::result::Result<(RgbImage, Vec<FaceHint>), String> {
    prepare_job(s, id)?.run()
}

/// [`prepare`] set up on the calling thread, to run elsewhere: portrait retouching finds a photo's faces from its
/// parse on a worker (`crate::retouch_input`), with the very picture and faces `parsing.run` uses, so both read one
/// cached parse.
pub(crate) struct Prepare {
    job: crate::RenderJob,
    detector: Option<std::sync::Arc<lightcraft_faces::yunet::Detector>>,
    regions: Vec<lightcraft_meta::Region>,
}

pub(crate) fn prepare_job(s: &mut Session, id: lightcraft_catalog::PhotoId) -> std::result::Result<Prepare, String> {
    let job = s.preview_job(id, PARSE_EDGE, PARSE_EDGE, false, &lightcraft_develop::DevelopSettings::default()).ok_or("no such photo")?;
    let detector = super::face_detect::detector(s).ok();
    let regions = s.catalog.photo(id).map(|p| p.meta.regions.clone()).unwrap_or_default();
    Ok(Prepare { job, detector, regions })
}

impl Prepare {
    /// Render the picture and find its faces (a model run: never on the UI thread).
    pub(crate) fn run(self) -> std::result::Result<(RgbImage, Vec<FaceHint>), String> {
        let image = self.job.run().rendered?.image;
        let (w, h) = (image.width, image.height);
        let rgb: Vec<u8> = image.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
        let img = RgbImage::new(w, h, rgb).map_err(|e| e.to_string())?;
        let Some(detector) = self.detector else { return Ok((img, Vec::new())) };
        let faces = detector.detect(&img.data, w, h, &lightcraft_faces::yunet::Options::default()).map_err(|e| e.to_string())?;
        let regions = self.regions;
        let hints = faces
            .into_iter()
            .map(|f| {
                let rect = NormRect::new(f.x0, f.y0, f.x1, f.y1);
                let person_id = regions
                    .iter()
                    .filter(|r| r.kind == lightcraft_meta::RegionKind::Face && r.name.is_some())
                    .map(|r| (iou(&r.rect, &rect), r))
                    .filter(|(o, _)| *o >= SAME_FACE_IOU)
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .and_then(|(_, r)| r.name.clone());
                FaceHint { rect, landmarks: Some(f.landmarks), score: f.score, person_id }
            })
            .collect();
        Ok((img, hints))
    }
}

/// Parse photo `id` (waiting for the run with `wait`).
pub(crate) fn parse_photo(s: &mut Session, id: lightcraft_catalog::PhotoId, wait: bool) -> Outcome {
    match prepare(s, id) {
        Ok((img, hints)) => s.parsing.parse(img, hints, wait),
        Err(e) => Outcome::Failed(e),
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let mut photos = Vec::new();
    for id in s.targets(p)? {
        photos.push(match parse_photo(s, id, wait) {
            Outcome::Ready { parsing, cached } => json!({"id": id.0, "state": "ready", "cached": cached, "parse": summary(&parsing)}),
            Outcome::Running => json!({"id": id.0, "state": "running"}),
            Outcome::Failed(e) => json!({"id": id.0, "state": "failed", "error": e}),
        });
    }
    Ok(json!({"modelSet": s.parsing.model_set().id(), "photos": photos}))
}

/// A mask of `parsing` with its long side `edge`, refined along the edges of the photo at that very size (the parse's
/// own picture when that is the size), so a draft and a full-size export draw the same outline.
pub(crate) fn mask_values(
    s: &mut Session,
    id: lightcraft_catalog::PhotoId,
    parsing: &Parsing,
    sel: Select,
    parsed: &RgbImage,
    edge: usize,
) -> std::result::Result<(Vec<f32>, usize, usize), String> {
    let edge = edge.clamp(1, MAX_MASK_EDGE);
    let guide = if edge == parsed.width.max(parsed.height) {
        parsed.clone()
    } else {
        let (rgb, w, h) = super::face_detect::render_rgb(s, id, edge)?;
        RgbImage::new(w, h, rgb).map_err(|e| e.to_string())?
    };
    Ok((render::refined(parsing, sel, &guide), guide.width, guide.height))
}

fn mask(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "parsing.mask";
    let part =
        p.get("part").and_then(Value::as_str).ok_or_else(|| bad(C, "`part` is required (a part id such as \"facialSkin\", or \"background\")"))?;
    let path = p.get("path").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or_else(|| bad(C, "`path` is required"))?;
    let person = p.get("person").and_then(Value::as_u64).map(|v| usize::try_from(v).unwrap_or(usize::MAX));
    let sel = match (part, person) {
        ("background", _) => Select::Background,
        (id, who) => {
            let part = Part::from_id(id).ok_or_else(|| bad(C, format!("unknown part `{id}`")))?;
            who.map_or(Select::Everyone(part), |k| Select::Person(k, part))
        }
    };
    let id = *s.targets(p)?.first().ok_or_else(|| bad(C, "no photo"))?;
    let (img, hints) = prepare(s, id).map_err(|e| bad(C, e))?;
    let parsing = match s.parsing.parse(img.clone(), hints, true) {
        Outcome::Ready { parsing, .. } => parsing,
        Outcome::Running => return Err(bad(C, "the parse is still running")),
        Outcome::Failed(e) => return Err(bad(C, e)),
    };
    if let Select::Person(k, _) = sel
        && k >= parsing.people.len()
    {
        return Err(bad(C, format!("the photo has {} people (person is counted from 0)", parsing.people.len())));
    }
    let edge = p.get("edge").and_then(Value::as_u64).map_or(img.width.max(img.height), |e| usize::try_from(e).unwrap_or(MAX_MASK_EDGE));
    let (values, w, h) = mask_values(s, id, &parsing, sel, &img, edge).map_err(|e| bad(C, e))?;
    render::save_png(std::path::Path::new(path), &values, w, h).map_err(|e| bad(C, e.to_string()))?;
    let covered = values.iter().sum::<f32>() / values.len().max(1) as f32;
    Ok(json!({"path": path, "width": w, "height": h, "coverage": covered}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "parsing.status",
            "People Parts Status",
            [],
            None,
            "{} → {modelSet, sets: [{id, label, installed, bytes, missingBytes, version, files: [{label, folder, name, bytes, licence, installed}], download: {state: idle|running|done|cancelled|failed, bytes, total, error}}], modelsFolder, cacheFolder, busy, message}: the people parsing model sets",
            always,
            |s, _| Ok(status(s))
        ),
        cmd!(
            "parsing.modelSet",
            "People Parts Model Set",
            [],
            None,
            "{set: \"segformer\"|\"bisenet\"|\"sapiens2\"} — the model set that finds people's parts from now on (parses made by the other set stay cached) → {modelSet, installed}",
            always,
            |s, p| {
                let set = p
                    .get("set")
                    .and_then(Value::as_str)
                    .and_then(ModelSet::from_id)
                    .ok_or_else(|| bad("parsing.modelSet", "`set` must be \"segformer\", \"bisenet\" or \"sapiens2\""))?;
                s.parsing.set_model_set(set).map_err(|e| bad("parsing.modelSet", e))?;
                Ok(json!({"modelSet": set.id(), "installed": s.parsing.installed(set)}))
            }
        ),
        cmd!(
            "parsing.download",
            "Download People Parts Models",
            [],
            None,
            "{set?: \"segformer\"|\"bisenet\"|\"sapiens2\" (default: the chosen set), confirm: true, choose?: bool} — downloads the set's missing models from huggingface.co into the folder shared with the other ArtCraft apps, on a thread with a row in the activity stack (✕ stops it; asking again resumes; sizes and SHA-256 are checked; one download at a time). With `choose`, the set parses from the moment the download has finished; a download that stops or fails changes nothing. Without `confirm: true` it is refused: ask the user first → {started: true, bytes}",
            always,
            |s, p| {
                const C: &str = "parsing.download";
                let set = set_param(C, p, s)?;
                if p.get("confirm").and_then(Value::as_bool) != Some(true) {
                    let missing = s.parsing.models_dir().map_or(set.size(), |m| set.missing_size(&m));
                    return Err(bad(
                        C,
                        format!(
                            "this downloads {} MB from huggingface.co: ask the user first, then send {{\"confirm\": true}}",
                            missing.div_ceil(1_000_000)
                        ),
                    ));
                }
                let choose = p.get("choose").and_then(Value::as_bool).unwrap_or(false);
                let bytes = s.parsing.start_download(set, choose).map_err(|e| bad(C, e))?;
                Ok(json!({"started": true, "set": set.id(), "bytes": bytes}))
            }
        ),
        cmd!(
            "parsing.cancel",
            "Stop People Parts",
            [],
            None,
            "{} — stops the people parsing downloads and runs going (what a download fetched stays; asking again resumes) → {}",
            always,
            |s, _| {
                s.parsing.cancel_downloads();
                s.parsing.cancel_runs();
                Ok(json!({}))
            }
        ),
        cmd!(
            "parsing.run",
            "Find People Parts",
            [],
            None,
            "{ids?, wait?: bool (default false)} — finds each person's parts (skin, hair, eyes, lips, teeth, clothes) and face landmarks in the photos, once per photo and model set (later calls read the cache), on a worker thread with a row in the activity stack. Faces come from YuNet when it is downloaded (named after the photo's face regions), else from the body model → {modelSet, photos: [{id, state: ready|running|failed, cached, parse: {people: [{index, face, faceScore, personId, landmarks, landmarkScore, bounds, parts: {part: area}}]}, error}]}",
            has_selection,
            run
        ),
        cmd!(
            query "parsing.mask",
            "Write People Part Mask",
            [],
            None,
            "{id?, part: \"entirePerson\"|\"facialSkin\"|\"bodySkin\"|\"hair\"|\"eyebrows\"|\"eyeWhites\"|\"iris\"|\"lips\"|\"teeth\"|\"clothesTop\"|\"clothesBottom\"|\"dress\"|\"shoes\"|\"hat\"|\"bag\"|\"others\"|\"background\", person?: index (default everyone), path, edge?: long side in pixels} — parses the photo if needed (waiting) and writes the mask as a grey PNG → {path, width, height, coverage}",
            has_selection,
            mask
        ),
    ]
}
