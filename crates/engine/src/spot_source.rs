//! A new spot's automatic source, found on a worker (issue #97).
//!
//! `spot.add` with `background: true` (what the Remove tool on the photo sends) adds the spot at once with no source:
//! the render chooses one itself until the search has answered (`lightcraft_pipeline::spots::auto_source`). The search
//! (`pick_source` on a 512-pixel copy of the photo, which may first be made from a larger one or decoded) runs on a
//! worker, so the window never waits on it (if no worker can start, the spot keeps choosing its own). [`Session::poll_spot_sources`], called every frame, puts each answer on its
//! spot as part of the step that added it: one undo removes the spot. A spot moved, changed or removed before the
//! answer came keeps choosing its own source.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::Op;
use lightcraft_develop::{DevelopSettings, Spot};
use lightcraft_geom::Point;

use crate::media::{DecodedSource, SourceLevel, SourceRef};
use crate::{PhotoId, Result, Session};

/// One search that answered: the spot it was for and the source it found.
struct Found {
    photo: PhotoId,
    spot: Spot,
    source: Option<Point>,
    /// The 512-pixel copy, when the worker made it (kept for the next search).
    decoded: Option<DecodedSource>,
}

/// Searches running and answers waiting to be applied.
#[derive(Clone, Default)]
pub struct SpotSources {
    running: Arc<AtomicUsize>,
    done: Arc<Mutex<Vec<Found>>>,
}

impl SpotSources {
    /// Searches started whose answer has not been applied yet.
    pub fn pending(&self) -> usize {
        self.running.load(Ordering::SeqCst) + self.done.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

impl Session {
    /// Find `spot`'s automatic source (it was just added to photo `id`, whose settings are `d`) on a worker.
    pub(crate) fn search_spot_source(&mut self, id: PhotoId, d: DevelopSettings, spot: Spot) {
        let Some(p) = self.catalog.photo(id).cloned() else { return };
        let source = self.media.source_ref(&p, SourceLevel::Thumb);
        let info = self.source_info(id);
        let (running, done) = (self.spot_sources.running.clone(), self.spot_sources.done.clone());
        running.fetch_add(1, Ordering::SeqCst);
        let work = move || {
            #[cfg(test)]
            test_gate();
            let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let made = !matches!(source, SourceRef::Loaded(_));
                let decoded = source.load_source().ok()?;
                let at = lightcraft_pipeline::spots::pick_source(&decoded.image, &info, &d, &spot, None);
                Some((at, made.then_some(decoded)))
            }))
            .ok()
            .flatten();
            let (source, decoded) = found.unwrap_or_default();
            done.lock().unwrap_or_else(PoisonError::into_inner).push(Found { photo: id, spot, source, decoded });
            running.fetch_sub(1, Ordering::SeqCst);
        };
        if let Err(e) = std::thread::Builder::new().name("lc-spot-source".into()).spawn(work) {
            // (the spot keeps choosing its own source, as one moved before its answer came)
            log::warn!("spot source: no worker ({e})");
            self.spot_sources.running.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Put the sources the workers found on their spots (each as part of the step that added the spot); the number put.
    pub fn poll_spot_sources(&mut self) -> usize {
        let answers: Vec<Found> = std::mem::take(&mut *self.spot_sources.done.lock().unwrap_or_else(PoisonError::into_inner));
        let mut put = 0;
        for f in answers {
            if let Some(decoded) = f.decoded {
                self.media.insert_source(f.photo, SourceLevel::Thumb, decoded);
            }
            let Some(at) = f.source else { continue };
            if self.put_spot_source(f.photo, &f.spot, at).unwrap_or(false) {
                put += 1;
            }
        }
        put
    }

    /// `spot` of photo `id` (as added, no source yet) gets `at`, without an undo step of its own: undoing the add
    /// removes it with its source. False when the spot is no longer there as added.
    fn put_spot_source(&mut self, id: PhotoId, spot: &Spot, at: Point) -> Result<bool> {
        let Some(mut d) = self.develop_of(id).map(|d| (*d).clone()) else { return Ok(false) };
        let Some(sp) = d.spots.iter_mut().find(|s| *s == spot) else { return Ok(false) };
        sp.source_offset = Some(at);
        let op = Op::SetDevelop { id, settings: Arc::new(d), label: "Remove".into(), edited: Some((self.clock)()) };
        self.apply_amending(op)?;
        Ok(true)
    }
}

/// Test-only: tests that hold a search take turns (the gate is process-wide).
#[cfg(test)]
pub(crate) static TURN: Mutex<()> = Mutex::new(());

/// Test-only: a search waits here until the test lets it go.
#[cfg(test)]
pub(crate) static GATE: Mutex<Option<std::sync::mpsc::Receiver<()>>> = Mutex::new(None);

#[cfg(test)]
fn test_gate() {
    let rx = GATE.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(rx) = rx {
        let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
    }
}
