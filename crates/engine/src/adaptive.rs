//! Fork feature (`fork/adaptive-masks.md`, issue #239): AI mask parts found again on each photo.
//!
//! A preset, Paste, Sync or Auto Sync carries what an AI part selects (Subject, Sky, Background, the clicks of an
//! Object, a description, the parts and whose of a Select People mask), not the selection a model found on the first
//! photo: on another photo the part waits ([`lightcraft_develop::adaptive`]), drawing nothing, and is found there.
//!
//! After every command, the photos it changed that have waiting parts are found ([`Session::adaptive_after`]): at
//! once in the CLI, MCP and tests (the command waits), in the background in the desktop app
//! ([`crate::segment::Segmenter::background`]), one photo at a time, with one row in the ring for them all. The
//! models' own caches are used (the encoded photo on the SAM 3 worker, the people parse and its disk cache, which
//! parse-ahead fills). What is found goes into the photo's settings without an undo step of its own, so the command
//! that brought the parts stays one undo step, whose redo brings back what was found; a part that no longer waits,
//! or now selects something else, is left alone. A part found nothing for selects nothing; the batch counts it as
//! failed, with why. ✕ on the row stops the rest; their parts keep waiting until the photo is opened or Update AI
//! Masks runs.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::adaptive::Waiting;
use lightcraft_develop::{DevelopSettings, MaskShape, SegMask};
use lightcraft_geom::Orientation;

use crate::Session;
use crate::activity::{Cancel, TaskGuard};
use crate::segment::adaptive_jobs::{Ask, Replies};
use crate::select_people::Parsed;

/// Stands in for the models in tests: what an AI part other than Select People selects on a photo (`Ok(None)`:
/// nothing found). Background is turned around afterwards, as the model's subject is.
pub type Finder = Arc<dyn Fn(PhotoId, &MaskShape) -> Result<Option<SegMask>, String> + Send + Sync>;

/// What was found for one part: its selection (`None`: the classical way) and finer passes, and why nothing was.
#[derive(Clone, Debug)]
struct Found {
    seg: Option<SegMask>,
    detail: Vec<SegMask>,
    why: Option<String>,
    /// It selects nothing: a part that kept its selection for the search (Update AI Masks) gets that back.
    empty: bool,
}

impl Found {
    fn classical() -> Found {
        Found { seg: None, detail: Vec::new(), why: None, empty: false }
    }

    fn of(seg: SegMask, detail: Vec<SegMask>) -> Found {
        Found { seg: Some(seg), detail, why: None, empty: false }
    }

    fn nothing(why: String) -> Found {
        Found { seg: Some(SegMask::nothing()), detail: Vec::new(), why: Some(why), empty: true }
    }

    /// A Select People part that selects nothing (`why`: a failure, else the face is simply not there).
    fn no_people(why: Option<String>) -> Found {
        let seg = crate::select_people::grid::nothing("people");
        Found { seg: Some(seg), detail: Vec::new(), why, empty: true }
    }
}

enum State {
    Done(Found),
    /// Waiting for the photo's people.
    People,
    /// Waiting for the model's answer to request `job`.
    Model {
        job: u64,
        invert: bool,
    },
}

struct Part {
    waiting: Waiting,
    state: State,
}

/// The photo whose parts are being found.
struct Running {
    id: PhotoId,
    orientation: Orientation,
    parts: Vec<Part>,
    /// Stops its model requests.
    cancels: Vec<Arc<AtomicBool>>,
    /// Its parts changed while they were being found (a command reached it): it is done again afterwards.
    again: bool,
}

/// The photos waiting for their AI parts, and the one being done (`Session::adaptive`).
#[derive(Default)]
pub struct Adaptive {
    queue: VecDeque<PhotoId>,
    running: Option<Running>,
    batch: Option<TaskGuard>,
    total: u64,
    failed: u64,
    replies: Replies,
    finder: Option<Finder>,
}

