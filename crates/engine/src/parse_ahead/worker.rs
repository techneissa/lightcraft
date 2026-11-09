//! The parse-ahead worker: one thread, one photo at a time. For a photo it renders the picture the parse sees and
//! finds its faces exactly as a request does ([`crate::cmd::parsing::Prepare`]), then asks people parsing for it
//! ([`PeopleParsing::parse_quietly`]), so the parse is filed under the key a later request computes. Between steps it waits while it is held (the user is working, the
//! photo on screen is being made, another parse runs); a model run stopped by a hold is run again once the hold ends.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lightcraft_catalog::PhotoId;

use crate::cmd::parsing::Prepare;
use crate::people_parsing::{Outcome, PeopleParsing, QuietRun};

/// How often a held worker looks again.
const HOLD_POLL: Duration = Duration::from_millis(40);
/// Model runs of one photo stopped by holds before it is put back in the queue.
const MAX_ATTEMPTS: usize = 4;

/// One photo to parse.
pub struct Task {
    pub id: PhotoId,
    pub identity: u128,
    /// The render and the face detection, set up on the session thread.
    pub prepare: Prepare,
    /// Set: the photo is dropped (✕, the setting, the app closing).
    pub cancel: Arc<AtomicBool>,
    pub run: QuietRun,
}

/// How a photo ended.
#[derive(Debug)]
pub enum Ended {
    /// Parsed now (`cached`: it already was), filed under `key`.
    Parsed {
        key: Option<String>,
        cached: bool,
    },
    Failed(String),
    /// Dropped or held too often: not parsed, try again later.
    Cancelled,
}

pub struct Finished {
    pub id: PhotoId,
    pub identity: u128,
    pub ended: Ended,
}

pub struct Worker {
    tx: Sender<Task>,
    rx: Receiver<Finished>,
}

impl Worker {
    /// Start the thread; `hold` is set by the pump while the worker must wait.
    pub fn start(parsing: PeopleParsing, hold: Arc<AtomicBool>) -> Result<Worker, String> {
        let (tx, tasks) = channel::<Task>();
        let (done, rx) = channel::<Finished>();
        std::thread::Builder::new()
            .name("parse-ahead".into())
            .spawn(move || {
                // ends when the pump drops its side
                while let Ok(task) = tasks.recv() {
                    let (id, identity) = (task.id, task.identity);
                    let ended = catch_unwind(AssertUnwindSafe(|| work(&parsing, &hold, task)))
                        .unwrap_or_else(|_| Ended::Failed("parsing ahead stopped unexpectedly".into()));
                    if done.send(Finished { id, identity, ended }).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| format!("could not start parsing ahead: {e}"))?;
        Ok(Worker { tx, rx })
    }

    /// Hand the worker a photo; `false` when its thread has gone.
    pub fn submit(&self, task: Task) -> bool {
        self.tx.send(task).is_ok()
    }

    /// The photos it has finished since the last call.
    pub fn finished(&self) -> Vec<Finished> {
        self.rx.try_iter().collect()
    }
}

/// Whether the worker must wait now.
fn held(hold: &AtomicBool, parsing: &PeopleParsing) -> bool {
    hold.load(Ordering::SeqCst) || crate::prep::foreground_busy() || parsing.others_running(None)
}

/// Wait while held; `false`: the task was dropped meanwhile.
fn wait(hold: &AtomicBool, parsing: &PeopleParsing, cancel: &AtomicBool) -> bool {
    while held(hold, parsing) {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(HOLD_POLL);
    }
    !cancel.load(Ordering::SeqCst)
}

fn work(parsing: &PeopleParsing, hold: &AtomicBool, task: Task) -> Ended {
    if !wait(hold, parsing, &task.cancel) {
        return Ended::Cancelled;
    }
    let (img, hints) = match task.prepare.run() {
        Ok(r) => r,
        Err(e) => return Ended::Failed(e),
    };
    for _ in 0..MAX_ATTEMPTS {
        if !wait(hold, parsing, &task.cancel) {
            return Ended::Cancelled;
        }
        task.run.reset();
        match parsing.parse_quietly(img.clone(), hints.clone(), &task.run) {
            Outcome::Ready { cached, .. } => return Ended::Parsed { key: task.run.key(), cached },
            // stopped by a hold (the user came back): run again once it ends; by a drop: end here
            Outcome::Failed(e) if e == "cancelled" => {
                if task.cancel.load(Ordering::SeqCst) {
                    return Ended::Cancelled;
                }
            }
            Outcome::Failed(e) => return Ended::Failed(e),
            Outcome::Running => return Ended::Failed("the parse did not finish".into()),
        }
    }
    Ended::Cancelled
}
