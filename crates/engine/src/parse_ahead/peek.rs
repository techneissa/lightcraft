//! The early check for faces (issue #167): when a photo opens, find out whether it has any face at all, so the
//! Portrait tool-strip icon is right before the tab is ever opened (`fork/retouch-foundation.md`).
//!
//! Only "nobody in this photo" is kept, in the face store under the key the face search keeps its result under
//! ([`FaceStore::put_nobody`]), so the Portrait tab finds it and searches no more. A photo with faces keeps no entry:
//! the face search (masks, mesh) still runs when the tab opens.
//!
//! Where the answer comes from, cheapest first:
//! 1. the parse the index beside the cache already knows for the photo (`super::Known`): its people are the faces; no
//!    model runs;
//! 2. else a small render of the photo and only the face detector (YuNet, or the installed provider): not the parse,
//!    not the face mesh.
//!
//! Run like parse-ahead: nothing when its setting is off, in Low Power Mode or on a low battery; one photo at a time
//! on a worker thread that waits (as parse-ahead's worker does) while the user drags, types or scrolls, while the
//! photo on screen is being made and while an export runs, before each step; stopped when the user moves to another
//! photo. A photo with face regions in the catalog is left to the face search.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use lightcraft_catalog::PhotoId;

use super::{Known, Pace, has_faces, parseable, worker};
use crate::Session;
use crate::activity::Activity;
use crate::people_parsing::PeopleParsing;
use crate::retouch_input::PeekDetect;

/// Long edge of the render the detector looks at (YuNet shrinks to its own 640 anyway).
pub const EDGE: usize = 640;
/// Keys given up on (a failure, a photo with faces) before the list is started again.
const TRIED_MAX: usize = 1024;
/// How often a held worker looks again.
const POLL: Duration = Duration::from_millis(40);

/// How a check ended (`Running::ended`).
const RUNNING: u8 = 0;
/// An answer, or none to be had: the photo is not checked again.
const FINISHED: u8 = 1;
/// The user moved on first: the photo is checked again when it is open again.
const DROPPED: u8 = 2;

struct Running {
    key: String,
    cancel: Arc<AtomicBool>,
    ended: Arc<AtomicU8>,
}

/// The early check in a session (`ParseAhead::peek`).
#[derive(Default)]
pub(crate) struct Peek {
    /// The photo last seen open.
    current: Option<PhotoId>,
    running: Option<Running>,
    /// Keys not to check again.
    tried: HashSet<String>,
    /// Set by the pump while the worker must wait.
    hold: Arc<AtomicBool>,
    /// Checks started (tests count them).
    started: usize,
}

impl Peek {
    /// Checks started since the app started.
    pub(crate) fn started(&self) -> usize {
        self.started
    }

