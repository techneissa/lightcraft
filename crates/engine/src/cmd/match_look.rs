//! Fork feature (`fork/match-look.md`): Match Look, `develop.matchLook`. Gives the selected photos the colour and tone
//! of a reference picture (a library photo, an image file, a pasted image or an https address) as ordinary slider
//! values (`lightcraft_look`), one undo step for all of them.
//!
//! The fit runs on a worker thread with a row in the activity stack (✕ stops it). The app starts it with
//! `wait: false` and takes the result in with the per-frame `develop.matchLookPump`; scripts and MCP wait for it. A
//! reference from outside the library is never imported: a download is kept only in `<cache>/match-look`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_look::reference::{self, Source};
use lightcraft_look::{LookError, MatchOptions, MatchResult, RegionPair, Target};
use lightcraft_pipeline::SourceInfo;
use lightcraft_raster::{Rgb32f, Rgba8};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd};
use crate::activity::{Cancel, TaskGuard};
use crate::media::SourceLevel;
use crate::{Result, Session};

const CMD: &str = "develop.matchLook";
const LABEL: &str = "Match Look";
/// The most photos one match takes (each is a few hundred small renders).
const MAX_PHOTOS: usize = 500;
/// The long edge a library photo is rendered at to serve as the reference.
const REFERENCE_EDGE: usize = 512;

/// Match Look's state on the session: a pasted reference waiting to be used, the job in flight and the last result.
#[derive(Default)]
pub struct MatchLook {
    pasted: Option<Arc<Rgba8>>,
    job: Option<Job>,
    last: Option<Value>,
}

struct Job {
    rx: mpsc::Receiver<Done>,
    amount: f64,
}

/// What the worker sends back once: each photo's result.
struct Done {
    results: Vec<(PhotoId, std::result::Result<MatchResult, String>)>,
    cancelled: bool,
    seconds: f64,
}

/// Where the reference comes from, resolved on the session thread.
enum RefInput {
    /// A library photo: rendered with its own settings on the worker.
    Photo {
        src: Arc<Rgb32f>,
        info: Box<SourceInfo>,
        settings: Arc<DevelopSettings>,
    },
    Picture(Arc<Rgba8>),
    Load(Source),
}

struct TargetInput {
    id: PhotoId,
    src: Arc<Rgb32f>,
    info: SourceInfo,
    settings: Arc<DevelopSettings>,
}

impl Session {
    /// Hold a pasted picture as the next Match Look reference (`reference: {pasted: true}`); the app calls this when the
    /// user pastes an image in the Match Look dialog.
    pub fn set_match_look_pasted(&mut self, img: Rgba8) {
        self.match_look.pasted = (img.width > 0 && img.height > 0).then(|| Arc::new(img));
    }

    /// Whether a pasted picture is held for Match Look.
    pub fn match_look_has_pasted(&self) -> bool {
        self.match_look.pasted.is_some()
    }
}

fn options(p: &Value) -> MatchOptions {
    let mut o = MatchOptions::default();
    let flag = |k: &str, d: bool| p.get("options").and_then(|v| v.get(k)).and_then(Value::as_bool).unwrap_or(d);
    o.white_balance = flag("whiteBalance", o.white_balance);
    o.mixer = flag("mixer", o.mixer);
    o.grading = flag("grading", o.grading);
    o.curve = flag("curve", o.curve);
    o
}

fn reference_input(s: &mut Session, p: &Value) -> Result<RefInput> {
    let r = p.get("reference").ok_or_else(|| bad(CMD, "missing `reference`: {photo}, {path}, {url} or {pasted: true}"))?;
    if let Some(id) = r.get("photo") {
        let id = id.as_u64().map(PhotoId).ok_or_else(|| bad(CMD, "`reference.photo` must be a photo id"))?;
        let settings = s.develop_of(id).ok_or_else(|| bad(CMD, format!("no photo {}", id.0)))?;
        let src = s.source_now(id, SourceLevel::Thumb).map_err(|e| bad(CMD, format!("the reference photo could not be read: {e}")))?;
        return Ok(RefInput::Photo { src, info: Box::new(s.source_info(id)), settings });
    }
    if let Some(path) = r.get("path").and_then(Value::as_str) {
        return Ok(RefInput::Load(Source::File(path.into())));
    }
    if let Some(url) = r.get("url").and_then(Value::as_str) {
        #[cfg(not(target_arch = "wasm32"))]
        reference::check_url(url).map_err(|e| bad(CMD, e.to_string()))?;
        return Ok(RefInput::Load(Source::Url(url.to_string())));
    }
    if r.get("pasted").and_then(Value::as_bool) == Some(true) {
        return s.match_look.pasted.clone().map(RefInput::Picture).ok_or_else(|| bad(CMD, "no pasted picture"));
    }
    Err(bad(CMD, "`reference` needs `photo`, `path`, `url` or `pasted: true`"))
}

