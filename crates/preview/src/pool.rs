//! A prioritized job pool with per-slot de-duplication.
//!
//! Each job targets a *slot* (a grid cell, the loupe…). Submitting for a slot replaces a job for
//! that slot that hasn't started yet, so scrolling or dragging never builds a backlog. Workers
//! take the highest priority first (newest first among equals). Frontends re-prioritize or drop
//! queued jobs when the view changes (e.g. thumbnails scrolled out of view).
//!
//! Native: a fixed set of worker threads, started on first use. wasm32 (no threads): the host
//! calls [`JobPool::run_inline`] once per frame.
//!
//! No worker outlives its pool by more than a deadline (issue #620: a render still inside the GPU
//! driver while the process exits crashes there): [`JobPool::shutdown`] and dropping the pool
//! discard the queued jobs and wait, bounded, for the running ones.

use std::hash::Hash;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub type Job<R> = Box<dyn FnOnce() -> R + Send>;

/// What a slot gets instead of a result when its job panics: the result of a failed job, given the
/// panic's message (see [`JobPool::submit_or`]).
pub type Failure<R> = Box<dyn FnOnce(String) -> R + Send>;

/// What a worker calls when a job has finished (a frontend's "repaint now").
pub type Waker = Arc<dyn Fn() + Send + Sync>;

struct Entry<S, R> {
    slot: S,
    key: u64,
    priority: u32,
    seq: u64,
    job: Job<R>,
    fail: Option<Failure<R>>,
}

struct Shared<S, R> {
    queue: Mutex<Vec<Entry<S, R>>>,
    /// Slots with a job running: their next job waits until it ends, so a slot never has two at
    /// once (a slider drag submits faster than renders finish; overlapping renders of one view
    /// were mostly thrown away, and took the GPU from the one that is shown). Locked inside
    /// `queue`. Each with its run's number (see `released`).
    busy: Mutex<Vec<(S, u64)>>,
    /// Runs let go of their slot while they go on ([`JobPool::release_running`]): the slot's next job starts beside
    /// them, and their results are dropped. Locked inside `queue`.
    released: Mutex<Vec<u64>>,
    /// The number the next run takes.
    runs: std::sync::atomic::AtomicU64,
    cv: Condvar,
    /// Called by a worker after each finished job (see [`JobPool::set_waker`]).
    waker: Mutex<Option<Waker>>,
    shutdown: AtomicBool,
    /// The worker threads that have not ended yet, and the signal that one did.
    #[cfg(not(target_arch = "wasm32"))]
    live: Mutex<Vec<std::thread::ThreadId>>,
    #[cfg(not(target_arch = "wasm32"))]
    ended: Condvar,
}

/// How long dropping a pool waits for the jobs still running (see [`JobPool::shutdown`]).
const DROP_WAIT: Duration = Duration::from_secs(2);

/// A finished job.
pub struct Done<S, R> {
    pub slot: S,
    /// The key the job was submitted with (to drop stale results).
    pub key: u64,
    pub result: R,
    /// Run time in milliseconds (0 on wasm).
    pub ms: f64,
}

pub struct JobPool<S, R> {
    shared: Arc<Shared<S, R>>,
    tx: Sender<Done<S, R>>,
    rx: Receiver<Done<S, R>>,
    /// A finished job [`Self::wait`] took off the channel, handed out first by [`Self::try_recv`].
    early: Option<Done<S, R>>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    threads: usize,
    started: bool,
    seq: u64,
    #[cfg(not(target_arch = "wasm32"))]
    workers: Vec<std::thread::JoinHandle<()>>,
    /// [`JobPool::shutdown`] ran: dropping the pool doesn't wait a second time.
    stopped: bool,
}

