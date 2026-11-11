//! The renderer's side of the loupe's prefetch ([`Slot::Prefetch`]): what each slot was asked for, dropping the ones
//! the user has stepped past, the loupe taking over one already running for the photo it lands on, and how long a
//! prefetch takes. What to prefetch, and when, is `panels::detail::prefetch`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::media::{QuickSource, RenderJob, RenderResult};
use lightcraft_preview::Hash128;

use super::{Finished, Renderer, Requested, Slot, geometry_look, render_subject};

/// The longest the loupe waits for a prefetch of its photo that is running (s): one still running after that is held
/// up (at the memory gate, on a slow drive) or far slower than a render of the loupe's own, which then starts.
const ADOPT_LIMIT_SECS: f64 = 3.0;

/// Loupe renders of photos left that may go on beside the photo on screen's ([`lightcraft_preview::JobPool::release_running`]):
/// a decode can't stop half-way, so each one still costs a share of the cores. Past this, the next one keeps the slot.
const STOPPED_RUNNING: usize = 1;

/// The loupe renders still out that are remembered ([`Renderer::main_rendering`]): a step replaces the queued one, so
/// a few running at once is the most there can be.
const MAIN_OUT_KEPT: usize = 8;

/// How much a new prefetch time moves the running average ([`Renderer::prefetch_secs`]).
const TIME_WEIGHT: f64 = 0.3;

/// A kept view smaller than what is asked for by at most this share still covers it (the engine's own slack for a
/// cached view: rounding of the fitted size).
const COVER_SLACK: f64 = 0.01;

/// A loupe render on the workers ([`Renderer::note_main_out`]).
#[derive(Debug)]
pub(crate) struct MainOut {
    photo: PhotoId,
    /// Its request id.
    id: u64,
    /// When it was sent.
    at: web_time::Instant,
    /// Set: stop, nobody will show it.
    stop: Arc<AtomicBool>,
}

/// What a [`Slot::Prefetch`] was last asked for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Asked {
    /// The job's key.
    pub key: u64,
    pub photo: PhotoId,
    /// The key of the photo's kept view the job writes (`None`: it only decodes the photo, for editing).
    pub view: Option<Hash128>,
    /// The size asked for.
    pub size: [usize; 2],
    /// When it went to the workers (`None`: nothing was rendered, the photo's kept view already covered it).
    pub at: Option<web_time::Instant>,
}

impl Asked {
    fn of(job: &RenderJob, at: Option<web_time::Instant>) -> Asked {
        Asked { key: job.key, photo: job.photo, view: job.view_cache.as_ref().map(|(_, k)| *k), size: [job.request.max_w, job.request.max_h], at }
    }
}

/// Whether a picture of `have` pixels covers one fitted into `want` (same photo, same look: the same aspect).
pub(crate) fn covers(have: (usize, usize), want: [usize; 2]) -> bool {
    let (w, h) = have;
    if w == 0 || h == 0 || want[0] == 0 || want[1] == 0 {
        return false;
    }
    // (the scale from what is there to what is wanted: at most 1 when it is no smaller)
    let s = (want[0] as f64 / w as f64).min(want[1] as f64 / h as f64);
    s.is_finite() && s <= 1.0 + COVER_SLACK
}

impl Renderer {
    /// Prepare a photo in the background (a [`Slot::Prefetch`] job runs once per key; a newer one
    /// replaces it while queued). Memory stays bounded by the engine's source caches.
    pub fn prefetch(&mut self, slot: Slot, mut job: RenderJob, priority: u32) {
        if self.prefetch_asked(slot, job.key) {
            return;
        }
        self.prefetched.insert(slot, Asked::of(&job, Some(web_time::Instant::now())));
        self.note_request(slot, job.key, priority, "prefetch requested", || render_subject(slot, job.photo, Some(&job.origin)));
        self.pending.insert(slot, (job.key, priority));
        job.request_id = lightcraft_preview::next_tick();
        self.requests.insert(
            slot,
            Requested { look: Some((geometry_look(&job.settings), job.request.apply_crop)), id: job.request_id, source: Some(job.origin.clone()) },
        );
        let key = job.key;
        #[cfg(test)]
        {
            *self.renders_asked.entry(job.photo).or_default() += 1;
        }
        // (a job that panics comes back failed, and its slot is free again: #227)
        let panicked = Self::panicked(job.failed(String::new()));
        self.pool.submit_or(
            slot,
            key,
            priority,
            Box::new(move || Finished {
                look: None,
                result: lightcraft_engine::memory::in_background(|| job.run()),
                color: None,
                full: None,
                then: None,
            }),
            panicked,
        );
    }