fn load_reference(r: RefInput, cancel: &AtomicBool) -> std::result::Result<Arc<Rgba8>, LookError> {
    match r {
        RefInput::Photo { src, info, settings } => Ok(Arc::new(lightcraft_look::render_reference(&src, &info, &settings, REFERENCE_EDGE))),
        RefInput::Picture(img) => Ok(img),
        RefInput::Load(src) => {
            let cache = crate::config::cache_dir().map(|c| c.join("match-look"));
            reference::load(&src, cache.as_deref(), cancel).map(Arc::new)
        }
    }
}

/// The match itself, on whatever thread calls it: the reference, then each photo in turn.
fn work(reference: RefInput, targets: Vec<TargetInput>, opts: MatchOptions, cancel: &AtomicBool, guard: &TaskGuard) -> Done {
    let started = Instant::now();
    let total = targets.len() as u64;
    let refimg = match crate::guard::catch("reading the Match Look reference", || load_reference(reference, cancel)) {
        Ok(Ok(img)) => img,
        Ok(Err(e)) => {
            return Done { cancelled: e == LookError::Cancelled, results: targets.iter().map(|t| (t.id, Err(e.to_string()))).collect(), seconds: 0.0 };
        }
        Err(e) => return Done { cancelled: false, results: targets.iter().map(|t| (t.id, Err(e.to_string()))).collect(), seconds: 0.0 },
    };
    let mut results = Vec::new();
    let mut cancelled = false;
    for (i, t) in targets.into_iter().enumerate() {
        if cancelled || cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        guard.progress(i as u64, total);
        let target = Target { src: &t.src, info: &t.info, settings: &t.settings };
        let r = crate::guard::catch("matching a look", || {
            lightcraft_look::match_look(&target, &refimg, &[RegionPair::whole()], &opts, cancel, &|_, _| {})
        });
        let r = match r {
            Ok(Ok(m)) => Ok(m),
            Ok(Err(LookError::Cancelled)) => {
                cancelled = true;
                continue;
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        results.push((t.id, r));
    }
    guard.progress(total, total);
    Done { results, cancelled, seconds: started.elapsed().as_secs_f64() }
}

/// Write the results onto the photos as they are now: one undo step. The summary the command and the pump return.
fn apply(s: &mut Session, done: Done, amount: f64) -> Result<Value> {
    let mut ops = Vec::new();
    let mut photos = Vec::new();
    let mut failed = Vec::new();
    for (id, r) in done.results {
        match r {
            Ok(m) => {
                let Some(now) = s.develop_of(id) else { continue };
                let m = m.scaled(amount);
                let mut d = (*now).clone();
                m.apply_to(&mut d, &s.source_info(id));
                photos.push(json!({
                    "id": id.0,
                    "changes": m.changes,
                    "curve": m.curve.as_ref().map(|c| c.iter().map(|p| [p.x, p.y]).collect::<Vec<_>>()),
                    "lossBefore": m.loss_before,
                    "lossAfter": m.loss_after,
                    "renders": m.evals,
                }));
                if d != *now
                    && let Some(op) = s.develop_op(id, d, LABEL)
                {
                    ops.push(op);
                }
            }
            Err(e) => failed.push(json!({"id": id.0, "error": e})),
        }
    }
    if !ops.is_empty() {
        s.commit(LABEL, Op::Batch { ops })?;
    }
    let v = json!({"photos": photos, "failed": failed, "cancelled": done.cancelled, "seconds": done.seconds});
    s.match_look.last = Some(v.clone());
    Ok(v)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    if s.match_look.job.is_some() {
        return Err(bad(CMD, "a Match Look is already running"));
    }
    let ids = s.targets(p).map_err(|e| bad(CMD, e.to_string()))?;
    if ids.is_empty() {
        return Err(bad(CMD, "no photo to match"));
    }
    if ids.len() > MAX_PHOTOS {
        return Err(bad(CMD, format!("at most {MAX_PHOTOS} photos at once")));
    }
    let amount = p.get("amount").and_then(Value::as_f64).unwrap_or(100.0);
    if !amount.is_finite() || !(0.0..=100.0).contains(&amount) {
        return Err(bad(CMD, "`amount` must be 0..100"));
    }
    let reference = reference_input(s, p)?;
    let mut targets = Vec::new();
    let mut unreadable = Vec::new();
    for chunk in ids.chunks(8) {
        for (id, r) in s.sources_now(chunk, SourceLevel::Thumb) {
            match (r, s.develop_of(id)) {
                (Ok((src, info)), Some(settings)) => targets.push(TargetInput { id, src, info, settings }),
                (Err(e), _) => unreadable.push((id, Err(e))),
                (_, None) => unreadable.push((id, Err("no such photo".to_string()))),
            }
        }
    }
    let opts = options(p);
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(true);
    let cancel = Arc::new(AtomicBool::new(false));
    let label = if ids.len() == 1 { LABEL.to_string() } else { format!("{LABEL}: {} photos", ids.len()) };
    let guard = s.activity.start("look", &label, Cancel::Flag(cancel.clone()));
    let finish = move |mut done: Done| {
        done.results.extend(unreadable);
        done
    };
    if wait {
        let done = finish(work(reference, targets, opts, &cancel, &guard));
        drop(guard);
        return apply(s, done, amount / 100.0);
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("match-look".into()).spawn(move || {
        let done = finish(work(reference, targets, opts, &cancel, &guard));
        drop(guard);
        let _ = tx.send(done);
    });
    spawned.map_err(|e| bad(CMD, format!("could not start the match: {e}")))?;
    s.match_look.job = Some(Job { rx, amount: amount / 100.0 });
    Ok(json!({"started": true, "total": ids.len()}))
}

/// Take in a finished background match (cheap to call every frame).
fn pump(s: &mut Session, _: &Value) -> Result<Value> {
    let Some(job) = s.match_look.job.as_ref() else {
        return Ok(json!({"active": false, "last": s.match_look.last}));
    };
    match job.rx.try_recv() {
        Err(mpsc::TryRecvError::Empty) => Ok(json!({"active": true})),
        Ok(done) => {
            let amount = job.amount;
            s.match_look.job = None;
            let last = apply(s, done, amount)?;
            Ok(json!({"active": false, "finished": true, "last": last}))
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            s.match_look.job = None;
            let last = json!({"photos": [], "failed": [], "cancelled": false, "error": "the match stopped unexpectedly"});
            s.match_look.last = Some(last.clone());
            Ok(json!({"active": false, "finished": true, "last": last}))
        }
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "develop.matchLook",
            "Match Look",
            [],
            None,
            "{reference: {photo: id} | {path: string} | {url: \"https://…\"} | {pasted: true}, ids?: [photo], amount?: 0..100 (100), options?: {whiteBalance?, mixer?, grading?, curve?: bool}, wait?: bool (true)} → {photos: [{id, changes: [{id, from, to}], curve?, lossBefore, lossAfter, renders}], failed: [{id, error}], cancelled, seconds} — give the photos (default: the selection) the colour and tone of the reference as slider values (white balance, Light, Vibrance/Saturation, Color Mixer, Color Grading, a master tone curve), one undo step. The reference is never imported; a web address must be https (at most 64 MB, kept in the cache). With `wait: false` it runs in the background (activity row) and `develop.matchLookPump` applies it",
            super::has_selection,
            run
        ),
        cmd!(
            query "develop.matchLookPump",
            "Match Look Progress",
            [],
            None,
            "{} → {active, finished?, last?} — cheap to call every frame: applies a finished background Match Look (one undo step)",
            super::always,
            pump
        ),
    ]
}
