//! Find People / Find Wires: candidates for the Remove tool.
//!
//! Detection runs on its own thread with a row in the activity stack (cancellable), never on the session thread: a
//! render of the photo, then SAM 3's "person" mask (or, without the model, the face detector's boxes) or the thin-line
//! detector. The candidates wait in [`Finds`] for the Remove panel (or an agent) to select them and remove them with
//! content-aware spots in one undo step (`spot.removeCandidates`). Nothing is added to the photo until then.
//!
//! In the desktop app the commands return at once and the panel polls [`Finds::view`]; in the CLI, MCP and tests (and
//! with `wait`) they wait for the result, like AI masks do (`Segmenter::background`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::SegMask;
use lightcraft_pipeline::find::{Candidate, Kind, people, wires};
use lightcraft_raster::Rgba8;

use crate::activity::Cancel;
use crate::cmd::face_detect;
use crate::{EngineError, Session};

/// Long edge of the render the detectors look at.
pub const LOOK_EDGE: usize = 1600;
/// The text prompt for people.
const PERSON_PROMPT: &str = "person";

/// One candidate in the list.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u32,
    pub candidate: Candidate,
    pub selected: bool,
}

/// How a finished search ended, for one toast.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Counts up with each finished search, so a poller shows each once.
    pub seq: u64,
    pub kind: Kind,
    /// Candidates proposed.
    pub found: usize,
    /// What to tell the person (a sentence), or why it failed.
    pub text: String,
    pub error: bool,
}

#[derive(Default)]
struct State {
    photo: Option<PhotoId>,
    entries: Arc<Vec<Entry>>,
    next_id: u32,
    running: Vec<(Kind, Arc<AtomicBool>)>,
    last: Option<Outcome>,
    generation: u64,
}

/// What a poller sees.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// Changes whenever anything below does.
    pub generation: u64,
    pub entries: Arc<Vec<Entry>>,
    pub running: Vec<Kind>,
    pub last: Option<Outcome>,
}

/// The candidates of the active photo and the searches that make them.
#[derive(Clone, Default)]
pub struct Finds(Arc<Mutex<State>>);

impl Finds {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The candidates for `photo` (none when they were found on another photo).
    pub fn view(&self, photo: Option<PhotoId>) -> View {
        let s = self.lock();
        let mine = photo.is_some() && s.photo == photo;
        View {
            generation: s.generation,
            entries: if mine { s.entries.clone() } else { Arc::default() },
            running: s.running.iter().map(|r| r.0).collect(),
            last: if mine { s.last.clone() } else { None },
        }
    }

    /// Forget every candidate (the searches running now still finish into the list).
    pub fn clear(&self) {
        let mut s = self.lock();
        s.entries = Arc::default();
        s.generation += 1;
    }

    /// Cancel the searches running now.
    pub fn cancel(&self) {
        for (_, c) in &self.lock().running {
            c.store(true, Ordering::SeqCst);
        }
    }

