//! Fork feature (`fork/ai-culling.md`): `photo.analyze` split so the app measures on worker threads under an activity
//! row (stoppable), as Find Similar does: [`plan`] lists what is to be read (no file is read), [`measure`] reads it
//! without the session, [`apply`] files the scores (one undo step) and answers. The command itself (the CLI, MCP
//! without an app) does all three at once.
//!
//! Besides focus, clipping and bursts, each photo's faces are read for closed eyes and eyes out of focus
//! ([`crate::cull_eyes`]). Eyes already measured are kept (`eyes: "again"` measures them again); a photo whose eyes
//! could not be read (no detector or mesh yet) is tried again next time. A burst's best frame is the sharpest whose
//! eyes are open and in focus. Photos with closed or soft eyes are listed as Reject suggestions; nothing is rejected
//! for them unless the user says so.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use lightcraft_catalog::{Analysis, Eyes, Flag, Op, PhotoId};
use serde_json::{Value, json};

use crate::cull_eyes::{EyeJob, PhotoEyes, Tools};
use crate::media::{SourceLevel, SourceRef};
use crate::{Result, Session};

/// The kind of the row in the activity stack a run on a worker thread holds.
pub const KIND: &str = "cull";
/// What a closed-eyes frame loses against an open one when a burst's best is chosen (more than any focus score).
const CLOSED_PENALTY: f32 = 100.0;
/// What a frame with soft eyes loses.
const SOFT_EYES_PENALTY: f32 = 30.0;

/// Whether a run can start (one at a time).
pub fn no_run_going(s: &Session) -> std::result::Result<(), String> {
    if s.activity.list().iter().any(|r| r.kind == KIND) { Err("Assisted Culling is already running".to_string()) } else { Ok(()) }
}

/// How the eyes are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EyesMode {
    Off,
    /// Keep eyes measured before.
    Keep,
    Again,
}

/// What a run asks.
#[derive(Clone, Copy, Debug)]
struct Params {
    reject_below: Option<f32>,
    pick_best: bool,
    gap: f64,
    same: f32,
}

enum EyeWork {
    Skip,
    Kept(Eyes),
    Measure(Box<EyeJob>),
}

struct Item {
    id: PhotoId,
    time: Option<i64>,
    thumb: SourceRef,
    eyes: EyeWork,
}

/// What a run has to read.
pub struct Plan {
    params: Params,
    items: Vec<Item>,
    tools: Option<Tools>,
    failed: Vec<(PhotoId, String)>,
}

