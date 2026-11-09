//! Apply to Every Photo of a person: a portrait preset (or one face's settings) to the person's face on every photo
//! where they are named or recognised, as one undo step.
//!
//! The photos' faces and settings are read on the UI thread (the catalog's regions and the face index: no pixels);
//! the matching and the new settings are worked out on a worker with an activity row when more than a handful of
//! photos have faces ([`HANDFUL`]); the result is committed as one batch, which the catalog rolls back whole when it
//! fails. A photo whose settings changed meanwhile is worked out again from its new settings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::presets::Preset;
use lightcraft_develop::retouch::{Face, KeyFrame, KeyMap};
use lightcraft_pipeline::retouch_step::FrameKeyMap;
use serde_json::{Value, json};

use super::{Matcher, Miss, Portrait, Target, Who, faces_of, set_face};
use crate::activity::{Cancel, TaskGuard, TaskHandle};
use crate::{EngineError, Result, Session};

/// Most photos with faces worked out on the UI thread; more go to a worker.
pub const HANDFUL: usize = 24;

/// What to give to whom.
pub struct Request {
    pub who: Who,
    /// The face settings ([`super::carry`]).
    pub portrait: Portrait,
    /// The preset's other develop settings, when it has them.
    pub rest: Option<Preset>,
    /// A photo to leave out (the one the face's settings came from).
    pub exclude: Option<PhotoId>,
    /// The undo step's name.
    pub label: String,
}

/// One photo with faces, as read on the UI thread.
struct Candidate {
    id: PhotoId,
    faces: Vec<Face>,
    embeddings: Vec<Option<Arc<[f32]>>>,
    develop: Arc<DevelopSettings>,
    /// Only for a photo with a face's own settings kept after lens corrections and transform.
    keys: Option<FrameKeyMap>,
}

/// One photo the person was found on.
struct Reached {
    id: PhotoId,
    /// The settings it was worked out from, and its faces with the person's.
    from: Arc<DevelopSettings>,
    target: Target,
    /// Its new settings (`None`: they would not change).
    next: Option<DevelopSettings>,
}

/// What the matching found.
pub(crate) struct Plan {
    reached: Vec<Reached>,
    failed: Vec<(PhotoId, Miss)>,
}

/// A run on the worker, with its activity row.
pub(crate) struct Job {
    rx: Receiver<Option<Plan>>,
    cancel: Arc<AtomicBool>,
    request: Arc<Request>,
    _row: TaskGuard,
}

fn new_settings(r: &Request, c_develop: &DevelopSettings, target: &Target, keys: Option<&dyn KeyMap>) -> Option<Option<DevelopSettings>> {
    let mut d = match &r.rest {
        Some(rest) => rest.apply_on(c_develop, c_develop, 1.0),
        None => c_develop.clone(),
    };
    if !set_face(&mut d, target, &r.portrait.features, 1.0, keys) {
        return None;
    }
    Some((d != *c_develop).then_some(d))
}

/// Find the person on each candidate and work out its new settings. `None` when cancelled.
fn plan(cands: Vec<Candidate>, m: &Matcher, r: &Request, cancel: &AtomicBool, row: Option<&TaskHandle>) -> Option<Plan> {
    let total = cands.len() as u64;
    let mut out = Plan { reached: Vec::new(), failed: Vec::new() };
    for (n, c) in cands.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(row) = row
            && n % 64 == 0
        {
            row.progress(n as u64, total);
        }
        let Ok(index) = m.pick(&c.faces, &c.embeddings) else { continue };
        let target = Target { faces: c.faces, index };
        match new_settings(r, &c.develop, &target, c.keys.as_ref().map(|k| k as &dyn KeyMap)) {
            Some(next) => out.reached.push(Reached { id: c.id, from: c.develop, target, next }),
            None => out.failed.push((c.id, Miss::Full)),
        }
    }
    Some(out)
}

impl Session {
    /// Every in-library photo with faces but `exclude`.
    fn portrait_candidates(&self, exclude: Option<PhotoId>) -> Vec<Candidate> {
        self.catalog
            .photos()
            .filter(|p| p.in_library() && Some(p.id) != exclude)
            .filter_map(|p| {
                let (faces, embeddings) = faces_of(self, p);
                if faces.is_empty() {
                    return None;
                }
                let keys = p.develop.retouch.faces.iter().any(|o| o.face.frame == KeyFrame::Edit).then(|| self.retouch_key_map(p.id)).flatten();
                Some(Candidate { id: p.id, faces, embeddings, develop: p.develop.clone(), keys })
            })
            .collect()
    }

