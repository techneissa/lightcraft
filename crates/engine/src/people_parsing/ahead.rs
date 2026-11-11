//! Fork feature (`fork/parse-ahead.md`): what the parse-ahead queue ([`crate::parse_ahead`]) needs from people
//! parsing. A parse asked for from the queue's worker thread goes through [`PeopleParsing::parse`] like any other (the
//! same cache key, the same run, a foreground caller that wants the same photo joins it), with two differences:
//! it has no activity row of its own (the queue keeps one row for all its photos), and the queue learns the key and
//! version the parse is filed under and the run's cancel flag, so it can skip the photo next time and stop the run
//! when the user needs the machine. A request for another photo stops such a run at once (it holds the models); a
//! stopped one keeps no result and leaves no message.
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
    /// The version the parse is filed under ([`Filed::version`]).
    version: Option<String>,
    flag: Option<Arc<AtomicBool>>,
    /// Stop the run as soon as it starts (asked for before its flag was known).
    stop: bool,
}

impl QuietRun {
    /// The cache key the parse looked up (known once the parse has started).
    pub(crate) fn key(&self) -> Option<String> {
        lock(&self.slot).key.clone()
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

    /// The version the parse is filed under (known once the parse has started).
    pub(crate) fn version(&self) -> Option<String> {
        lock(&self.slot).version.clone()
    }

    /// The parse looks up `key`, filed under `version`.
    pub(super) fn note(&self, key: &str, version: &str) {
        let mut s = lock(&self.slot);
        s.key = Some(key.to_string());
        s.version = Some(version.to_string());
    }

    /// Tests: whether a stop waits for the run to start.
    #[cfg(test)]
    pub(crate) fn stop_pending(&self) -> bool {
        lock(&self.slot).stop
    }
}

/// Where a parse is filed: the version ([`PeopleParsing::ahead_version`] at the time) and the cache key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Filed {
    pub version: String,
    pub key: String,
}

thread_local! {
    /// Set while this thread runs a quiet parse.
    static QUIET: RefCell<Option<QuietRun>> = const { RefCell::new(None) };
}

pub(super) fn quiet() -> Option<QuietRun> {
    QUIET.with(|q| q.borrow().clone())
}

impl PeopleParsing {
    /// [`Self::parse`] of `img` with `hints`, waiting, for the parse-ahead queue: no row of its own; `run` learns the
    /// key, the version and the cancel flag. A run stopped through `run`, or by a request for another photo, ends as
    /// `Failed("cancelled")`; it leaves no message and no result.
    pub(crate) fn parse_quietly(&self, img: RgbImage, hints: Vec<FaceHint>, run: &QuietRun) -> Outcome {
        QUIET.with(|q| *q.borrow_mut() = Some(run.clone()));
        let out = self.parse(img, hints, true);
        QUIET.with(|q| *q.borrow_mut() = None);
        out
    }

    /// Hook of [`Self::remembered`]: every lookup of `key` marks its cache file as just used.
    pub(super) fn looked_up(&self, key: &str) {
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

    /// The version a parse is filed under now: the chosen set's, with the People edge's when it is not Current (the
    /// one [`Self::parse`] keys on).
    pub(crate) fn ahead_version(&self) -> String {
        self.choice().version()
    }

    /// Tests: file a finished run of `key` without caching its parse (a run that ended a moment ago).
    #[cfg(test)]
    pub(crate) fn test_finished(&self, key: &str, p: lightcraft_parsing::Parsing) {
        lock(&self.inner.jobs).finish(key.to_string(), Ok(Arc::new(p)));
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