    /// Note `job` as asked for in `slot` without rendering it: the photo's kept view already covers it (#223).
    pub fn prefetch_covered(&mut self, slot: Slot, job: &RenderJob) {
        self.decided(|| format!("{}: not prefetched, its kept view already covers it", render_subject(slot, job.photo, Some(&job.origin))));
        self.prefetched.insert(slot, Asked::of(job, None));
    }

    /// Prepared-ahead jobs ([`Slot::Prefetch`]) queued or running.
    pub fn prefetches_pending(&self) -> usize {
        self.pending.keys().filter(|s| matches!(s, Slot::Prefetch(_))).count()
    }

    /// Prepared-ahead slots asked for since the renderer last forgot everything.
    pub fn prefetch_slots_asked(&self) -> usize {
        self.prefetched.keys().filter(|s| matches!(s, Slot::Prefetch(_))).count()
    }

    /// Whether `slot` was last asked for the job of `key` (queued, running, done or covered already).
    pub fn prefetch_asked(&self, slot: Slot, key: u64) -> bool {
        self.prefetched.get(&slot).is_some_and(|a| a.key == key)
    }

    /// Drop the prefetches still queued for photos not in `keep` (stepped past, or out of reach of the plan); the slot
    /// is free to be asked again. One already running finishes: its picture is kept for a step back. How many were.
    pub fn drop_prefetches_except(&mut self, keep: &[PhotoId]) -> usize {
        let gone: Vec<(Slot, PhotoId)> =
            self.prefetched.iter().filter(|(s, a)| !keep.contains(&a.photo) && self.pending.contains_key(*s)).map(|(s, a)| (*s, a.photo)).collect();
        let mut dropped = 0;
        for (slot, photo) in gone {
            if self.drop_queued(slot) {
                self.prefetched.remove(&slot);
                self.decided(|| format!("{}: abandoned, stepped past before it started", render_subject(slot, photo, None)));
                dropped += 1;
            }
        }
        dropped
    }

    /// The loupe is about to render `photo` (its kept view `view`, drawn at `fit`): whether a prefetch of it that will
    /// give that view is already running, so the loupe waits for it instead of decoding the photo a second time (two
    /// renders of one look, the smaller finishing last over the larger: #223). A prefetch of it still queued is
    /// dropped (the loupe's own render goes first), and one running too long is not waited for.
    pub fn adopt_prefetch(&mut self, photo: PhotoId, view: Hash128, fit: [usize; 2]) -> bool {
        let Some((slot, asked)) = self.prefetched.iter().find(|(s, a)| a.photo == photo && self.pending.contains_key(*s)).map(|(s, a)| (*s, *a))
        else {
            return false;
        };
        if self.drop_queued(slot) {
            self.prefetched.remove(&slot);
            self.decided(|| format!("{}: abandoned, the loupe renders the photo itself", render_subject(slot, photo, None)));
            return false;
        }
        let fresh = asked.at.is_some_and(|at| at.elapsed().as_secs_f64() < ADOPT_LIMIT_SECS);
        fresh && asked.view == Some(view) && covers((asked.size[0], asked.size[1]), fit)
    }

    /// How long a prefetch render takes (s), averaged over the recent ones (`None` before the first): the loupe plans how
    /// far ahead to prepare with it. Prefetches answered from a kept view don't count.
    pub fn prefetch_secs(&self) -> Option<f64> {
        #[cfg(test)]
        if let Some(secs) = self.render_secs_for_test {
            return Some(secs);
        }
        self.prefetch_ms.map(|ms| ms / 1000.0)
    }

    /// Tests: the photos of the loupe's renders asked for and not told to stop, oldest first.
    #[cfg(test)]
    pub(crate) fn main_asked(&self) -> Vec<PhotoId> {
        self.main_out.iter().filter(|o| !o.stop.load(Ordering::Relaxed)).map(|o| o.photo).collect()
    }