impl<S: Copy + Eq + Hash + Send + 'static, R: Send + 'static> JobPool<S, R> {
    /// A pool with `threads` workers (none on wasm; `0` = jobs only run via [`JobPool::run_inline`]).
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = channel();
        JobPool {
            shared: Arc::new(Shared {
                queue: Mutex::new(Vec::new()),
                busy: Mutex::new(Vec::new()),
                released: Mutex::new(Vec::new()),
                runs: std::sync::atomic::AtomicU64::new(0),
                cv: Condvar::new(),
                waker: Mutex::new(None),
                shutdown: AtomicBool::new(false),
                #[cfg(not(target_arch = "wasm32"))]
                live: Mutex::new(Vec::new()),
                #[cfg(not(target_arch = "wasm32"))]
                ended: Condvar::new(),
            }),
            tx,
            rx,
            early: None,
            threads,
            started: false,
            seq: 0,
            #[cfg(not(target_arch = "wasm32"))]
            workers: Vec::new(),
            stopped: false,
        }
    }

    /// Have the workers call `wake` after every job they finish, so a frontend that sleeps between
    /// frames wakes for the result instead of polling [`Self::try_recv`] on a timer.
    pub fn set_waker(&self, wake: Waker) {
        *self.shared.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(wake);
    }

    /// Workers for this machine: cores − 1, between 2 and 8.
    pub fn default_threads() -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(4).clamp(2, 8)
        }
        #[cfg(target_arch = "wasm32")]
        {
            1
        }
    }

    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            // held while spawning: a worker that ends at once still finds its id listed
            let mut live = self.shared.live.lock().unwrap_or_else(|e| e.into_inner());
            for i in 0..self.threads {
                let shared = self.shared.clone();
                let tx = self.tx.clone();
                let spawned = std::thread::Builder::new().name(format!("lc-job-{i}")).spawn(move || {
                    let _ended = Ended(&shared);
                    while let Some((e, run)) = take(&shared, true) {
                        let t0 = std::time::Instant::now();
                        // the slot is free again when the job ends, also if it panics
                        let _free = Free(&shared, e.slot, run);
                        let Some(result) = run_guarded(e.job, e.fail) else { continue };
                        // (let go of: nobody waits for it)
                        if is_released(&shared, run) {
                            continue;
                        }
                        let done = Done { slot: e.slot, key: e.key, result, ms: t0.elapsed().as_secs_f64() * 1000.0 };
                        if tx.send(done).is_err() {
                            break;
                        }
                        // (the result is on the channel first: the woken frame finds it)
                        let waker = shared.waker.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        if let Some(wake) = waker {
                            wake();
                        }
                    }
                });
                match spawned {
                    Ok(h) => {
                        live.push(h.thread().id());
                        self.workers.push(h);
                    }
                    Err(e) => log::error!("job pool: worker {i} could not start: {e}"),
                }
            }
        }
    }

    /// Stop the pool: queued jobs are dropped, no new one starts, and the call waits up to
    /// `timeout` for the jobs that are running. Returns whether every worker has ended; one that
    /// hasn't is left to finish on its own. For owners that are about to end the process, where
    /// dropping the pool may come too late or never (`std::process::exit`).
    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        self.stopped = true;
        #[cfg(not(target_arch = "wasm32"))]
        {
            stop(&self.shared, &mut self.workers, timeout)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = timeout;
            close(&self.shared);
            true
        }
    }

    /// Worker threads that have not ended (0 before the first job, and on wasm).
    pub fn live_workers(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.shared.live.lock().map(|l| l.len()).unwrap_or(0)
        }
        #[cfg(target_arch = "wasm32")]
        {
            0
        }
    }

    /// Queue `job` for `slot`, replacing a queued (not yet running) job for the same slot. If the job
    /// panics, nothing is sent (see [`Self::submit_or`]): for a caller that waits for the result,
    /// use that instead.
    pub fn submit(&mut self, slot: S, key: u64, priority: u32, job: Job<R>) {
        self.queue_job(slot, key, priority, job, None);
    }

    /// [`Self::submit`], and if `job` panics the slot still gets a result: `fail(message)` is sent
    /// under the same slot and key, so whoever waits for the job learns it failed instead of
    /// waiting for ever. The worker lives on and the slot is free for its next job.
    pub fn submit_or(&mut self, slot: S, key: u64, priority: u32, job: Job<R>, fail: Failure<R>) {
        self.queue_job(slot, key, priority, job, Some(fail));
    }

    fn queue_job(&mut self, slot: S, key: u64, priority: u32, job: Job<R>, fail: Option<Failure<R>>) {
        if self.stopped {
            return; // nothing runs it any more
        }
        self.start();
        self.seq += 1;
        let (lock, cv) = (&self.shared.queue, &self.shared.cv);
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.retain(|e| e.slot != slot);
        q.push(Entry { slot, key, priority, seq: self.seq, job, fail });
        cv.notify_one();
    }

    /// Change queued jobs' priorities: `f(slot, priority)` returns the new priority, or `None` to
    /// drop the job. Returns the dropped slots.
    pub fn reprioritize(&self, mut f: impl FnMut(&S, u32) -> Option<u32>) -> Vec<S> {
        let mut dropped = Vec::new();
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.retain_mut(|e| match f(&e.slot, e.priority) {
            Some(p) => {
                e.priority = p;
                true
            }
            None => {
                dropped.push(e.slot);
                false
            }
        });
        dropped
    }

    /// Let the job running for `slot` go on without its slot: the slot's next job starts at once beside it, instead of
    /// waiting for it to end, and its result is dropped when it does. For a job whose result nobody will use, told to
    /// stop by its caller, that can't stop until it reaches its next check (a decode in progress). At most `most` let go
    /// of are running at once, so stepping fast can't pile them up. Whether one was let go of (not: no job of `slot` is
    /// running, or `most` already are).
    pub fn release_running(&self, slot: S, most: usize) -> bool {
        {
            // (under the queue lock, as `take` looks at `busy`)
            let _q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            let mut busy = self.shared.busy.lock().unwrap_or_else(|e| e.into_inner());
            let mut released = self.shared.released.lock().unwrap_or_else(|e| e.into_inner());
            let Some(i) = busy.iter().position(|(s, _)| *s == slot) else { return false };
            if released.len() >= most {
                return false;
            }
            let (_, run) = busy.swap_remove(i);
            released.push(run);
        }
        self.shared.cv.notify_all();
        true
    }

    /// Jobs let go of ([`Self::release_running`]) still running.
    pub fn released_running(&self) -> usize {
        self.shared.released.lock().map(|r| r.len()).unwrap_or(0)
    }

    /// Queued (not started) jobs.
    pub fn queued(&self) -> usize {
        self.shared.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Is a job for `slot` queued?
    pub fn is_queued(&self, slot: S) -> bool {
        self.shared.queue.lock().map(|q| q.iter().any(|e| e.slot == slot)).unwrap_or(false)
    }

    /// A finished job, if any.
    pub fn try_recv(&mut self) -> Option<Done<S, R>> {
        self.early.take().or_else(|| self.rx.try_recv().ok())
    }

    /// Wait up to `timeout` for a job to finish; whether one has (it is the next [`Self::try_recv`]).
    pub fn wait(&mut self, timeout: Duration) -> bool {
        if self.early.is_none() {
            self.early = self.rx.recv_timeout(timeout).ok();
        }
        self.early.is_some()
    }

    /// Run up to `n` jobs on the calling thread (wasm, tests). Returns how many ran.
    pub fn run_inline(&mut self, n: usize) -> usize {
        let mut ran = 0;
        while ran < n {
            let Some((e, run)) = take(&self.shared, false) else { break };
            let _free = Free(&self.shared, e.slot, run);
            if let Some(result) = run_guarded(e.job, e.fail)
                && !is_released(&self.shared, run)
            {
                let _ = self.tx.send(Done { slot: e.slot, key: e.key, result, ms: 0.0 });
            }
            ran += 1;
        }
        ran
    }
}