    /// Give `r` to every photo of its person: at once for a handful of photos with faces (or with `wait`), else on a
    /// worker (`{state: "running"}`; [`Session::portrait_pump`] finishes it).
    pub fn portrait_apply_to_person(&mut self, r: Request, wait: bool) -> Result<Value> {
        if self.portrait_presets.job.is_some() {
            return Err(EngineError::Other("portrait settings are already being applied: wait for that to finish".into()));
        }
        let m = Matcher::new(self, &r.who, false);
        let cands = self.portrait_candidates(r.exclude);
        let photos = cands.len();
        let r = Arc::new(r);
        if wait || photos <= HANDFUL {
            let none = AtomicBool::new(false);
            let p = plan(cands, &m, &r, &none, None).unwrap_or(Plan { reached: Vec::new(), failed: Vec::new() });
            return self.portrait_commit(p, &r);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let row = self.activity.start("portrait", "Applying portrait settings", Cancel::Flag(cancel.clone()));
        row.progress(0, photos as u64);
        let handle = row.handle();
        let (tx, rx) = std::sync::mpsc::channel();
        let (flag, req) = (cancel.clone(), r.clone());
        let spawned = std::thread::Builder::new().name("portrait-apply".into()).spawn(move || {
            let _ = tx.send(plan(cands, &m, &req, &flag, Some(&handle)));
        });
        if let Err(e) = spawned {
            return Err(EngineError::Other(format!("could not start applying portrait settings: {e}")));
        }
        self.portrait_presets.job = Some(Job { rx, cancel, request: r, _row: row });
        Ok(json!({"state": "running", "photos": photos}))
    }

    /// Whether Apply to Every Photo is running in the background.
    pub fn portrait_job_running(&self) -> bool {
        self.portrait_presets.job.is_some()
    }

    /// Stop Apply to Every Photo in the background: nothing is changed.
    pub fn portrait_cancel(&self) {
        if let Some(j) = &self.portrait_presets.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Finish Apply to Every Photo once its worker is done (cheap to call every frame): `None` while it runs or when
    /// none runs; else what happened, committed as one undo step.
    pub fn portrait_pump(&mut self) -> Option<Result<Value>> {
        let job = self.portrait_presets.job.as_ref()?;
        let got = match job.rx.try_recv() {
            Err(TryRecvError::Empty) => return None,
            Ok(p) => Ok(p),
            Err(TryRecvError::Disconnected) => Err(()),
        };
        let job = self.portrait_presets.job.take()?;
        Some(match got {
            Ok(Some(p)) => self.portrait_commit(p, &job.request),
            Ok(None) => Ok(json!({"state": "cancelled", "person": job.request.who.person, "applied": [], "count": 0, "failed": []})),
            Err(()) => Err(EngineError::Other("applying portrait settings stopped unexpectedly: nothing was changed".into())),
        })
    }

    /// Commit `p` as one undo step. A photo whose settings changed since they were read is worked out again.
    fn portrait_commit(&mut self, p: Plan, r: &Request) -> Result<Value> {
        let mut failed: Vec<(PhotoId, Miss)> = p.failed;
        let (mut ops, mut applied) = (Vec::new(), Vec::new());
        for x in p.reached {
            let Some(now) = self.develop_of(x.id) else { continue };
            let next = if Arc::ptr_eq(&now, &x.from) {
                x.next
            } else {
                let keys = self.retouch_key_map(x.id);
                match new_settings(r, &now, &x.target, keys.as_ref().map(|k| k as &dyn KeyMap)) {
                    Some(next) => next,
                    None => {
                        failed.push((x.id, Miss::Full));
                        continue;
                    }
                }
            };
            if let Some(next) = next
                && let Some(op) = self.develop_op(x.id, next, &r.label)
            {
                ops.push(op);
            }
            applied.push(x.id.0);
        }
        let changed = ops.len();
        if !ops.is_empty() {
            // one batch: one undo step, rolled back whole when it fails
            self.commit(&r.label, Op::Batch { ops }).map_err(|e| EngineError::Other(format!("nothing was changed: {e}")))?;
        }
        let failed: Vec<Value> = failed.iter().map(|(id, why)| json!({"id": id.0, "why": why.as_str()})).collect();
        Ok(json!({"state": "done", "person": r.who.person, "applied": applied, "count": applied.len(), "changed": changed, "failed": failed}))
    }
}
