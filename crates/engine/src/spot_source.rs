//! A new spot's automatic source, found on a worker (issue #97).
//!
//! `spot.add` with `background: true` (what the Remove tool on the photo sends) returns at once: the search for the
//! spot's source (`pick_source` on a 512-pixel copy of the photo, which may first be made from a larger one or decoded)
//! runs on a worker, and [`Session::poll_spot_sources`], called every frame, adds the spot with its source when the
//! answer comes, as one undo step. The window never waits on the search, and the spot renders once, with its source:
//! added before its answer, it would render twice (choosing its own source first). Spots are added in the order they
//! were asked for. A search that fails, or a worker that can't start, adds the spot without a source (the render then
//! chooses one itself).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::{DevelopSettings, Spot};
use lightcraft_geom::Point;

use crate::media::{DecodedSource, SourceLevel, SourceRef};
use crate::{PhotoId, Session};

/// One search that answered: the spot it was for, its place in the order of asking, and the source it found.
struct Found {
    seq: u64,
    photo: PhotoId,
    spot: Spot,
    source: Option<Point>,
    /// The 512-pixel copy, when the worker made it (kept for the next search).
    decoded: Option<DecodedSource>,
}

/// Searches running and answers waiting to be added.
#[derive(Clone, Default)]
pub struct SpotSources {
    running: Arc<AtomicUsize>,
    done: Arc<Mutex<Vec<Found>>>,
    /// The next search's place, and the next place to add.
    asked: u64,
    added: u64,
}

impl SpotSources {
    /// Spots asked for that are not added yet.
    pub fn pending(&self) -> usize {
        self.running.load(Ordering::SeqCst) + self.done.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl Session {
    /// Add `spot` to photo `id` once its automatic source is found on a worker (`d`: the photo's settings now).
    pub(crate) fn add_spot_when_found(&mut self, id: PhotoId, d: DevelopSettings, spot: Spot) {
        let seq = self.spot_sources.asked;
        self.spot_sources.asked += 1;
        let source = self.catalog.photo(id).cloned().map(|p| self.media.source_ref(&p, SourceLevel::Thumb));
        let info = self.source_info(id);
        let (running, done) = (self.spot_sources.running.clone(), self.spot_sources.done.clone());
        running.fetch_add(1, Ordering::SeqCst);
        let failed = Found { seq, photo: id, spot: spot.clone(), source: None, decoded: None };
        let work = move || {
            #[cfg(test)]
            test_gate(seq);
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
            done.lock().unwrap_or_else(PoisonError::into_inner).push(Found { seq, photo: id, spot, source: at, decoded });
            running.fetch_sub(1, Ordering::SeqCst);
        };
        if let Err(e) = std::thread::Builder::new().name("lc-spot-source".into()).spawn(work) {
            log::warn!("spot source: no worker ({e}); the spot is added without one");
            self.spot_sources.running.fetch_sub(1, Ordering::SeqCst);
            self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner).push(failed);
        }
    }

    /// Add the spots whose source was found, in the order they were asked for; the number added.
    pub fn poll_spot_sources(&mut self) -> usize {
        let mut added = 0;
        loop {
            let next = {
                let mut done = self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner);
                let at = done.iter().position(|f| f.seq == self.spot_sources.added);
                at.map(|i| done.swap_remove(i))
            };
            let Some(f) = next else { break };
            self.spot_sources.added += 1;
            if let Some(decoded) = f.decoded {
                self.media.insert_source(f.photo, SourceLevel::Thumb, decoded);
            }
            let mut spot = f.spot;
            spot.source_offset = f.source;
            match self.add_spot(f.photo, spot) {
                Ok(_) => added += 1,
                Err(e) => log::warn!("spot source: the spot could not be added: {e}"),
            }
        }
        added
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

/// Test-only: tests that hold a search take turns (the gate is process-wide).
#[cfg(test)]
pub(crate) static TURN: Mutex<()> = Mutex::new(());

/// Test-only: the search with this place in the order waits here until the test lets it go.
#[cfg(test)]
pub(crate) static GATE: Mutex<Option<(u64, std::sync::mpsc::Receiver<()>)>> = Mutex::new(None);

#[cfg(test)]
fn test_gate(seq: u64) {
    let rx = {
        let mut g = GATE.lock().unwrap_or_else(PoisonError::into_inner);
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