impl Adaptive {
    /// Whether photos wait for their AI parts or one is being done.
    pub fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }

    /// Tests: `finder` stands in for SAM 3 and BiRefNet (Select People keeps the people parser's own test runner).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_finder(&mut self, finder: Option<Finder>) {
        self.finder = finder;
    }

    /// Whether a test's finder stands in for the models.
    pub(crate) fn has_finder(&self) -> bool {
        self.finder.is_some()
    }

    /// Stop: the running photo's requests, the photos waiting their turn, the row. A model's answer still on its way
    /// goes to a channel nobody reads any more, so it never lands on a photo (another library's may have the same id).
    pub(crate) fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            r.cancels.iter().for_each(|c| c.store(true, Ordering::Relaxed));
        }
        self.queue.clear();
        self.replies = Replies::default();
        if let Some(b) = self.batch.take() {
            b.reports_itself();
        }
        (self.total, self.failed) = (0, 0);
    }
}

/// What the model answered for a part.
fn answered(result: Result<Option<SegMask>, String>, invert: bool, shape: &MaskShape) -> Found {
    match result {
        Ok(Some(seg)) if invert => match seg.invert() {
            Some(inv) => Found::of(inv, Vec::new()),
            None => Found::nothing(not_found(shape)),
        },
        Ok(Some(seg)) => Found::of(seg, Vec::new()),
        Ok(None) => Found::nothing(not_found(shape)),
        Err(e) => Found::nothing(e),
    }
}

fn not_found(shape: &MaskShape) -> String {
    match shape {
        MaskShape::Object { .. } => "The object was not found in this photo.".into(),
        MaskShape::Prompt { text, .. } => format!("Nothing matching “{text}” was found in this photo."),
        _ => crate::cmd::concepts::of(shape).map_or_else(|| "Nothing was found in this photo.".into(), |c| crate::cmd::concepts::not_found(c.kind)),
    }
}

/// A Select People part drawn from a photo's people (`fork/select-people.md`): its face looked for there by name,
/// else by place; everyone's parts are everyone's. A face that is not there selects nothing (not a failure: the
/// picker says so).
fn people_found(people: Result<&Parsed, &String>, shape: &MaskShape, orient: Orientation) -> Found {
    use crate::select_people::grid::{self, Selection};
    let MaskShape::People { parts, face, .. } = shape else { return Found::classical() };
    let p = match people {
        Ok(p) => p,
        Err(e) => return Found::no_people(Some(e.clone())),
    };
    let picks = crate::select_people::parts::picks_of(parts);
    let person = match face {
        None => None,
        Some(k) => match crate::select_people::find_person(&p.parsing, k) {
            Some(i) => Some(i),
            None => return Found::no_people(None),
        },
    };
    match grid::build(&p.parsing, &Selection::of(person, &picks), orient) {
        Some((main, fine)) => Found::of(main, fine),
        None => Found::no_people(None),
    }
}

/// What [`Session::adaptive_fill`] did.
#[derive(Default)]
struct Filled {
    /// The photo's settings changed.
    changed: bool,
    /// Why parts selected nothing.
    failed: Vec<String>,
    /// A part waits for something other than what was found (its recipe changed, or the photo was turned).
    rejected: bool,
}

/// Every photo an op changes.
fn photos_of(ops: &[Op]) -> Vec<PhotoId> {
    let mut out = Vec::new();
    for op in ops {
        op.photo_ids(&mut |id| {
            if !out.contains(&id) {
                out.push(id);
            }
        });
    }
    out
}

impl Session {
    /// `synced`, the settings photo `x` gets from another photo's (`before`: its own until now; Paste, Sync, Auto
    /// Sync): each AI part keeps `x`'s own selection when `x` had the same part found, else waits to be found on `x`.
    pub(crate) fn adapted(&self, before: &DevelopSettings, mut synced: DevelopSettings) -> DevelopSettings {
        lightcraft_develop::adaptive::adapt(Some(before), &mut synced);
        synced
    }

