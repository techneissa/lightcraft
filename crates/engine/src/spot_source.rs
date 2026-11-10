//! A new spot's automatic source, found on a worker (issue #97).
//!
//! In the window (which installs a waker, [`SpotSources::set_waker`]), `spot.add` with `background: true` (what the
//! Remove tool on the photo sends) returns at once: the search for the spot's source (`pick_source` on a 512-pixel copy
//! of the photo, which may first be made from a larger one or decoded) runs on a worker, which wakes the window when it
//! answers, and [`Session::poll_spot_sources`], called every frame, adds the spot with its source as one undo step. The
//! window never waits on the search, and the spot renders once, with its source. Without a window (scripts, the CLI,
//! MCP) nothing would poll: the search runs at once there, as without the flag.
//!
//! - Spots are added in the order they were asked for; a spot that needs no search, asked behind pending ones, waits its
//!   turn too.
//! - While a drag (an interaction) is open on the photo, answers wait and are added after it ends; a spot asked during a
//!   drag that is cancelled is dropped with it.
//! - Undo and redo first finish the pending searches, so they undo the spot just clicked, not the step before it.
//! - An answer for a library that was closed since is dropped.
//! - A search that fails, or a worker that can't start, adds the spot without a source (the render then chooses one).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use lightcraft_develop::{DevelopSettings, Spot};
use lightcraft_geom::Point;

use crate::media::{DecodedSource, SourceLevel, SourceRef};
use crate::{PhotoId, Session};

/// One search that answered: its place in the order of asking, the spot, and the source it found.
struct Found {
    seq: u64,
    spot: Spot,
    source: Option<Point>,
    /// The 512-pixel copy, when the worker made it (kept for the next search).
    decoded: Option<DecodedSource>,
}

/// What the session knows of a spot asked for and not added yet.
#[derive(Clone)]
struct Asked {
    photo: PhotoId,
    /// The library opening it was asked in.
    library: Weak<()>,
    /// Asked while a drag was open on the photo: dropped if that drag is cancelled.
    in_drag: bool,
}

/// Called by a worker when its answer is ready (the window repaints).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Searches running and spots waiting to be added.
#[derive(Clone, Default)]
pub struct SpotSources {
    running: Arc<AtomicUsize>,
    done: Arc<Mutex<Vec<Found>>>,
    waker: Arc<Mutex<Option<Waker>>>,
    asked: HashMap<u64, Asked>,
    /// The next spot's place, and the next place to add.
    next: u64,
    added: u64,
    /// Test-only: the search with this place in the order waits until the test lets it go. Each session has its own.
    #[cfg(test)]
    pub(crate) gate: Arc<Mutex<Option<(u64, std::sync::mpsc::Receiver<()>)>>>,
}

impl SpotSources {
    /// Spots asked for that are not added yet.
    pub fn pending(&self) -> usize {
        self.asked.len()
    }

    /// Wake the window with `waker` when a search answers; from then on `background` spots are searched on workers.
    pub fn set_waker(&mut self, waker: Waker) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(waker);
    }

    /// Whether a window polls this session (it set a waker).
    pub fn has_waker(&self) -> bool {
        self.waker.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    /// The drag open on `photo` ended: the spots asked during it are added as their own steps.
    pub(crate) fn drag_ended(&mut self, photo: PhotoId) {
        self.asked.values_mut().filter(|a| a.photo == photo).for_each(|a| a.in_drag = false);
    }

    /// The drag open on `photo` was cancelled: the spots asked during it are dropped with it.
    pub(crate) fn drag_cancelled(&mut self, photo: PhotoId) {
        self.asked.retain(|_, a| !(a.photo == photo && a.in_drag));
    }
}

impl Session {
    /// Whether a `background` spot is searched on a worker: only when a window polls the session.
    pub(crate) fn spots_in_background(&self) -> bool {
        self.spot_sources.has_waker()
    }

    /// Remember a spot asked for photo `id`; its place in the order.
    fn ask(&mut self, id: PhotoId) -> u64 {
        let seq = self.spot_sources.next;
        self.spot_sources.next += 1;
        let in_drag = self.interaction.as_ref().is_some_and(|i| i.photo == id);
        self.spot_sources.asked.insert(seq, Asked { photo: id, library: Arc::downgrade(&self.library_identity), in_drag });
        seq
    }