impl Plan {
    /// How many photos are to be read.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// One photo as measured.
struct Row {
    id: PhotoId,
    time: Option<i64>,
    sharp: f32,
    clip: f32,
    sig: [f32; 64],
    eyes: Option<Eyes>,
    /// Measured now (not kept from before).
    fresh: Option<PhotoEyes>,
    eye_error: Option<String>,
}

/// What [`measure`] read.
pub struct Measured {
    params: Params,
    rows: Vec<Row>,
    failed: Vec<(PhotoId, String)>,
    /// Why no eyes could be read for photos without a parse (no detector or mesh), once for the run.
    eyes_missing: Option<String>,
    /// The engine the face mesh ran on, when it ran.
    mesh_engine: Option<&'static str>,
    /// The run was stopped: nothing is filed.
    pub stopped: bool,
}

fn secs(iso: &str) -> Option<i64> {
    let d = lightcraft_meta::DateTime::parse_iso(iso)?;
    Some(d.unix_seconds())
}

fn eyes_mode(p: &Value) -> Result<EyesMode> {
    match p.get("eyes") {
        None | Some(Value::Null) | Some(Value::Bool(true)) => Ok(EyesMode::Keep),
        Some(Value::Bool(false)) => Ok(EyesMode::Off),
        Some(Value::String(s)) if s == "again" => Ok(EyesMode::Again),
        Some(_) => Err(super::bad(super::cull::C, "`eyes` must be true, false or \"again\"")),
    }
}

/// The run `p` asks for: which photos, and for each what to read. No file is read.
pub fn plan(s: &mut Session, p: &Value) -> Result<Plan> {
    let ids: Vec<PhotoId> = if p.get("ids").is_some() || s.selection.ids.len() > 1 { s.targets(p)? } else { s.visible_cloned() };
    // a photo listed twice is one photo, not a burst of two look-alikes (first occurrence kept)
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<PhotoId> = ids.into_iter().filter(|id| seen.insert(*id)).collect();
    let reject_below = p.get("rejectBelow").and_then(Value::as_f64).map(|v| v as f32);
    let pick_best = p.get("pickBest").and_then(Value::as_bool).unwrap_or(false);
    let (gap, same) = super::cull::burst_params(p)?;
    let mode = eyes_mode(p)?;
    let mut items = Vec::with_capacity(ids.len());
    let mut failed = Vec::new();
    for id in ids {
        let Some(photo) = s.catalog.photo(id) else {
            failed.push((id, "no such photo".to_string()));
            continue;
        };
        let time = photo.captured.as_deref().and_then(secs);
        let kept = photo.analysis.and_then(|a| a.eyes);
        let thumb = s.media.source_ref(photo, SourceLevel::Thumb);
        let eyes = match (mode, kept) {
            // (not measuring keeps what was measured before)
            (EyesMode::Off | EyesMode::Keep, Some(e)) => EyeWork::Kept(e),
            (EyesMode::Off, None) => EyeWork::Skip,
            _ => crate::cull_eyes::job(s, id).map_or(EyeWork::Skip, |j| EyeWork::Measure(Box::new(j))),
        };
        items.push(Item { id, time, thumb, eyes });
    }
    let tools = (mode != EyesMode::Off).then(|| Tools::new(s));
    Ok(Plan { params: Params { reject_below, pick_best, gap, same }, items, tools, failed })
}

/// Read one photo: its thumbnail's scores, then its eyes.
fn read_one(item: Item, tools: Option<&Tools>, cancel: &AtomicBool) -> std::result::Result<Row, String> {
    let scores = crate::guard::catch("Assisted Culling", || {
        crate::memory::in_background(|| item.thumb.load_source()).map(|d| {
            use lightcraft_pipeline::cull;
            (cull::sharpness(&d.image), cull::clipped(&d.image), cull::signature(&d.image))
        })
    })??;
    let (sharp, clip, sig) = scores;
    let mut row = Row { id: item.id, time: item.time, sharp, clip, sig, eyes: None, fresh: None, eye_error: None };
    match (item.eyes, tools) {
        (EyeWork::Kept(e), _) => row.eyes = Some(e),
        (EyeWork::Measure(job), Some(tools)) if !cancel.load(Ordering::Relaxed) => match tools.measure(*job, cancel) {
            Ok(found) => {
                row.eyes = Some(found.summary);
                row.fresh = Some(found);
            }
            Err(e) => row.eye_error = Some(e),
        },
        _ => {}
    }
    Ok(row)
}

/// Read what [`plan`] listed, several photos at once (the models run for one photo at a time). Doesn't touch the
/// session. `progress(done)` is called from the reading threads; once `cancel` is set no further photo is started.
pub fn measure(plan: Plan, cancel: &AtomicBool, progress: &(dyn Fn(usize) + Sync)) -> Measured {
    let Plan { params, items, tools, failed } = plan;
    let eyes_missing = tools.as_ref().and_then(Tools::missing);
    let rows = Mutex::new(Vec::with_capacity(items.len()));
    let failed = Mutex::new(failed);
    let done = AtomicUsize::new(0);
    let lanes = crate::export::export_parallelism(items.len()).min(4);
    let queue = Mutex::new(items.into_iter());
    let work = || {
        while !cancel.load(Ordering::Relaxed) {
            let next = queue.lock().unwrap_or_else(PoisonError::into_inner).next();
            let Some(item) = next else { break };
            let id = item.id;
            match read_one(item, tools.as_ref(), cancel) {
                Ok(row) => rows.lock().unwrap_or_else(PoisonError::into_inner).push(row),
                Err(e) => failed.lock().unwrap_or_else(PoisonError::into_inner).push((id, e)),
            }
            progress(done.fetch_add(1, Ordering::Relaxed).saturating_add(1));
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::scope(|sc| {
        for k in 1..lanes.max(1) {
            // a lane that can't start leaves its photos to the others (and to this thread)
            if std::thread::Builder::new().name(format!("cull-{k}")).spawn_scoped(sc, work).is_err() {
                break;
            }
        }
        work();
    });
    #[cfg(target_arch = "wasm32")]
    {
        let _ = lanes;
        work();
    }
    Measured {
        params,
        rows: rows.into_inner().unwrap_or_else(PoisonError::into_inner),
        failed: failed.into_inner().unwrap_or_else(PoisonError::into_inner),
        eyes_missing,
        mesh_engine: tools.as_ref().and_then(Tools::mesh_engine),
        stopped: cancel.load(Ordering::Relaxed),
    }
}

/// How good a frame is when a burst's best is chosen: its focus, less [`CLOSED_PENALTY`] for closed eyes and
/// [`SOFT_EYES_PENALTY`] for soft ones.
fn frame_score(r: &Row) -> f32 {
    let e = r.eyes.unwrap_or_default();
    let closed = if e.closed == Some(true) { CLOSED_PENALTY } else { 0.0 };
    let soft = if e.out_of_focus() == Some(true) { SOFT_EYES_PENALTY } else { 0.0 };
    r.sharp - closed - soft
}

fn eyes_json(e: &Eyes) -> Value {
    json!({"faces": e.faces, "main": e.main, "closed": e.closed, "focus": e.focus, "outOfFocus": e.out_of_focus()})
}

fn faces_json(found: &PhotoEyes) -> Value {
    let faces: Vec<Value> = found
        .faces
        .iter()
        .map(|f| {
            let round = |v: f32| (v * 1000.0).round() / 1000.0;
            json!({
                "rect": [round(f.rect.x0), round(f.rect.y0), round(f.rect.x1), round(f.rect.y1)],
                "main": f.eyes.main,
                "closed": f.eyes.closed,
                "focus": f.eyes.focus.map(|v| (v * 10.0).round() / 10.0),
                "mesh": round(f.mesh_score),
                "eyes": f.eyes.eyes.iter().map(|e| json!({"open": e.ear.map(round)})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!(faces)
}

/// File what [`measure`] read (one undo step) and answer. Photos gone from the library meanwhile are left out.
pub fn apply(s: &mut Session, m: Measured) -> Result<Value> {
    let Measured { params, mut rows, failed, eyes_missing, mesh_engine, stopped } = m;
    if stopped {
        return Err(super::bad(super::cull::C, "stopped: nothing was changed"));
    }
    rows.retain(|r| s.catalog.photo(r.id).is_some());
    // bursts: consecutive (by capture time) look-alike shots
    rows.sort_by_key(|r| (r.time.unwrap_or(i64::MAX), r.id));
    let mut group_of: Vec<Option<u32>> = vec![None; rows.len()];
    let mut next = 1u32;
    for i in 1..rows.len() {
        let (a, b) = (&rows[i - 1], &rows[i]);
        let close = matches!((a.time, b.time), (Some(x), Some(y)) if (y.saturating_sub(x)).unsigned_abs() as f64 <= params.gap);
        if close && lightcraft_pipeline::cull::similarity(&a.sig, &b.sig) >= params.same {
            let g = *group_of[i - 1].get_or_insert_with(|| {
                next += 1;
                next - 1
            });
            group_of[i] = Some(g);
        }
    }
    let mut best: std::collections::HashMap<u32, (usize, f32)> = Default::default();
    for (i, g) in group_of.iter().enumerate() {
        if let Some(g) = g {
            let e = best.entry(*g).or_insert((i, f32::NEG_INFINITY));
            let score = frame_score(&rows[i]);
            if score > e.1 {
                *e = (i, score);
            }
        }
    }
    let mut ops = Vec::new();
    let (mut rejected, mut picked) = (0, 0);
    let mut out = Vec::new();
    let mut suggest = Vec::new();
    let (mut measured, mut from_parse, mut kept, mut eye_failed) = (0, 0, 0, 0);
    for (i, r) in rows.iter().enumerate() {
        let g = group_of[i];
        let is_best = g.is_some_and(|g| best.get(&g).is_some_and(|b| b.0 == i));
        let analysis = Analysis { sharpness: r.sharp, clipped: r.clip, group: g, best: is_best, eyes: r.eyes };
        ops.push(Op::SetAnalysis { id: r.id, analysis: Some(analysis) });
        let flag = s.catalog.photo(r.id).map(|p| p.flag).unwrap_or_default();
        if params.reject_below.is_some_and(|t| r.sharp < t) && flag != Flag::Reject {
            ops.push(Op::SetFlag { id: r.id, flag: Flag::Reject });
            rejected += 1;
        } else if params.pick_best && is_best && flag == Flag::None {
            ops.push(Op::SetFlag { id: r.id, flag: Flag::Pick });
            picked += 1;
        }
        let mut why = Vec::new();
        if let Some(e) = &r.eyes {
            if e.closed == Some(true) {
                why.push("eyesClosed");
            }
            if e.out_of_focus() == Some(true) {
                why.push("eyesOutOfFocus");
            }
        }
        let rejected_now = params.reject_below.is_some_and(|t| r.sharp < t);
        if !why.is_empty() && flag != Flag::Reject && !rejected_now {
            suggest.push(json!({"id": r.id.0, "why": why}));
        }
        match (&r.fresh, &r.eyes, &r.eye_error) {
            (Some(f), _, _) if f.from_parse => from_parse += 1,
            (Some(_), _, _) => measured += 1,
            (None, Some(_), _) => kept += 1,
            (None, None, Some(_)) => eye_failed += 1,
            _ => {}
        }
        let mut row = json!({"id": r.id.0, "sharpness": (r.sharp * 10.0).round() / 10.0, "clipped": r.clip, "group": g, "best": is_best});
        if let Some(e) = &r.eyes {
            row["eyes"] = eyes_json(e);
        }
        if let Some(f) = &r.fresh {
            row["faces"] = faces_json(f);
        }
        if let Some(e) = &r.eye_error {
            row["eyesError"] = json!(e);
        }
        out.push(row);
    }
    if !ops.is_empty() {
        s.commit("Assisted Culling", Op::Batch { ops })?;
    }
    let failed: Vec<Value> = failed.into_iter().map(|(id, e)| json!([id.0, e])).collect();
    Ok(json!({
        "photos": out,
        "groups": best.len(),
        "rejected": rejected,
        "picked": picked,
        "failed": failed,
        "suggestReject": suggest,
        "eyes": {"measured": measured, "fromParse": from_parse, "kept": kept, "failed": eye_failed, "unavailable": eyes_missing, "meshEngine": mesh_engine},
    }))
}

/// Reject the photos the user chose from a run's suggestions (one undo step); the ones already rejected, or gone,
/// are left as they are. How many were rejected.
pub fn reject(s: &mut Session, ids: &[PhotoId]) -> Result<usize> {
    let ops: Vec<Op> = ids
        .iter()
        .filter(|id| s.catalog.photo(**id).is_some_and(|p| p.flag != Flag::Reject))
        .map(|id| Op::SetFlag { id: *id, flag: Flag::Reject })
        .collect();
    let n = ops.len();
    if n > 0 {
        s.commit("Reject Suggested Photos", Op::Batch { ops })?;
    }
    Ok(n)
}

/// `photo.analyze` at once (the CLI, MCP without an app, tests).
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let plan = plan(s, p)?;
    let measured = measure(plan, &AtomicBool::new(false), &|_| {});
    apply(s, measured)
}

#[cfg(test)]
mod tests;