    /// After a command: find the waiting AI parts of the photos its `ops` changed, and of the photo it opened.
    pub(crate) fn adaptive_after(&mut self, ops_from: usize, opened: Option<PhotoId>) {
        let mut ids = photos_of(self.pending_log.get(ops_from..).unwrap_or_default());
        ids.extend(opened.filter(|id| !ids.contains(id)));
        self.adaptive_find(&ids);
    }

    /// Of `ids`, the photos with AI parts still to be found; those are found (queued in the app, at once otherwise).
    /// Export asks first: a part still waiting would export as nothing.
    pub fn adaptive_find(&mut self, ids: &[PhotoId]) -> usize {
        let ids: Vec<PhotoId> =
            ids.iter().copied().filter(|id| self.develop_of(*id).is_some_and(|d| d.masks.iter().any(lightcraft_develop::Mask::is_waiting))).collect();
        let n = ids.len();
        if n == 0 {
            return 0;
        }
        if self.segmenter.background {
            self.adaptive_queue(ids);
            return n;
        }
        for id in ids {
            self.adaptive_now(id);
        }
        // (found now, or nothing found: none waits any more)
        0
    }

    /// Whether AI parts are being found on some photos.
    pub fn adaptive_busy(&self) -> bool {
        self.adaptive.busy()
    }

    fn adaptive_queue(&mut self, ids: Vec<PhotoId>) {
        let a = &mut self.adaptive;
        let mut added = 0;
        for id in ids {
            if let Some(r) = a.running.as_mut().filter(|r| r.id == id) {
                // (done again once this round ends: what is found now is for the parts as they were)
                r.again = true;
                continue;
            }
            if a.queue.contains(&id) {
                continue;
            }
            a.queue.push_back(id);
            added += 1;
        }
        if added == 0 {
            return;
        }
        a.total += added;
        let batch = a.batch.get_or_insert_with(|| self.activity.start_background_batch("adaptiveMasks", "Finding AI masks", Cancel::Yes));
        batch.progress(batch.done(), a.total);
    }

    /// Find photo `id`'s waiting parts now, waiting for the models.
    fn adaptive_now(&mut self, id: PhotoId) {
        let Some(d) = self.develop_of(id) else { return };
        let mut people: Option<Result<Arc<Parsed>, String>> = None;
        let mut found = Vec::new();
        for w in d.waiting() {
            let f = match &w.recipe {
                MaskShape::People { .. } => {
                    let p = people.get_or_insert_with(|| self.people_now(id));
                    people_found(p.as_deref(), &w.recipe, d.orientation)
                }
                shape => self.find_now(id, shape),
            };
            found.push((w, f));
        }
        let filled = self.adaptive_fill(id, d.orientation, found);
        if let Some(why) = filled.failed.first() {
            log::info!("AI masks found again: {why}");
        }
    }

    fn find_now(&mut self, id: PhotoId, shape: &MaskShape) -> Found {
        let invert = matches!(shape, MaskShape::Background { .. });
        if let Some(f) = self.adaptive.finder.clone() {
            return answered(f(id, shape), invert, shape);
        }
        match self.adaptive_ask(id, shape) {
            Ask::Classical => Found::classical(),
            Ask::Missing(why) => Found::nothing(why),
            Ask::Model { request, invert } => {
                let r = self.adaptive_wait(id, request);
                answered(r, invert, shape)
            }
        }
    }