    fn begin(&self, photo: PhotoId, kind: Kind) -> Result<Arc<AtomicBool>, String> {
        let mut s = self.lock();
        if s.running.iter().any(|r| r.0 == kind) {
            return Err(format!("Find {} is already running", if kind == Kind::Person { "People" } else { "Wires" }));
        }
        if s.photo != Some(photo) {
            s.photo = Some(photo);
            s.entries = Arc::default();
            s.last = None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        s.running.push((kind, cancel.clone()));
        s.generation += 1;
        Ok(cancel)
    }

    /// A search ended: replace the candidates of its kind (unless it was cancelled or the list moved to another photo).
    fn finish(&self, photo: PhotoId, kind: Kind, cancel: &Arc<AtomicBool>, result: Result<(Vec<Candidate>, String), String>) {
        let mut s = self.lock();
        s.running.retain(|r| !Arc::ptr_eq(&r.1, cancel));
        s.generation += 1;
        if s.photo != Some(photo) || cancel.load(Ordering::SeqCst) {
            return;
        }
        let seq = s.last.as_ref().map_or(1, |o| o.seq + 1);
        match result {
            Ok((found, text)) => {
                let mut entries: Vec<Entry> = s.entries.iter().filter(|e| e.candidate.kind != kind).cloned().collect();
                let n = found.len();
                for candidate in found {
                    s.next_id += 1;
                    entries.push(Entry { id: s.next_id, candidate, selected: false });
                }
                s.entries = Arc::new(entries);
                s.last = Some(Outcome { seq, kind, found: n, text, error: false });
            }
            Err(text) => s.last = Some(Outcome { seq, kind, found: 0, text, error: true }),
        }
    }

    /// Select (or deselect) candidate `id`; with `only`, every other one is deselected. `false`: no such candidate.
    pub fn select(&self, photo: Option<PhotoId>, id: Option<u32>, selected: bool, only: bool) -> bool {
        let mut s = self.lock();
        if s.photo != photo || photo.is_none() {
            return false;
        }
        if let Some(id) = id
            && !s.entries.iter().any(|e| e.id == id)
        {
            return false;
        }
        let entries: Vec<Entry> = s
            .entries
            .iter()
            .map(|e| {
                let mut e = e.clone();
                if Some(e.id) == id {
                    e.selected = selected;
                } else if only {
                    e.selected = false;
                }
                e
            })
            .collect();
        s.entries = Arc::new(entries);
        s.generation += 1;
        true
    }

    /// Take `ids` (every id when `None`; the selected ones when `selected_only`) out of the list.
    pub fn take(&self, photo: Option<PhotoId>, ids: Option<&[u32]>, selected_only: bool) -> Vec<Entry> {
        let mut s = self.lock();
        if s.photo != photo || photo.is_none() {
            return Vec::new();
        }
        let wanted = |e: &Entry| match ids {
            Some(list) => list.contains(&e.id),
            None => !selected_only || e.selected,
        };
        let (taken, kept): (Vec<Entry>, Vec<Entry>) = s.entries.iter().cloned().partition(|e| wanted(e));
        if !taken.is_empty() {
            s.entries = Arc::new(kept);
            s.generation += 1;
        }
        taken
    }

    /// Put entries `take` took back, as they were.
    pub fn restore(&self, photo: Option<PhotoId>, taken: Vec<Entry>) {
        let mut s = self.lock();
        if s.photo != photo || photo.is_none() {
            return;
        }
        let mut entries: Vec<Entry> = s.entries.iter().cloned().chain(taken).collect();
        entries.sort_by_key(|e| e.id);
        s.entries = Arc::new(entries);
        s.generation += 1;
    }

    /// Put candidates found by something else in the list (tests).
    #[cfg(test)]
    pub(crate) fn inject(&self, photo: PhotoId, found: Vec<Candidate>) {
        let mut s = self.lock();
        s.photo = Some(photo);
        let mut entries: Vec<Entry> = s.entries.iter().cloned().collect();
        for candidate in found {
            s.next_id += 1;
            entries.push(Entry { id: s.next_id, candidate, selected: false });
        }
        s.entries = Arc::new(entries);
        s.generation += 1;
    }
}

/// What to look for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Job {
    /// People; `include_main` also proposes the main subject (marked).
    People { include_main: bool },
    /// Wires; sensitivity 0..100.
    Wires { sensitivity: f32 },
}

impl Job {
    pub(crate) fn kind(self) -> Kind {
        match self {
            Job::People { .. } => Kind::Person,
            Job::Wires { .. } => Kind::Wire,
        }
    }
}

/// The ways to find people, as far as this machine has them.
pub(crate) struct PeopleSources {
    /// SAM 3's answer, to wait for.
    pub sam: Option<Box<dyn FnOnce(&AtomicBool) -> Result<Option<SegMask>, String> + Send>>,
    pub faces: Option<Arc<lightcraft_faces::yunet::Detector>>,
}

/// Run a search on a rendered photo: the candidates and a sentence about them.
pub(crate) fn find(job: Job, img: &Rgba8, sources: PeopleSources, cancel: &AtomicBool) -> Result<(Vec<Candidate>, String), String> {
    match job {
        Job::Wires { sensitivity } => {
            let found = wires::detect(img, sensitivity);
            let text = match found.len() {
                0 => "No wires found".to_string(),
                1 => "Found 1 wire".to_string(),
                n => format!("Found {n} wires"),
            };
            Ok((found, text))
        }
        Job::People { include_main } => {
            let PeopleSources { sam, faces } = sources;
            let mut note = None;
            let mut found = None;
            if let Some(wait) = sam {
                match wait(cancel) {
                    Ok(Some(seg)) => match seg.logits() {
                        Some(l) => found = Some(people::from_logits(seg.side as usize, &l, img.width, img.height)),
                        None => note = Some("the mask was damaged".to_string()),
                    },
                    Ok(None) => found = Some(Vec::new()),
                    Err(e) if cancel.load(Ordering::SeqCst) => return Err(e),
                    Err(e) => {
                        log::warn!("Find People: SAM 3 failed: {e}");
                        note = Some(e);
                    }
                }
            }
            if found.is_none() {
                let Some(detector) = faces else {
                    return Err(note.unwrap_or_else(|| "Finding people needs the SAM 3 model or the face detector".into()));
                };
                let rgb: Vec<u8> = img.data.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
                let faces = detector.detect(&rgb, img.width, img.height, &lightcraft_faces::yunet::Options::default()).map_err(|e| e.to_string())?;
                let boxes: Vec<lightcraft_pipeline::blemish::FaceBox> = faces
                    .iter()
                    .map(|f| lightcraft_pipeline::blemish::FaceBox {
                        x0: f64::from(f.x0),
                        y0: f64::from(f.y0),
                        x1: f64::from(f.x1),
                        y1: f64::from(f.y1),
                        landmarks: f.landmarks.map(|(x, y)| (f64::from(x), f64::from(y))),
                    })
                    .collect();
                found = Some(people::from_faces(&boxes, img.width, img.height));
                note = Some(
                    note.map_or_else(|| "rough shapes from faces (no SAM 3)".to_string(), |e| format!("SAM 3 failed ({e}); rough shapes from faces")),
                );
            }
            let all = found.unwrap_or_default();
            let held = all.iter().filter(|c| c.main).count();
            let mut found: Vec<Candidate> = if include_main { all } else { all.into_iter().filter(|c| !c.main).collect() };
            found.sort_by(|a, b| b.area.total_cmp(&a.area));
            let mut text = match found.len() {
                0 if held > 0 => "Only the main subject is in the photo".to_string(),
                0 => "No people found".to_string(),
                1 => "Found 1 person".to_string(),
                n => format!("Found {n} people"),
            };
            if held > 0 && !found.is_empty() && !include_main {
                text.push_str(", not counting the main subject");
            }
            if let Some(n) = note {
                text.push_str(&format!(" ({n})"));
            }
            Ok((found, text))
        }
    }
}