    /// Add `spot` to photo `id` once its automatic source is found on a worker (`d`: the photo's settings now).
    pub(crate) fn add_spot_when_found(&mut self, id: PhotoId, d: DevelopSettings, spot: Spot) {
        let seq = self.ask(id);
        let source = self.catalog.photo(id).cloned().map(|p| self.media.source_ref(&p, SourceLevel::Thumb));
        let info = self.source_info(id);
        let (running, done, waker) = (self.spot_sources.running.clone(), self.spot_sources.done.clone(), self.spot_sources.waker.clone());
        #[cfg(test)]
        let gate = self.spot_sources.gate.clone();
        running.fetch_add(1, Ordering::SeqCst);
        let failed = Found { seq, spot: spot.clone(), source: None, decoded: None };
        let work = move || {
            #[cfg(test)]
            test_gate(&gate, seq);
            let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let source = source?;
                let made = !matches!(source, SourceRef::Loaded(_));
                let decoded = source.load_source().ok()?;
                let at = lightcraft_pipeline::spots::pick_source(&decoded.image, &info, &d, &spot, None);
                Some((at, made.then_some(decoded)))
            }))
            .ok()
            .flatten();
            let (at, decoded) = found.unwrap_or_default();
            done.lock().unwrap_or_else(PoisonError::into_inner).push(Found { seq, spot, source: at, decoded });
            running.fetch_sub(1, Ordering::SeqCst);
            // the window sleeps until something wakes it: this answer must
            let wake = waker.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(wake) = wake {
                wake();
            }
        };
        if let Err(e) = std::thread::Builder::new().name("lc-spot-source".into()).spawn(work) {
            log::warn!("spot source: no worker ({e}); the spot is added without one");
            self.spot_sources.running.fetch_sub(1, Ordering::SeqCst);
            self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner).push(failed);
        }
    }

    /// Add `spot` (which needs no search) to photo `id`: now, or after the spots asked before it when any are pending.
    /// Whether it waits.
    pub(crate) fn add_spot_in_turn(&mut self, id: PhotoId, spot: Spot) -> crate::Result<Option<usize>> {
        if self.spot_sources.pending() == 0 {
            return self.add_spot(id, spot).map(Some);
        }
        let seq = self.ask(id);
        let source = spot.source_offset;
        self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner).push(Found { seq, spot, source, decoded: None });
        Ok(None)
    }

    /// Add the spots whose source was found, in the order they were asked for; the number added. A spot waits while a
    /// drag is open on its photo.
    pub fn poll_spot_sources(&mut self) -> usize {
        let mut added = 0;
        {
            // answers of spots dropped since (their drag was cancelled) are thrown away
            let asked = &self.spot_sources.asked;
            self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner).retain(|f| asked.contains_key(&f.seq));
        }
        while self.spot_sources.added < self.spot_sources.next {
            let seq = self.spot_sources.added;
            let Some(asked) = self.spot_sources.asked.get(&seq).cloned() else {
                // dropped (its drag was cancelled)
                self.spot_sources.added += 1;
                continue;
            };
            let library = Arc::downgrade(&self.library_identity);
            let elsewhere = !Weak::ptr_eq(&asked.library, &library);
            if !elsewhere && self.interaction.as_ref().is_some_and(|i| i.photo == asked.photo) {
                break;
            }
            let found = {
                let mut done = self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner);
                let at = done.iter().position(|f| f.seq == seq);
                at.map(|i| done.swap_remove(i))
            };
            let Some(f) = found else { break };
            self.spot_sources.added += 1;
            self.spot_sources.asked.remove(&seq);
            if elsewhere {
                // asked in a library that has been closed since
                continue;
            }
            if let Some(decoded) = f.decoded {
                self.media.insert_source(asked.photo, SourceLevel::Thumb, decoded);
            }
            let mut spot = f.spot;
            spot.source_offset = f.source;
            match self.add_spot(asked.photo, spot) {
                Ok(_) => added += 1,
                Err(e) => log::warn!("spot source: the spot could not be added: {e}"),
            }
        }
        added
    }

    /// Wait for the searches still running and add their spots (before an undo or a redo, which would otherwise undo
    /// the step before a spot just clicked). Searches take milliseconds; a worker stuck for 10 s is left behind.
    pub(crate) fn finish_spot_sources(&mut self) {
        let start = std::time::Instant::now();
        while self.spot_sources.running.load(Ordering::SeqCst) > 0 && start.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        self.poll_spot_sources();
    }

    /// Add `spot` to photo `id` as one undo step ("Remove"), selecting it when the photo is the active one; its index.
    pub(crate) fn add_spot(&mut self, id: PhotoId, spot: Spot) -> crate::Result<usize> {
        let mut d = (*self.develop_of(id).unwrap_or_default()).clone();
        d.spots.push(spot);
        let n = d.spots.len();
        self.set_develop(id, d, "Remove")?;
        if self.active() == Some(id) {
            self.active_spot = Some(n - 1);
        }
        Ok(n - 1)
    }
}

#[cfg(test)]
fn test_gate(gate: &Mutex<Option<(u64, std::sync::mpsc::Receiver<()>)>>, seq: u64) {
    let rx = {
        let mut g = gate.lock().unwrap_or_else(PoisonError::into_inner);
        if g.as_ref().is_some_and(|(s, _)| *s == seq) { g.take() } else { None }
    };
    if let Some((_, rx)) = rx {
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
    }
}

#[cfg(test)]
impl SpotSources {
    /// Answers waiting to be added.
    pub(crate) fn answered(&self) -> usize {
        self.done.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}