    /// Put what was found into photo `id`'s settings, where its part still waits for it and selects the same; without
    /// an undo step (`apply_system`). A part that kept its selection for the search gets it back when nothing was
    /// found ([`SegMask::previous`]): a failed search never loses a selection the user had.
    fn adaptive_fill(&mut self, id: PhotoId, orientation: Orientation, found: Vec<(Waiting, Found)>) -> Filled {
        let mut out = Filled::default();
        let Some(p) = self.catalog.photo(id) else { return out };
        let (now, edited) = (p.develop.clone(), p.edited.clone());
        // (turned since: what was found is the other way round; the parts wait to be found again)
        if now.orientation != orientation {
            out.rejected = true;
            return out;
        }
        let mut d = (*now).clone();
        for (w, f) in found {
            let Some(c) = d.masks.iter_mut().find(|m| m.id == w.mask).and_then(|m| m.components.get_mut(w.component)) else { continue };
            if !c.shape.is_waiting() {
                continue;
            }
            // (it selects something else now: found again for that)
            if c.shape.recipe().as_ref() != Some(&w.recipe) {
                out.rejected = true;
                continue;
            }
            out.failed.extend(f.why);
            match c.shape.seg().and_then(SegMask::previous).filter(|_| f.empty) {
                Some(prev) => {
                    let detail = match &c.shape {
                        MaskShape::Subject { detail, .. }
                        | MaskShape::Sky { detail, .. }
                        | MaskShape::Background { detail, .. }
                        | MaskShape::Object { detail, .. }
                        | MaskShape::Prompt { detail, .. }
                        | MaskShape::People { detail, .. } => detail.clone(),
                        _ => Vec::new(),
                    };
                    c.shape.set_found(Some(prev), detail);
                }
                None => c.shape.set_found(f.seg, f.detail),
            }
            out.changed = true;
        }
        if out.changed
            && let Err(e) = self.apply_system(Op::SetDevelop { id, settings: Arc::new(d), label: "AI Masks".into(), edited })
        {
            log::warn!("AI masks found again: {e}");
            out.changed = false;
        }
        out
    }

    /// The desktop app's per-frame step (from [`Session::segment_poll`]): collect the answers for the photo being
    /// done, put them in when all are in, start the next photo. Whether a photo's masks changed.
    pub(crate) fn adaptive_poll(&mut self) -> bool {
        if !self.adaptive.busy() {
            return false;
        }
        if self.adaptive.batch.as_ref().is_some_and(TaskGuard::is_cancelled) {
            self.adaptive.stop();
            return false;
        }
        for (job, result) in self.adaptive.replies.drain() {
            let Some(r) = self.adaptive.running.as_mut() else { break };
            if let Some(part) = r.parts.iter_mut().find(|p| matches!(p.state, State::Model { job: j, .. } if j == job))
                && let State::Model { invert, .. } = part.state
            {
                part.state = State::Done(answered(result, invert, &part.waiting.recipe));
            }
        }
        self.adaptive_people();
        let mut changed = false;
        if self.adaptive.running.as_ref().is_some_and(|r| r.parts.iter().all(|p| matches!(p.state, State::Done(_))))
            && let Some(r) = self.adaptive.running.take()
        {
            changed = self.adaptive_finish(r);
        }
        while self.adaptive.running.is_none() {
            let Some(id) = self.adaptive.queue.pop_front() else { break };
            match self.adaptive_start(id) {
                Some(r) => self.adaptive.running = Some(r),
                // (nothing waits there any more: undone, or found meanwhile)
                None => {
                    if let Some(b) = &self.adaptive.batch {
                        b.item_done();
                    }
                }
            }
        }
        if !self.adaptive.busy()
            && let Some(batch) = self.adaptive.batch.take()
        {
            // (the ring says nothing when every photo went well)
            if self.adaptive.failed == 0 {
                batch.reports_itself();
            }
            (self.adaptive.total, self.adaptive.failed) = (0, 0);
        }
        changed
    }