    /// Drop the check going: its worker stops before its next step.
    fn stop(&mut self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Take in a check that ended.
    fn reap(&mut self) {
        let Some(r) = &self.running else { return };
        match r.ended.load(Ordering::SeqCst) {
            RUNNING => {}
            ended => {
                if ended == FINISHED {
                    if self.tried.len() >= TRIED_MAX {
                        self.tried.clear();
                    }
                    self.tried.insert(r.key.clone());
                }
                self.running = None;
            }
        }
    }
}

/// One pump of the early check: follow the open photo, and start its check when nothing holds it back.
pub(super) fn pump(s: &mut Session, pace: Pace) {
    let allowed = s.parse_ahead.enabled && s.parse_ahead.power.holds_back().is_none();
    let active = s.active();
    let peek = &mut s.parse_ahead.peek;
    if peek.current != active {
        peek.stop();
        peek.current = active;
    }
    peek.reap();
    if !allowed {
        peek.stop();
        return;
    }
    // (a check going waits too: its worker reads this)
    let held = pace == Pace::Pause || crate::prep::foreground_busy() || worker::exporting(&s.activity);
    peek.hold.store(held, Ordering::SeqCst);
    if held || peek.running.is_some() {
        return;
    }
    if let Some(id) = active {
        start(s, id);
    }
}

/// Start photo `id`'s check, when it needs one.
fn start(s: &mut Session, id: PhotoId) {
    let Some(p) = s.catalog.photo(id).cloned() else { return };
    if !parseable(&p) || has_faces(&p) {
        return;
    }
    let key = s.retouch_key(&p, &p.develop);
    if s.parse_ahead.peek.tried.contains(&key) || s.retouch_faces.knows(&key) {
        return;
    }
    // (the parse's people, when the index knows the parse: no model; read by the worker, never here)
    let version = s.parsing.ahead_version();
    let known =
        if s.parsing.can_parse() && s.parsing.cache_dir().is_some() { super::known(s, id, &version).filter(|k| k.key.is_some()) } else { None };
    let detect = s.retouch_peek_detector();
    if known.is_none() && detect.is_none() {
        s.parse_ahead.peek.tried.insert(key);
        return;
    }
    // (the render is set up here, only when the detector is to run)
    let job = match (&known, &detect) {
        (None, Some(_)) => s.retouch_detect_job_at(id, EDGE).ok(),
        _ => None,
    };
    if known.is_none() && job.is_none() {
        s.parse_ahead.peek.tried.insert(key);
        return;
    }
    if !s.retouch_faces.claim(&key, id) {
        return;
    }
    let peek = &mut s.parse_ahead.peek;
    let (cancel, ended) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU8::new(RUNNING)));
    let gate = Gate { hold: peek.hold.clone(), cancel: cancel.clone(), activity: s.activity.clone() };
    let task = Task { gate, parsing: s.parsing.clone(), known, detect: detect.zip(job) };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (done, store, k) = (ended.clone(), s.retouch_faces.clone(), key.clone());
        let spawned = std::thread::Builder::new().name("faces-peek".into()).spawn(move || {
            let end = catch_unwind(AssertUnwindSafe(|| work(task))).unwrap_or(Ended::Unknown);
            match end {
                Ended::Nobody => store.put_nobody(&k, id),
                _ => store.release(&k),
            }
            done.store(if end == Ended::Dropped { DROPPED } else { FINISHED }, Ordering::SeqCst);
        });
        if spawned.is_err() {
            s.retouch_faces.release(&key);
            s.parse_ahead.peek.tried.insert(key);
            return;
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (task, &ended);
        s.retouch_faces.release(&key);
        return;
    }
    let peek = &mut s.parse_ahead.peek;
    peek.started += 1;
    peek.running = Some(Running { key, cancel, ended });
}

/// What a worker needs, taken on the session thread.
struct Task {
    gate: Gate,
    parsing: PeopleParsing,
    known: Option<Known>,
    detect: Option<(PeekDetect, crate::RenderJob)>,
}

/// What holds a worker back, and what drops it.
struct Gate {
    hold: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    activity: Activity,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Ended {
    /// No face.
    Nobody,
    /// At least one face: the face search runs when the Portrait tab opens.
    Somebody,
    /// No answer (no parse, no detector, a failure).
    Unknown,
    /// The user moved on before the answer.
    Dropped,
}

impl Gate {
    /// Wait while the work is held back; `false`: the check was dropped meanwhile.
    fn wait(&self) -> bool {
        while self.hold.load(Ordering::SeqCst) || crate::prep::foreground_busy() || worker::exporting(&self.activity) {
            if self.cancel.load(Ordering::SeqCst) {
                return false;
            }
            std::thread::sleep(POLL);
        }
        !self.cancel.load(Ordering::SeqCst)
    }
}

fn work(t: Task) -> Ended {
    let Task { gate, parsing, known, detect } = t;
    if !gate.wait() {
        return Ended::Dropped;
    }
    if let Some(parse) = known.as_ref().and_then(|k| k.read(&parsing)) {
        return if parse.people.is_empty() { Ended::Nobody } else { Ended::Somebody };
    }
    let Some((detector, job)) = detect else { return Ended::Unknown };
    // (the render can't be stopped once it runs: each step starts only while nothing holds the work back)
    let img = match job.run().rendered {
        Ok(r) => r.image,
        Err(e) => {
            log::info!("faces peek: {e}");
            return Ended::Unknown;
        }
    };
    if !gate.wait() {
        return Ended::Dropped;
    }
    match detector.count(&img) {
        Ok(0) => Ended::Nobody,
        Ok(_) => Ended::Somebody,
        Err(e) => {
            log::info!("faces peek: {e}");
            Ended::Unknown
        }
    }
}