/// Run `job`; a panic is its failure: `fail`'s result (none without one, or when that panics too).
/// On wasm32 (built with `panic = "abort"`) nothing can be caught and this just runs the job.
fn run_guarded<R>(job: Job<R>, fail: Option<Failure<R>>) -> Option<R> {
    let payload = match catch_unwind(AssertUnwindSafe(job)) {
        Ok(result) => return Some(result),
        Err(payload) => payload,
    };
    let message = format!("a job panicked: {}", panic_message(payload.as_ref()));
    // (a job with a fallback hands the message to its result, which the caller reports; only a job without one is
    // logged here)
    if fail.is_none() {
        log::error!("job pool: {message}");
    }
    catch_unwind(AssertUnwindSafe(move || fail.map(|f| f(message)))).unwrap_or(None)
}

/// The text of a panic payload (`panic!("…")` gives a `&str` or a `String`).
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    match (payload.downcast_ref::<&str>(), payload.downcast_ref::<String>()) {
        (Some(s), _) => (*s).to_string(),
        (_, Some(s)) => s.clone(),
        _ => "unknown panic".into(),
    }
}

/// Pop the best job (highest priority, newest first), with its run's number. Blocks when `wait` until one is queued
/// or the pool shuts down.
fn take<S: PartialEq + Copy, R>(shared: &Shared<S, R>, wait: bool) -> Option<(Entry<S, R>, u64)> {
    let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            return None;
        }
        // (a slot with a job running waits for it: see `Shared::busy`)
        let best = {
            let busy = shared.busy.lock().unwrap_or_else(|e| e.into_inner());
            q.iter().enumerate().filter(|(_, e)| !busy.iter().any(|(s, _)| *s == e.slot)).max_by_key(|(_, e)| (e.priority, e.seq)).map(|(i, _)| i)
        };
        if let Some(i) = best {
            let e = q.swap_remove(i);
            let run = shared.runs.fetch_add(1, Ordering::Relaxed);
            shared.busy.lock().unwrap_or_else(|e| e.into_inner()).push((e.slot, run));
            return Some((e, run));
        }
        if !wait {
            return None;
        }
        q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
    }
}