    /// Put the photo's answers in (a command of its own, so it is saved and its sidecar written like an edit).
    fn adaptive_finish(&mut self, r: Running) -> bool {
        let found: Vec<(Waiting, Found)> = r
            .parts
            .into_iter()
            .filter_map(|p| match p.state {
                State::Done(f) => Some((p.waiting, f)),
                _ => None,
            })
            .collect();
        let (id, orientation, again) = (r.id, r.orientation, r.again);
        let mut filled = Filled::default();
        let _ = self.execute_fn("adaptive.fill", |s| {
            filled = s.adaptive_fill(id, orientation, found);
            Ok(serde_json::Value::Null)
        });
        let Filled { changed, failed, rejected } = filled;
        // its parts changed or it was turned meanwhile, and some still wait: found again as they are now
        if (again || rejected) && self.develop_of(id).is_some_and(|d| d.masks.iter().any(lightcraft_develop::Mask::is_waiting)) {
            if !self.adaptive.queue.contains(&id) {
                self.adaptive.queue.push_back(id);
            }
            return changed;
        }
        if let Some(b) = &self.adaptive.batch {
            match failed.first() {
                Some(why) => {
                    let name = self.catalog.photo(id).map(|p| p.file_name.clone()).unwrap_or_default();
                    b.item_failed(&format!("{name}: {why}"));
                    self.adaptive.failed += 1;
                }
                None => b.item_done(),
            }
        }
        changed
    }

    /// Start finding photo `id`'s waiting parts; `None` when none wait.
    fn adaptive_start(&mut self, id: PhotoId) -> Option<Running> {
        let d = self.develop_of(id)?;
        let waiting = d.waiting();
        if waiting.is_empty() {
            return None;
        }
        if let (Some(b), Some(p)) = (&self.adaptive.batch, self.catalog.photo(id)) {
            b.detail(&p.file_name);
        }
        let mut r = Running { id, orientation: d.orientation, parts: Vec::new(), cancels: Vec::new(), again: false };
        for w in waiting {
            let state = match &w.recipe {
                MaskShape::People { .. } => State::People,
                shape => match self.adaptive.finder.clone() {
                    Some(f) => State::Done(answered(f(id, shape), matches!(shape, MaskShape::Background { .. }), shape)),
                    None => match self.adaptive_ask(id, shape) {
                        Ask::Classical => State::Done(Found::classical()),
                        Ask::Missing(why) => State::Done(Found::nothing(why)),
                        Ask::Model { request, invert } => {
                            let replies = std::mem::take(&mut self.adaptive.replies);
                            let sent = self.adaptive_submit(id, request, &replies);
                            self.adaptive.replies = replies;
                            match sent {
                                Ok((job, cancel)) => {
                                    r.cancels.push(cancel);
                                    State::Model { job, invert }
                                }
                                Err(e) => State::Done(Found::nothing(e)),
                            }
                        }
                    },
                },
            };
            r.parts.push(Part { waiting: w, state });
        }
        Some(r)
    }

    /// The running photo's Select People parts: drawn once its people are found (started here, under the batch's row).
    fn adaptive_people(&mut self) {
        let Some(r) = &self.adaptive.running else { return };
        if !r.parts.iter().any(|p| matches!(p.state, State::People)) {
            return;
        }
        let (id, orient) = (r.id, r.orientation);
        let state = self.people_of(id);
        let people: Result<Arc<Parsed>, String> = match (state.ready, state.error) {
            (Some(p), _) => Ok(p),
            (None, Some(e)) => Err(e),
            (None, None) if self.catalog.photo(id).is_none() => Err("the photo was removed".into()),
            (None, None) => {
                if !state.busy {
                    let parent = self.adaptive.batch.as_ref().map(TaskGuard::id);
                    self.people_find_under(id, false, parent);
                }
                return;
            }
        };
        let Some(r) = self.adaptive.running.as_mut() else { return };
        for p in r.parts.iter_mut().filter(|p| matches!(p.state, State::People)) {
            p.state = State::Done(people_found(people.as_deref(), &p.waiting.recipe, orient));
        }
    }
}

#[cfg(test)]
mod tests;
