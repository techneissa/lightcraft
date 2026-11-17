//! One rule for what the app keeps in memory for work it may not do again soon (issue #304, `fork/idle-memory.md`):
//! AI models and caches that cost a lot to hold and little to make again are let go after [`MODEL_IDLE`] without use
//! ([`BACKGROUND_IDLE`] while the window is in the background), and the memory the allocator keeps after big frees goes
//! back to the system once the work that made it has been quiet for [`TRIM_QUIET`].
//!
//! A holder [`register`]s once with a function that lets its memory go, and marks each use: [`Tracker::using`] around a
//! run (never unloaded meanwhile; the clock starts when it ends) or [`Tracker::touch`] when it has no run to bracket.
//! One thread, started by the first holder, sleeps until the nearest deadline (nothing is polled: with nothing loaded it
//! sleeps for good) and calls the holder's function when its time is up. The function reports whether it freed
//! something, nothing, or found the holder in use (it is asked again after a whole new idle time).
//!
//! The decisions are in [`Registry::sweep`], which takes the time as an argument so tests need no clock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use web_time::Instant;

/// How long a model, or a cache that is cheap to make again, stays after its last use.
pub const MODEL_IDLE: Duration = Duration::from_secs(60);
/// The same while the window is in the background (no window in front, or minimized): nobody is waiting on it.
pub const BACKGROUND_IDLE: Duration = Duration::from_secs(15);
/// How long the app must have done no big work before the allocator's free pages go back to the system.
pub const TRIM_QUIET: Duration = Duration::from_secs(4);

/// What a holder's unload function found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unload {
    /// It let memory go.
    Freed,
    /// There was nothing loaded.
    Nothing,
    /// It was in use after all (its lock is held): asked again after another idle time.
    Busy,
}

struct EntryState {
    /// Something may be loaded (a use began since the last unload).
    armed: bool,
    /// Runs in progress ([`Tracker::using`]).
    busy: usize,
    last_used: Instant,
}

struct Entry {
    name: &'static str,
    unload: Box<dyn Fn() -> Unload + Send + Sync>,
    state: Mutex<EntryState>,
}

#[derive(Default)]
struct Inner {
    entries: Vec<Arc<Entry>>,
    /// The last big work (a decode, a render), until its memory was given back.
    last_work: Option<Instant>,
    /// Counts every change the thread must look at again.
    generation: u64,
}

/// The holders and what to do for them. The app has one ([`global`]); tests make their own.
pub struct Registry {
    inner: Mutex<Inner>,
    cv: Condvar,
    background: AtomicBool,
}

/// What one pass decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    /// The holders that let memory go.
    pub freed: Vec<&'static str>,
    /// When to look again; `None`: when something happens.
    pub next: Option<Instant>,
    /// Whether the allocator's free pages were asked back.
    pub trimmed: bool,
}

/// A holder's handle: marks its uses.
#[derive(Clone)]
pub struct Tracker {
    entry: Arc<Entry>,
    registry: Arc<Registry>,
}

/// A run in progress; the holder is not unloaded while one lives.
pub struct Use {
    tracker: Tracker,
}