    /// A loupe render (`job`, its request id set) goes to the workers: the job to send, which stops when told to. The
    /// one still queued, if any, is replaced by it and never comes back; the ones running for other photos are told to
    /// stop (nobody will show them: their results are dropped), so the workers go to the photo on screen. A running
    /// job stops at its next check (before the read, after the decode); one past those finishes.
    pub(super) fn note_main_out(&mut self, job: RenderJob) -> RenderJob {
        if self.is_queued(Slot::Main) {
            self.main_out.pop();
        }
        for out in self.main_out.iter().filter(|o| o.photo != job.photo) {
            out.stop.store(true, Ordering::Relaxed);
        }
        // the one holding the loupe's slot (the newest still out) lets go of it, so this render starts at once instead
        // of after its decode; its result is dropped. Not one of this photo (an edit's earlier render).
        if self.main_out.last().is_some_and(|o| o.photo != job.photo) && self.pool.release_running(Slot::Main, STOPPED_RUNNING) {
            self.main_out.pop();
        }
        if self.main_out.len() >= MAIN_OUT_KEPT {
            self.main_out.remove(0);
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.main_out.push(MainOut { photo: job.photo, id: job.request_id, at: web_time::Instant::now(), stop: stop.clone() });
        job.with_stop(stop)
    }

    /// A job of `slot` came back after `ms` on its worker: what the loupe's renders and the prefetches keep track of.
    pub(super) fn note_back(&mut self, slot: Slot, r: &RenderResult, ms: f64) {
        match slot {
            Slot::Main => self.main_back(r),
            Slot::Prefetch(_) => self.note_prefetch_time(r, ms),
            _ => {}
        }
    }

    /// A loupe render came back (shown, superseded or failed).
    fn main_back(&mut self, r: &RenderResult) {
        self.main_out.retain(|o| o.id != r.request_id);
        #[cfg(test)]
        if r.rendered.is_ok() && r.quick.is_none() {
            *self.renders_asked.entry(r.photo).or_default() += 1;
        }
    }

    /// Whether a loupe render of `photo` is on the workers, and will make its view. Not prefetched meanwhile: two
    /// renders of one look (#223). One told to stop, or out too long, doesn't count.
    pub fn main_rendering(&self, photo: PhotoId) -> bool {
        self.main_out.iter().any(|o| o.photo == photo && !o.stop.load(Ordering::Relaxed) && o.at.elapsed().as_secs_f64() < ADOPT_LIMIT_SECS)
    }

    /// A prefetch finished in `ms` on its worker.
    fn note_prefetch_time(&mut self, r: &RenderResult, ms: f64) {
        if r.rendered.is_err() || r.quick == Some(QuickSource::Cached) || !ms.is_finite() || ms <= 0.0 {
            return;
        }
        self.prefetch_ms = Some(self.prefetch_ms.map_or(ms, |avg| avg + (ms - avg) * TIME_WEIGHT));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_catalog::{Op, Photo, Source};
    use lightcraft_engine::Session;
    use lightcraft_preview::JobPool;

    /// A session with photos 1..=3, and a renderer whose jobs stay queued until a test takes them off the queue (as
    /// a worker starting them would: still pending, no longer queued).
    fn fixture() -> (Session, Renderer) {
        let mut session = Session::new();
        for id in 1..=3 {
            let name = format!("cull-{id}.jpg");
            let p = Photo::new(PhotoId(id), Source::File { path: name.clone() }, &name, "JPEG", 6000, 4000, "2026-01-01");
            session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        (session, Renderer { pool: JobPool::new(0), ..Default::default() })
    }

    fn job(session: &mut Session, id: u64) -> RenderJob {
        session.loupe_job(PhotoId(id), 900, 600, true).unwrap()
    }

    fn start_queued(r: &Renderer) {
        r.pool.reprioritize(|_, _| None);
    }

    #[test]
    fn a_prefetch_stepped_past_is_dropped_while_it_waits() {
        let (mut session, mut r) = fixture();
        let (one, two) = (job(&mut session, 1), job(&mut session, 2));
        let (k1, k2) = (one.key, two.key);
        r.prefetch(Slot::Prefetch(1), one, 4);
        r.prefetch(Slot::Prefetch(2), two, 4);
        assert_eq!(r.drop_prefetches_except(&[PhotoId(2)]), 1);
        assert_eq!(r.prefetches_pending(), 1, "photo 2 still waits");
        assert!(!r.prefetch_asked(Slot::Prefetch(1), k1), "the slot is free to be asked again");
        assert!(r.prefetch_asked(Slot::Prefetch(2), k2));
        // one a worker has started finishes (its view is kept for a step back)
        start_queued(&r);
        assert_eq!(r.drop_prefetches_except(&[]), 0);
        assert_eq!(r.prefetches_pending(), 1);
    }

    #[test]
    fn the_loupe_waits_for_a_running_prefetch_of_its_photo_instead_of_rendering_it_again() {
        let (mut session, mut r) = fixture();
        let j = job(&mut session, 1);
        let view = j.view_cache.as_ref().map(|(_, k)| *k).unwrap();
        r.prefetch(Slot::Prefetch(1), j, 4);
        assert!(!r.adopt_prefetch(PhotoId(1), view, [900, 600]), "still queued: the loupe renders the photo itself");
        assert_eq!(r.prefetches_pending(), 0, "and the queued prefetch is dropped");
        r.prefetch(Slot::Prefetch(1), job(&mut session, 1), 4);
        start_queued(&r);
        assert!(r.adopt_prefetch(PhotoId(1), view, [900, 600]), "running: its view is the loupe's picture");
        assert!(!r.adopt_prefetch(PhotoId(1), view, [1800, 1200]), "smaller than the loupe draws: not waited for");
        assert!(!r.adopt_prefetch(PhotoId(2), view, [900, 600]), "another photo");
    }

    /// The loupe's renders asked for, as (photo, told to stop), oldest first.
    fn out(r: &Renderer) -> Vec<(u64, bool)> {
        r.main_out.iter().map(|o| (o.photo.0, o.stop.load(Ordering::Relaxed))).collect()
    }

    /// Stepping at a steady pace, each step asks for the new photo's render at once; the last photo's render goes:
    /// dropped while it waits, told to stop once it runs (and it does, at its next check).
    #[test]
    fn each_step_renders_the_new_photo_at_once_and_stops_the_one_left() {
        let (mut session, mut r) = fixture();
        r.request(Slot::Main, job(&mut session, 1), 100);
        // the step came before photo 1's render started: replaced, never run
        r.request(Slot::Main, job(&mut session, 2), 100);
        assert_eq!(out(&r), vec![(2, false)]);
        assert_eq!(r.pending.get(&Slot::Main).map(|p| p.1), Some(100));
        // photo 2's render runs; the step to photo 3 tells it to stop
        start_queued(&r);
        r.request(Slot::Main, job(&mut session, 3), 100);
        assert_eq!(out(&r), vec![(2, true), (3, false)]);
        assert!(r.pool.is_queued(Slot::Main), "photo 3's render is asked for at once");
        // a job told to stop doesn't read its photo (this one's file isn't there: it would fail on that instead)
        let stop = Arc::new(AtomicBool::new(true));
        let stopped = job(&mut session, 2).with_stop(stop).run();
        assert_eq!(stopped.rendered.err().as_deref(), Some(lightcraft_engine::media::STOPPED));
        // a re-render of the photo on screen (an edit) doesn't stop its own earlier one
        start_queued(&r);
        r.request(Slot::Main, job(&mut session, 3).draft(), 100);
        assert!(out(&r).iter().filter(|(p, _)| *p == 3).all(|(_, stop)| !stop), "{:?}", out(&r));
    }

    #[test]
    fn a_loupe_render_counts_until_it_is_told_to_stop_or_dropped() {
        let (mut session, mut r) = fixture();
        r.request(Slot::Main, job(&mut session, 1), 100);
        assert!(r.main_rendering(PhotoId(1)));
        // an edit's render of the same photo: the earlier one goes on
        start_queued(&r);
        r.request(Slot::Main, job(&mut session, 1).draft(), 100);
        assert!(r.main_rendering(PhotoId(1)));
        // a step: photo 1's renders are told to stop, and make no view
        r.request(Slot::Main, job(&mut session, 2), 100);
        assert!(!r.main_rendering(PhotoId(1)) && r.main_rendering(PhotoId(2)));
        assert!(r.drop_queued(Slot::Main));
        assert!(!r.main_rendering(PhotoId(2)), "a dropped one never runs");
    }

    #[test]
    fn a_kept_view_covers_what_is_no_bigger() {
        // the loupe's 2560 px view and the 2176 px fit a prefetch asks for (#223)
        assert!(covers((2560, 1707), [2176, 1451]));
        assert!(covers((2176, 1450), [2176, 1451]), "a pixel of rounding");
        assert!(!covers((2176, 1450), [2560, 1707]), "a smaller one does not");
        assert!(!covers((0, 0), [10, 10]));
        assert!(!covers((10, 10), [0, 0]));
    }
}