/// What a started search tells its caller.
pub(crate) struct Started {
    /// The search ran to its end before this returned (it waited).
    pub finished: bool,
}

impl Session {
    /// Start a search on the active photo. Waits for it when `wait` (or when the app is not in background mode).
    pub(crate) fn start_find(&mut self, c: &str, job: Job, wait: bool) -> crate::Result<Started> {
        let bad = |why: &str| crate::cmd::bad(c, why);
        let id = self.active().ok_or_else(|| bad("no active photo"))?;
        let kind = job.kind();
        // everything that needs the session happens here; the thread gets plain data
        let sources = match job {
            Job::Wires { .. } => PeopleSources { sam: None, faces: None },
            Job::People { .. } => {
                let sam_ready = self.segmenter.require_model();
                let faces = face_detect::detector(self);
                if let (Err(sam), Err(f)) = (&sam_ready, &faces) {
                    // neither: say how to get the better one (the model), and the fallback
                    return Err(match (sam, f) {
                        (EngineError::ModelMissing { model, msg }, _) => EngineError::ModelMissing {
                            model: *model,
                            msg: format!("{msg} Or download the YuNet face detector (Settings > Faces) for rough shapes from faces."),
                        },
                        (other, _) => EngineError::Other(other.to_string()),
                    });
                }
                let sam: Option<Box<dyn FnOnce(&AtomicBool) -> Result<Option<SegMask>, String> + Send>> = if sam_ready.is_ok() {
                    match self.segment_text_request(id, PERSON_PROMPT) {
                        Ok(req) => Some(Box::new(move |cancel| req.wait(cancel))),
                        Err(e) => {
                            log::warn!("Find People: {e}");
                            None
                        }
                    }
                } else {
                    None
                };
                PeopleSources { sam, faces: faces.ok() }
            }
        };
        let render = self.render_job(id, LOOK_EDGE, LOOK_EDGE, false, false).ok_or_else(|| bad("no photo"))?;
        let cancel = self.finds.begin(id, kind).map_err(|e| bad(&e))?;
        let (label_kind, label) = match kind {
            Kind::Person => ("findPeople", "Finding people"),
            Kind::Wire => ("findWires", "Finding wires"),
        };
        let guard = self.activity.start(label_kind, label, Cancel::Flag(cancel.clone()));
        let finds = self.finds.clone();
        let failed_cancel = cancel.clone();
        let wait = wait || !self.segmenter.background;
        let work = move || {
            let _guard = guard;
            #[cfg(test)]
            test_gate();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let img = render.run().rendered.map_err(|e| format!("couldn't render the photo: {e}"))?.image;
                if cancel.load(Ordering::SeqCst) {
                    return Err("cancelled".to_string());
                }
                find(job, &img, sources, &cancel)
            }))
            .unwrap_or_else(|_| Err("finding failed unexpectedly (the photo is untouched)".to_string()));
            finds.finish(id, kind, &cancel, result);
        };
        let name = if kind == Kind::Person { "find-people" } else { "find-wires" };
        match std::thread::Builder::new().name(name.into()).spawn(work) {
            Ok(handle) => {
                if wait {
                    let _ = handle.join();
                }
                Ok(Started { finished: wait })
            }
            Err(e) => {
                let why = format!("could not start the search: {e}");
                self.finds.finish(id, kind, &failed_cancel, Err(why.clone()));
                Err(bad(&why))
            }
        }
    }
}

/// Test-only: a search waits here, after its row is up, until the test lets it go.
#[cfg(test)]
pub(crate) static GATE: Mutex<Option<std::sync::mpsc::Receiver<()>>> = Mutex::new(None);

#[cfg(test)]
fn test_gate() {
    let rx = GATE.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(rx) = rx {
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
    }
}
