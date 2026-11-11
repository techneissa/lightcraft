//! The parse-ahead worker: one thread, one photo at a time. For a photo it renders the picture the parse sees and
//! finds its faces exactly as a request does ([`crate::cmd::parsing::Prepare`]), then asks people parsing for it
//! ([`PeopleParsing::parse_quietly`]), so the parse is filed under the key a later request computes. Before each step
//! (the render, the face detection, the model run) it waits while it is held (the user is working, the photo on screen
//! is being made, an export runs, another parse runs); a model run stopped by a hold is run again once the hold ends.
//! It also reads, lists and writes the index beside the cache ([`super::index`]), so the UI thread never does.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lightcraft_catalog::PhotoId;

use super::index::{self, Slot};
use crate::activity::Activity;
use crate::cmd::parsing::Prepare;
use crate::people_parsing::{Outcome, PeopleParsing, QuietRun};

/// How often a held worker looks again.
const HOLD_POLL: Duration = Duration::from_millis(40);
/// Model runs of one photo stopped by holds before it is put back in the queue.
const MAX_ATTEMPTS: usize = 4;
/// The rows that hold the work back while they run: exports, and print, book and slideshow output, which share the
/// kind (`docs/background-tasks.md`).
const HOLDING_KINDS: [&str; 1] = ["export"];

/// One photo to parse.
pub struct Task {
    pub id: PhotoId,
    pub identity: u128,
    /// The version `identity` was made with: a parse filed under another (the People edge changed meanwhile) is not
    /// filed for it.
    pub version: String,
    /// The render and the face detection, set up on the session thread.
    pub prepare: Prepare,
    /// Set: the photo is dropped (✕, the setting, the app closing).
    pub cancel: Arc<AtomicBool>,
    pub run: QuietRun,
}

/// What the worker is asked to do.
pub enum Job {
    Parse(Box<Task>),
    /// Read the index and listing of this cache folder.
    Load(std::path::PathBuf),
    /// List the cache folder again.
    Relist,
    /// Write the index if it is due.
    Save,
}

/// How a photo ended.
#[derive(Debug)]
pub enum Ended {
    /// Parsed now (`cached`: it already was), filed under `key` (`None`: not for this photo's identity), whose file
    /// takes `bytes`.
    Parsed {
        key: Option<String>,
        bytes: u64,
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
    tx: Sender<Job>,
    rx: Receiver<Finished>,
}

/// What the worker waits on.
#[derive(Clone)]
pub struct Holds {
    /// Set by the pump while the worker must wait.
    pub hold: Arc<AtomicBool>,
    pub parsing: PeopleParsing,
    pub activity: Activity,
    /// The session's photo on screen being made.
    pub foreground: crate::prep::Foreground,
}

impl Worker {
    /// Start the thread.
    pub fn start(holds: Holds, slot: Slot) -> Result<Worker, String> {
        let (tx, jobs) = channel::<Job>();
        let (done, rx) = channel::<Finished>();
        std::thread::Builder::new()
            .name("parse-ahead".into())
            .spawn(move || {
                // ends when the pump drops its side
                while let Ok(job) = jobs.recv() {
                    let task = match job {
                        Job::Parse(task) => task,
                        Job::Load(dir) => {
                            index::load(&slot, &dir);
                            continue;
                        }
                        Job::Relist => {
                            index::relist(&slot);
                            continue;
                        }
                        Job::Save => {
                            index::save(&slot, false);
                            continue;
                        }
                    };
                    let (id, identity) = (task.id, task.identity);
                    let ended = catch_unwind(AssertUnwindSafe(|| work(&holds, *task)))
                        .unwrap_or_else(|_| Ended::Failed("parsing ahead stopped unexpectedly".into()));
                    if done.send(Finished { id, identity, ended }).is_err() {
                        return;
                    }
                }
            })
            .map_err(|e| format!("could not start parsing ahead: {e}"))?;
        Ok(Worker { tx, rx })
    }

    /// Hand the worker a job; `false` when its thread has gone.
    pub fn send(&self, job: Job) -> bool {
        self.tx.send(job).is_ok()
    }

    /// The photos it has finished since the last call.
    pub fn finished(&self) -> Vec<Finished> {
        self.rx.try_iter().collect()
    }
}

/// Whether a row that holds the work back runs ([`HOLDING_KINDS`]).
pub fn exporting(activity: &Activity) -> bool {
    activity.list().iter().any(|t| HOLDING_KINDS.contains(&t.kind))
}

/// Whether the worker must wait now.
fn held(h: &Holds) -> bool {
    h.hold.load(Ordering::SeqCst) || h.foreground.busy() || h.parsing.others_running(None) || exporting(&h.activity)
}

/// Wait while held; `false`: the task was dropped meanwhile.
fn wait(h: &Holds, cancel: &AtomicBool) -> bool {
    while held(h) {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(HOLD_POLL);
    }
    !cancel.load(Ordering::SeqCst)
}

fn work(h: &Holds, task: Task) -> Ended {
    // (the render can't be stopped once it runs: each step starts only while nothing holds the work back)
    if !wait(h, &task.cancel) {
        return Ended::Cancelled;
    }
    let rendered = match task.prepare.render() {
        Ok(r) => r,
        Err(e) => return Ended::Failed(e),
    };
    if !wait(h, &task.cancel) {
        return Ended::Cancelled;
    }
    let (img, hints) = match rendered.detect() {
        Ok(r) => r,
        Err(e) => return Ended::Failed(e),
    };
    let mut attempts = 0;
    while attempts < MAX_ATTEMPTS {
        if !wait(h, &task.cancel) {
            return Ended::Cancelled;
        }
        // a stop asked for from here on is kept for the run, even before the run has started
        task.run.reset();
        if held(h) {
            continue;
        }
        attempts += 1;
        match h.parsing.parse_quietly(img.clone(), hints.clone(), &task.run) {
            Outcome::Ready { cached, .. } => {
                let key = task.run.key().filter(|_| task.run.version().as_deref() == Some(task.version.as_str()));
                let bytes = key
                    .as_ref()
                    .zip(h.parsing.cache_dir())
                    .and_then(|(k, d)| std::fs::metadata(lightcraft_parsing::cache::path(&d, k)).ok())
                    .map_or(0, |m| m.len());
                return Ended::Parsed { key, bytes, cached };
            }
            // stopped by a hold (the user came back) or a request: run again once it ends; by a drop: end here
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