fn soonest(t: Instant, next: &mut Option<Instant>) {
    *next = Some(next.map_or(t, |n| n.min(t)));
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Registry {
    pub fn new() -> Arc<Registry> {
        Arc::new(Registry { inner: Mutex::new(Inner::default()), cv: Condvar::new(), background: AtomicBool::new(false) })
    }

    /// Add a holder. `unload` lets its memory go (it must not wait for a lock a run holds: `try_lock`, and
    /// [`Unload::Busy`] when it is taken).
    pub fn register(self: &Arc<Self>, name: &'static str, unload: impl Fn() -> Unload + Send + Sync + 'static) -> Tracker {
        let state = EntryState { armed: false, busy: 0, last_used: Instant::now() };
        let entry = Arc::new(Entry { name, unload: Box::new(unload), state: Mutex::new(state) });
        lock(&self.inner).entries.push(entry.clone());
        Tracker { entry, registry: self.clone() }
    }

    /// Say whether the window is in the background: holders go after [`BACKGROUND_IDLE`] instead of [`MODEL_IDLE`].
    pub fn set_background(&self, background: bool) {
        if self.background.swap(background, Ordering::AcqRel) != background {
            self.wake();
        }
    }

    /// Big work happened (memory was held and let go): the allocator's pages are asked back once it is quiet.
    pub fn note_work_at(&self, at: Instant) {
        lock(&self.inner).last_work = Some(at);
        self.wake();
    }

    fn wake(&self) {
        lock(&self.inner).generation += 1;
        self.cv.notify_all();
    }

    fn idle(&self) -> Duration {
        if self.background.load(Ordering::Acquire) { BACKGROUND_IDLE } else { MODEL_IDLE }
    }

    /// Unload what has been idle at `now`, ask the allocator's pages back (`trim`) when work has been quiet, and say when
    /// to look again.
    pub fn sweep(&self, now: Instant, trim: &dyn Fn()) -> Sweep {
        let idle = self.idle();
        let entries: Vec<Arc<Entry>> = lock(&self.inner).entries.clone();
        let mut out = Sweep::default();
        for e in entries {
            let due = {
                let s = lock(&e.state);
                if !s.armed || s.busy > 0 {
                    continue;
                }
                s.last_used + idle
            };
            if now < due {
                soonest(due, &mut out.next);
                continue;
            }
            // (the function runs with no lock of ours held: dropping a model takes a while)
            match (e.unload)() {
                Unload::Freed => {
                    lock(&e.state).armed = false;
                    out.freed.push(e.name);
                }
                Unload::Nothing => lock(&e.state).armed = false,
                Unload::Busy => {
                    log::debug!("{} is in use: asked again after {idle:?}", e.name);
                    lock(&e.state).last_used = now;
                    soonest(now + idle, &mut out.next);
                }
            }
        }
        let mut inner = lock(&self.inner);
        if let Some(w) = inner.last_work {
            if now >= w + TRIM_QUIET || !out.freed.is_empty() {
                inner.last_work = None;
                out.trimmed = true;
            } else {
                soonest(w + TRIM_QUIET, &mut out.next);
            }
        } else if !out.freed.is_empty() {
            out.trimmed = true;
        }
        drop(inner);
        if out.trimmed {
            trim();
        }
        out
    }

    /// The thread's loop: sweep, then sleep to the next deadline or until something changes.
    #[cfg(not(target_arch = "wasm32"))]
    fn run(&self, trim: &dyn Fn()) {
        loop {
            let seen = lock(&self.inner).generation;
            let sweep = self.sweep(Instant::now(), trim);
            for name in &sweep.freed {
                log::info!("{name} let go after a while without use");
            }
            let guard = lock(&self.inner);
            // (a poisoned lock is looked at again on the next pass)
            match sweep.next {
                Some(at) => {
                    drop(self.cv.wait_timeout_while(guard, at.saturating_duration_since(Instant::now()), |i| i.generation == seen));
                }
                None => {
                    drop(self.cv.wait_while(guard, |i| i.generation == seen));
                }
            }
        }
    }
}

impl Tracker {
    /// The holder is loaded and was used now (cheap enough for every frame).
    pub fn touch(&self) {
        self.touch_at(Instant::now());
    }

    fn touch_at(&self, at: Instant) {
        let was_armed = {
            let mut s = lock(&self.entry.state);
            let was = std::mem::replace(&mut s.armed, true);
            s.last_used = at;
            was
        };
        // (a holder already waited for only moves its deadline later: the thread finds that out when it wakes, so a
        // busy loupe touching it every frame costs the thread nothing)
        if !was_armed {
            self.registry.wake();
        }
    }

    /// A run begins: the holder stays loaded until the returned guard is dropped, and its idle time counts from then.
    pub fn using(&self) -> Use {
        {
            let mut s = lock(&self.entry.state);
            s.armed = true;
            s.busy += 1;
        }
        Use { tracker: self.clone() }
    }
}

impl Drop for Use {
    fn drop(&mut self) {
        {
            let mut s = lock(&self.tracker.entry.state);
            s.busy = s.busy.saturating_sub(1);
            s.last_used = Instant::now();
        }
        self.tracker.registry.wake();
    }
}

/// The app's registry, with its thread started.
pub fn global() -> &'static Arc<Registry> {
    static GLOBAL: OnceLock<Arc<Registry>> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        let registry = Registry::new();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let again = registry.clone();
            // (a thread that cannot start leaves models loaded, as before)
            let _ = std::thread::Builder::new().name("lc-idle".into()).spawn(move || again.run(&crate::memory::release));
        }
        registry
    })
}

/// Add a holder to the app's registry.
pub fn register(name: &'static str, unload: impl Fn() -> Unload + Send + Sync + 'static) -> Tracker {
    global().register(name, unload)
}

/// Big work happened now: the allocator's free pages go back to the system once it has been quiet.
pub fn note_work() {
    global().note_work_at(Instant::now());
}

/// The window went to the background or came back.
pub fn set_background(background: bool) {
    global().set_background(background);
}