/// Whether run `run` was let go of ([`JobPool::release_running`]): its result is dropped.
fn is_released<S, R>(shared: &Shared<S, R>, run: u64) -> bool {
    shared.released.lock().map(|r| r.contains(&run)).unwrap_or(false)
}

/// Run `run` of `slot` ended: a job queued for the slot may start (one let go of had freed it already).
fn finished<S: PartialEq, R>(shared: &Shared<S, R>, slot: &S, run: u64) {
    {
        // under the queue lock: a worker is either before its look at `busy` or waiting
        let _q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        let mut busy = shared.busy.lock().unwrap_or_else(|e| e.into_inner());
        let mut released = shared.released.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = released.iter().position(|r| *r == run) {
            released.swap_remove(i);
        } else if let Some(i) = busy.iter().position(|(s, r)| s == slot && *r == run) {
            busy.swap_remove(i);
        }
    }
    shared.cv.notify_all();
}

/// Frees a job's slot when the job ends (see [`finished`]).
struct Free<'a, S: PartialEq, R>(&'a Shared<S, R>, S, u64);

impl<S: PartialEq, R> Drop for Free<'_, S, R> {
    fn drop(&mut self) {
        finished(self.0, &self.1, self.2);
    }
}

/// Takes a worker off the live list when its thread ends (also when a job panicked).
#[cfg(not(target_arch = "wasm32"))]
struct Ended<'a, S, R>(&'a Shared<S, R>);

#[cfg(not(target_arch = "wasm32"))]
impl<S, R> Drop for Ended<'_, S, R> {
    fn drop(&mut self) {
        let me = std::thread::current().id();
        self.0.live.lock().unwrap_or_else(|e| e.into_inner()).retain(|id| *id != me);
        self.0.ended.notify_all();
    }
}

/// Tell the workers to stop and take the queued jobs away from them.
fn close<S, R>(shared: &Shared<S, R>) {
    // the flag changes under the queue lock: a worker is either before its check or waiting
    let queued = {
        let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        shared.shutdown.store(true, Ordering::Relaxed);
        std::mem::take(&mut *q)
    };
    shared.cv.notify_all();
    drop(queued); // outside the lock: a job may own a lot
}

/// [`JobPool::shutdown`]: wait up to `timeout` for the workers to end, without holding the queue
/// lock, and never for the calling thread (a pool dropped by one of its own jobs).
#[cfg(not(target_arch = "wasm32"))]
fn stop<S, R>(shared: &Shared<S, R>, workers: &mut Vec<std::thread::JoinHandle<()>>, timeout: Duration) -> bool {
    close(shared);
    let me = std::thread::current().id();
    let t0 = std::time::Instant::now();
    let mut live = shared.live.lock().unwrap_or_else(|e| e.into_inner());
    while live.iter().any(|id| *id != me) {
        let Some(left) = timeout.checked_sub(t0.elapsed()).filter(|d| !d.is_zero()) else { break };
        live = shared.ended.wait_timeout(live, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    let running: Vec<std::thread::ThreadId> = live.iter().copied().filter(|id| *id != me).collect();
    drop(live);
    for h in workers.drain(..) {
        let id = h.thread().id();
        // a worker off the list is past its last job: joining it only waits for the thread to unwind
        if id != me && !running.contains(&id) {
            let _ = h.join();
        }
    }
    if !running.is_empty() {
        log::warn!("job pool: {} job(s) still running after {timeout:?}; not waiting for them", running.len());
    }
    running.is_empty()
}

impl<S, R> Drop for JobPool<S, R> {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        stop(&self.shared, &mut self.workers, DROP_WAIT);
        #[cfg(target_arch = "wasm32")]
        close(&self.shared);
    }
}
