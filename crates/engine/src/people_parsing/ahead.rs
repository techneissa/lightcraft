//! Fork feature (`fork/parse-ahead.md`): what the parse-ahead queue ([`crate::parse_ahead`]) needs from people
//! parsing. A parse asked for from the queue's worker thread goes through [`PeopleParsing::parse`] like any other (the
//! same cache key, the same run, a foreground caller that wants the same photo joins it), with two differences:
//! it has no activity row of its own (the queue keeps one row for all its photos), and the queue learns the key the
//! parse is filed under and the run's cancel flag, so it can skip the photo next time and stop the run when the user
//! needs the machine.
//!
//! Every parse looked up also marks its cache file as just used, so the cache's size limit removes the parses used
//! least recently ([`lightcraft_parsing::cache::prune`] removes the oldest modification times first).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lightcraft_parsing::{FaceHint, ModelSet, RgbImage, cache};

use super::{CACHE_BYTES, Job, Outcome, PeopleParsing, lock};
use crate::activity::TaskGuard;

/// What a quiet parse shares with whoever runs the queue: the cache key it is filed under and the run's cancel flag,
/// once the run has started.
#[derive(Clone, Default)]
pub(crate) struct QuietRun {
    slot: Arc<Mutex<RunSlot>>,
}

#[derive(Default)]
struct RunSlot {
    key: Option<String>,
    flag: Option<Arc<AtomicBool>>,
    /// Stop the run as soon as it starts (asked for before its flag was known).
    stop: bool,
}

impl QuietRun {
    /// The cache key the parse looked up (known once the parse has started).
    pub(crate) fn key(&self) -> Option<String> {
        lock(&self.slot).key.clone()
    }

    /// Whether the model run has started (and so can be stopped).
    pub(crate) fn running(&self) -> bool {
        lock(&self.slot).flag.is_some()
    }

    /// Stop the run now, or as soon as it starts.
    pub(crate) fn stop(&self) {
        let mut s = lock(&self.slot);
        s.stop = true;
        if let Some(f) = &s.flag {
            f.store(true, Ordering::SeqCst);
        }
    }

    /// Forget a stop asked for (a new attempt of the same photo).
    pub(crate) fn reset(&self) {
        let mut s = lock(&self.slot);
        s.stop = false;
        s.flag = None;
    }
}

thread_local! {
    /// Set while this thread runs a quiet parse.
    static QUIET: RefCell<Option<QuietRun>> = const { RefCell::new(None) };
}

fn quiet() -> Option<QuietRun> {
    QUIET.with(|q| q.borrow().clone())
}

impl PeopleParsing {
    /// [`Self::parse`] of `img` with `hints`, waiting, for the parse-ahead queue: no row of its own; `run` learns the
    /// key and the cancel flag. A run stopped through `run` ends as `Failed("cancelled")`, and leaves no message.
    pub(crate) fn parse_quietly(&self, img: RgbImage, hints: Vec<FaceHint>, run: &QuietRun) -> Outcome {
        QUIET.with(|q| *q.borrow_mut() = Some(run.clone()));
        let out = self.parse(img, hints, true);
        QUIET.with(|q| *q.borrow_mut() = None);
        if matches!(&out, Outcome::Failed(e) if e == "cancelled") {
            let mut m = lock(&self.inner.message);
            if m.as_deref() == Some("cancelled") {
                *m = None;
            }
        }
        out
    }

    /// Hook of [`Self::remembered`]: every lookup of `key` marks its cache file as just used; a quiet parse also
    /// learns the key.
    pub(super) fn looked_up(&self, key: &str) {
        if let Some(q) = quiet() {
            lock(&q.slot).key = Some(key.to_string());
        }
        if let Some(dir) = self.cache_dir() {
            touch(&cache::path(&dir, key));
        }
    }

    /// Hook of [`Self::spawn`]: the run's row, or none for a quiet parse (whose queue learns the run's flag instead).
    pub(super) fn shown(&self, row: TaskGuard, set: ModelSet, flag: &Arc<AtomicBool>) -> Option<TaskGuard> {
        let Some(q) = quiet() else {
            row.detail(set.label());
            return Some(row);
        };
        let mut s = lock(&q.slot);
        s.flag = Some(flag.clone());
        if s.stop {
            flag.store(true, Ordering::SeqCst);
        }
        None
    }

    /// Whether a caller other than the queue waits for the run of `key`.
    pub(crate) fn waited_on(&self, key: &str) -> bool {
        // (the queue's own wait counts once)
        lock(&self.inner.jobs).waiters.get(key).copied().unwrap_or(0) > 1
    }

    /// Whether a run other than the one of `mine` is going.
    pub(crate) fn others_running(&self, mine: Option<&str>) -> bool {
        lock(&self.inner.jobs).map.iter().any(|(k, j)| matches!(j, Job::Running(_)) && Some(k.as_str()) != mine)
    }

    /// Whether a parse can run now: the chosen set is downloaded (or a test runner stands in for it).
    pub(crate) fn can_parse(&self) -> bool {
        lock(&self.inner.runner).is_some() || self.installed(self.model_set())
    }

    /// The parse filed under `key`, from memory or disk (no run); kept in memory as the newest.
    pub(crate) fn cached(&self, key: &str) -> Option<Arc<lightcraft_parsing::Parsing>> {
        let p = self.remembered(key).or_else(|| self.filed(key))?;
        self.remember(key, p.clone());
        Some(p)
    }

    /// The version parses of the chosen set are filed under.
    pub(crate) fn ahead_version(&self) -> String {
        self.model_set().version()
    }
}

/// What the parse files on disk may take together.
pub(crate) const fn cache_budget() -> u64 {
    CACHE_BYTES
}

/// Mark the file at `path` as just used; a missing or read-only file is left as it is.
fn touch(path: &std::path::Path) {
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}