impl crate::Session {
    /// The window has been in the background a while: let go of what the photo in front can do without (the full-size
    /// originals; the photo's own previews and renders stay).
    pub fn release_for_background(&mut self) {
        self.media.release_originals();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A holder whose "model" is a flag: loaded, and how many times it was let go.
    struct Fake {
        loaded: Arc<AtomicBool>,
        unloads: Arc<AtomicUsize>,
        busy: Arc<AtomicBool>,
    }

    fn fake(reg: &Arc<Registry>) -> (Fake, Tracker) {
        let (loaded, unloads, busy) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
        let (l, u, b) = (loaded.clone(), unloads.clone(), busy.clone());
        let t = reg.register("fake", move || {
            if b.load(Ordering::SeqCst) {
                Unload::Busy
            } else if l.swap(false, Ordering::SeqCst) {
                u.fetch_add(1, Ordering::SeqCst);
                Unload::Freed
            } else {
                Unload::Nothing
            }
        });
        (Fake { loaded, unloads, busy }, t)
    }

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn a_model_goes_after_its_idle_time_and_loads_again_on_the_next_use() {
        let reg = Registry::new();
        let (m, t) = fake(&reg);
        let t0 = Instant::now();
        m.loaded.store(true, Ordering::SeqCst);
        t.touch_at(t0);
        let nothing = || {};
        assert!(reg.sweep(at(t0, MODEL_IDLE.as_secs() - 1), &nothing).freed.is_empty(), "not before its time");
        assert_eq!(reg.sweep(at(t0, MODEL_IDLE.as_secs()), &nothing).freed, vec!["fake"]);
        assert!(!m.loaded.load(Ordering::SeqCst));
        assert!(reg.sweep(at(t0, 10 * MODEL_IDLE.as_secs()), &nothing).freed.is_empty(), "nothing loaded, nothing to free");
        // the next use loads it again, and it goes again after its own idle time
        let t1 = at(t0, 1000);
        m.loaded.store(true, Ordering::SeqCst);
        t.touch_at(t1);
        assert!(reg.sweep(at(t1, 1), &nothing).freed.is_empty());
        assert_eq!(reg.sweep(at(t1, MODEL_IDLE.as_secs()), &nothing).freed, vec!["fake"]);
        assert_eq!(m.unloads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_run_in_progress_keeps_its_model_and_the_clock_starts_when_it_ends() {
        let reg = Registry::new();
        let (m, t) = fake(&reg);
        let t0 = Instant::now();
        m.loaded.store(true, Ordering::SeqCst);
        let run = t.using();
        assert!(reg.sweep(at(t0, 100 * MODEL_IDLE.as_secs()), &|| {}).freed.is_empty(), "never while it runs");
        drop(run);
        let ended = Instant::now();
        assert!(reg.sweep(at(ended, MODEL_IDLE.as_secs() - 1), &|| {}).freed.is_empty(), "the idle time counts from the end");
        assert_eq!(reg.sweep(at(ended, MODEL_IDLE.as_secs()), &|| {}).freed, vec!["fake"]);
    }

    #[test]
    fn a_holder_found_in_use_is_asked_again_after_a_whole_idle_time() {
        let reg = Registry::new();
        let (m, t) = fake(&reg);
        let t0 = Instant::now();
        m.loaded.store(true, Ordering::SeqCst);
        m.busy.store(true, Ordering::SeqCst);
        t.touch_at(t0);
        let due = at(t0, MODEL_IDLE.as_secs());
        let s = reg.sweep(due, &|| {});
        assert!(s.freed.is_empty());
        assert_eq!(s.next, Some(due + MODEL_IDLE));
        m.busy.store(false, Ordering::SeqCst);
        assert_eq!(reg.sweep(due + MODEL_IDLE, &|| {}).freed, vec!["fake"]);
    }

    #[test]
    fn the_background_shortens_the_time() {
        let reg = Registry::new();
        let (m, t) = fake(&reg);
        let t0 = Instant::now();
        m.loaded.store(true, Ordering::SeqCst);
        t.touch_at(t0);
        reg.set_background(true);
        assert_eq!(reg.sweep(at(t0, BACKGROUND_IDLE.as_secs()), &|| {}).freed, vec!["fake"]);
    }

    #[test]
    fn the_allocators_pages_go_back_once_the_work_is_quiet() {
        let reg = Registry::new();
        let t0 = Instant::now();
        let trims = AtomicUsize::new(0);
        let trim = || {
            trims.fetch_add(1, Ordering::SeqCst);
        };
        reg.note_work_at(t0);
        let s = reg.sweep(at(t0, 1), &trim);
        assert!(!s.trimmed);
        assert_eq!(s.next, Some(t0 + TRIM_QUIET));
        // more work pushes it back
        reg.note_work_at(at(t0, 3));
        assert!(!reg.sweep(at(t0, 5), &trim).trimmed);
        assert!(reg.sweep(at(t0, 3 + TRIM_QUIET.as_secs()), &trim).trimmed);
        assert_eq!(trims.load(Ordering::SeqCst), 1);
        assert!(!reg.sweep(at(t0, 100), &trim).trimmed, "once per burst of work");
    }
}
