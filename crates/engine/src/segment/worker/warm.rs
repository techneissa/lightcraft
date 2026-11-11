//! Core ML's warm-up of SAM 3 (issue #268): Core ML plans each part on the GPU at its first run after a load (1.6–5.6
//! s each), so the parts the user has not run yet are run once on made-up inputs while the worker is idle.
//!
//! - It starts only after a reply has gone out and no request waits ([`Flags::resume`]), one at a time per worker.
//! - [`super::Worker::submit`] pauses it ([`Flags::pause`]): it stops between parts, so a request waits for at most the
//!   one part being planned (Core ML runs one prediction of a part at a time), and never for a part the user is
//!   running ([`lightcraft_segment::WarmUp::step`] skips it). The next idle moment resumes it with the parts left.
//! - It shows in the top bar's ring (the app starts it), and stopping it there, or a part that fails, ends it for the
//!   loaded model.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What warms a loaded model up, one part at a time (Core ML's parts; a stand-in in tests).
pub(crate) trait Steps: Send + Sync {
    /// Run the next part not run yet; `false` when none was left to run now.
    fn step(&self) -> Result<bool, String>;
    /// Whether every part has run once.
    fn done(&self) -> bool;
}

impl Steps for lightcraft_segment::WarmUp {
    fn step(&self) -> Result<bool, String> {
        lightcraft_segment::WarmUp::step(self).map_err(|e| e.to_string())
    }

    fn done(&self) -> bool {
        lightcraft_segment::WarmUp::done(self)
    }
}

/// The warm-up's flags, shared by the worker thread, the session's `submit` and the warm-up thread.
#[derive(Default)]
pub struct Flags {
    /// Raised by a request: the warm-up stops before its next part.
    stop: Arc<AtomicBool>,
    /// A warm-up thread is running (one at a time).
    running: Arc<AtomicBool>,
    /// The warm-up was stopped from its row, or a part failed: no more for the loaded model.
    given_up: Arc<AtomicBool>,
}

impl Flags {
    /// A request came: the warm-up stops after the part it is on.
    pub fn pause(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Whether a warm-up thread is running (tests).
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Whether the warm-up was given up since the last call (stopped from its row, or a part failed).
    pub(crate) fn take_given_up(&self) -> bool {
        self.given_up.swap(false, Ordering::SeqCst)
    }

    /// Warm `steps` up on a thread of its own, with a row in the top bar's ring, unless one runs already, every part
    /// has run, or `busy` (a request waits). `busy` is read after the stop flag is lowered, and `submit` raises it
    /// after counting its request, so a request that comes meanwhile is either seen here or stops the thread before
    /// its first part. Returns whether a thread started.
    pub(crate) fn resume(&self, steps: &Arc<dyn Steps>, activity: Option<&crate::activity::Activity>, busy: impl Fn() -> bool) -> bool {
        if steps.done() || self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.stop.store(false, Ordering::SeqCst);
        if busy() {
            self.stop.store(true, Ordering::SeqCst);
            self.running.store(false, Ordering::SeqCst);
            return false;
        }
        let user = Arc::new(AtomicBool::new(false));
        let row = activity.map(|a| a.start_background("sam3-warm-up", "Getting AI masks ready", crate::activity::Cancel::Flag(user.clone())));
        let (steps, stop, running, given_up) = (steps.clone(), self.stop.clone(), self.running.clone(), self.given_up.clone());
        let spawned = std::thread::Builder::new().name("sam3-warm-up".into()).spawn(move || {
            let _row = row;
            let t = web_time::Instant::now();
            let mut parts = 0;
            while !stop.load(Ordering::SeqCst) && !user.load(Ordering::SeqCst) {
                match steps.step() {
                    Ok(true) => parts += 1,
                    Ok(false) => break,
                    Err(e) => {
                        // the part's next real run reports it (and on Automatic hands the model to candle)
                        log::warn!("SAM 3's Core ML warm-up failed: {e}");
                        given_up.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            if user.load(Ordering::SeqCst) {
                given_up.store(true, Ordering::SeqCst);
            }
            log::info!("SAM 3's Core ML warm-up planned {parts} parts in {:?}{}", t.elapsed(), if steps.done() { ", all done" } else { "" });
            running.store(false, Ordering::SeqCst);
        });
        if let Err(e) = spawned {
            log::warn!("could not start SAM 3's Core ML warm-up: {e}");
            self.running.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}
