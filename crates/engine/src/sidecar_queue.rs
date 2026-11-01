//! Automatic XMP sidecar writes off the UI thread. With auto-write on, every edit rewrites the sidecars of the photos
//! it changed; a bulk change on hundreds of photos (or a sidecar on a slow share) held the window up for the whole
//! batch. With the queue on ([`Session::queue_sidecar_writes`], the desktop app), the UI thread only gathers each
//! photo's packet and paths (cheap); one worker reads, merges and writes the files in order, under one row in the
//! activity stack ("Writing XMP sidecars"; the stack shows it only once the batch takes more than a moment). A newer
//! write for a photo replaces its waiting one. Each result comes back to the UI thread
//! ([`Session::poll_sidecar_writes`]), which notes the write and keeps a failed photo unsaved, so the retry still works.
//!
//! Anything else that reads, writes or moves sidecars waits for the queue first ([`Session::finish_sidecar_writes`]):
//! Save and Read Metadata, import and Synchronize Folder scans, rename and move, opening another library, quitting.
//! So a queued write never lands after a newer one, and never puts old data back. Without the queue (tests, the CLI,
//! MCP), the writes stay synchronous as before. Dropping the queue finishes what is in it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;

use crate::activity::{Activity, Cancel, TaskGuard};

/// The activity row of the queue.
pub const LABEL: &str = "Writing XMP sidecars";

/// One photo's sidecar write, gathered on the UI thread.
pub(crate) struct Job {
    pub id: PhotoId,
    pub file_name: String,
    /// The original, absolute.
    pub original: String,
    /// Where a new sidecar goes, and every existing name it may already have (the existing one's spelling wins).
    pub preferred: PathBuf,
    pub candidates: Vec<PathBuf>,
    /// The library's packet for the photo, merged into what the file holds.
    pub packet: String,
    /// The clock's time, for a backup's name.
    pub stamp: String,
}

/// What became of a [`Job`]: the sidecar written and its bytes, or why not.
pub(crate) struct Written {
    pub id: PhotoId,
    pub result: Result<(PathBuf, Vec<u8>), String>,
}

#[derive(Default)]
struct State {
    waiting: VecDeque<Job>,
    /// A job is being written.
    busy: bool,
    done: Vec<Written>,
    closed: bool,
    /// Jobs taken since the queue was last empty, and how many of them are written.
    total: u64,
    finished: u64,
    /// Tests: the worker takes nothing until someone waits for the queue.
    #[cfg(test)]
    held: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, State> {
    s.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The queue and its worker (started with the first write).
#[derive(Default)]
pub struct SidecarQueue {
    shared: Shared,
    worker: Option<std::thread::JoinHandle<()>>,
    row: Option<TaskGuard>,
}

impl SidecarQueue {
    /// Queue `job`, replacing a waiting one for the same photo (it would write older data). False when no worker
    /// can start: the caller writes it in place.
    pub(crate) fn push(&mut self, job: Job, activity: &Activity) -> bool {
        if self.worker.is_none() {
            let shared = self.shared.clone();
            match std::thread::Builder::new().name("xmp-sidecars".into()).spawn(move || work(&shared)) {
                Ok(h) => self.worker = Some(h),
                Err(_) => return false,
            }
        }
        {
            let mut s = lock(&self.shared);
            match s.waiting.iter_mut().find(|j| j.id == job.id) {
                Some(slot) => *slot = job,
                None => {
                    s.waiting.push_back(job);
                    s.total += 1;
                }
            }
        }
        self.shared.1.notify_all();
        let row = self.row.get_or_insert_with(|| activity.start("xmp", LABEL, Cancel::No));
        let s = lock(&self.shared);
        row.progress(s.finished, s.total);
        true
    }

    /// Writes queued or in progress.
    pub fn pending(&self) -> usize {
        let s = lock(&self.shared);
        s.waiting.len() + usize::from(s.busy)
    }

    /// Tests: hold the worker until the queue is waited for, so a test sees writes still queued.
    #[cfg(test)]
    pub(crate) fn hold(&self) {
        lock(&self.shared).held = true;
    }

    /// Block until every queued write is done.
    pub fn wait(&self) {
        let mut s = lock(&self.shared);
        #[cfg(test)]
        {
            s.held = false;
            self.shared.1.notify_all();
        }
        while !s.waiting.is_empty() || s.busy {
            if self.worker.is_none() {
                return;
            }
            s = self.shared.1.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The results so far, in the order they were written; the row follows, and goes once the queue is empty.
    pub(crate) fn take_done(&mut self) -> Vec<Written> {
        let mut s = lock(&self.shared);
        let done = std::mem::take(&mut s.done);
        let idle = s.waiting.is_empty() && !s.busy;
        if let Some(row) = &self.row {
            row.progress(s.finished, s.total);
        }
        if idle {
            (s.total, s.finished) = (0, 0);
            drop(s);
            self.row = None;
        }
        done
    }
}

impl Drop for SidecarQueue {
    /// Finish what is queued (none is lost), then stop the worker.
    fn drop(&mut self) {
        {
            let mut s = lock(&self.shared);
            s.closed = true;
            #[cfg(test)]
            {
                s.held = false;
            }
        }
        self.shared.1.notify_all();
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn work(shared: &Shared) {
    loop {
        let job = {
            let mut s = lock(shared);
            loop {
                #[cfg(test)]
                if s.held && !s.closed {
                    s = shared.1.wait(s).unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
                if let Some(job) = s.waiting.pop_front() {
                    s.busy = true;
                    break job;
                }
                if s.closed {
                    return;
                }
                s = shared.1.wait(s).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let id = job.id;
        // a panic in one write is that photo's failure, not the end of the queue
        let result = crate::guard::catch("XMP sidecar write", || write(&job)).unwrap_or_else(Err);
        {
            let mut s = lock(shared);
            s.done.push(Written { id, result });
            s.busy = false;
            s.finished += 1;
        }
        shared.1.notify_all();
    }
}

/// The file half of [`crate::Session::save_sidecar`]: beside an original that is there, the existing sidecar's
/// spelling kept, `packet` merged into what it holds.
pub(crate) fn write(job: &Job) -> Result<(PathBuf, Vec<u8>), String> {
    use crate::rename::MoveFs;
    if !Path::new(&job.original).is_file() {
        return Err(format!("{}: the original {} is missing; no XMP sidecar is written", job.file_name, job.original));
    }
    // on a case-insensitive volume `a.xmp` also answers for `a.XMP`: the spelling the folder lists comes first
    let existing: Vec<&PathBuf> = job.candidates.iter().filter(|p| p.is_file()).collect();
    let listed = existing.iter().find(|p| crate::rename::RealFs.listed(p)).copied();
    let path = listed.or_else(|| existing.first().copied()).unwrap_or(&job.preferred).clone();
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let (data, _, _) = crate::sidecar::sidecar_contents(&path, job.packet.clone(), &job.stamp).map_err(err)?;
    crate::sidecar::write_atomic(&path, data.as_bytes()).map_err(err)?;
    Ok((path, data.into_bytes()))
}

impl crate::Session {
    /// Write automatic sidecars on a worker (`on`), or in place again (the default; what is queued lands first).
    pub fn queue_sidecar_writes(&mut self, on: bool) {
        if on {
            self.sidecar_queue.get_or_insert_with(Box::default);
        } else if self.sidecar_queue.is_some() {
            self.finish_sidecar_writes();
            self.sidecar_queue = None;
        }
    }

    /// Automatic sidecar writes queued or in progress.
    pub fn sidecar_writes_pending(&self) -> usize {
        self.sidecar_queue.as_ref().map_or(0, |q| q.pending())
    }

    /// Block until the queued writes are on disk (their results are noted by the next
    /// [`Self::poll_sidecar_writes`]).
    pub fn wait_for_sidecar_writes(&self) {
        if let Some(q) = &self.sidecar_queue {
            q.wait();
        }
    }

    /// Land every queued write and note how each went: before anything else reads, writes or moves sidecars, and
    /// before quitting.
    pub fn finish_sidecar_writes(&mut self) {
        self.wait_for_sidecar_writes();
        self.poll_sidecar_writes();
    }

    /// Note the writes the worker finished (per frame): a written sidecar leaves the unsaved photos, a failed one
    /// joins them for [`Self::retry_unsaved_sidecars`]. Returns how many were written.
    pub fn poll_sidecar_writes(&mut self) -> usize {
        let Some(q) = self.sidecar_queue.as_mut() else { return 0 };
        let done = q.take_done();
        let mut written = 0;
        for w in done {
            match w.result {
                Ok((path, bytes)) => {
                    self.unsaved_sidecars.remove(&w.id);
                    self.note_sidecar_write(&path, &bytes);
                    written += 1;
                }
                Err(e) => {
                    // kept for a retry once the file is reachable: a network share that dropped out mustn't leave
                    // the sidecar silently behind the edit
                    log::warn!("auto-write XMP: {e}");
                    self.unsaved_sidecars.insert(w.id);
                }
            }
        }
        written
    }

    /// Whether automatic writes go through the queue now: it is on, and no sidecar repair is waiting (a repair
    /// moves packets the write must see in place, so then they are written in place).
    pub(crate) fn queueing_sidecars(&self) -> bool {
        self.sidecar_queue.is_some()
            && self.pending_sidecar_repairs.is_empty()
            && self.undo.iter().chain(&self.redo).all(|e| e.retry_sidecars.is_empty())
    }

    /// Queue photo `id`'s automatic sidecar write: `Ok(true)` queued, `Ok(false)` to be written in place (no queue,
    /// a repair waiting, no worker), an error when it can't be written at all.
    pub(crate) fn queue_sidecar(&mut self, id: PhotoId, owners: &crate::sidecar::StemOwners) -> crate::Result<bool> {
        if !self.queueing_sidecars() {
            return Ok(false);
        }
        let (original, preferred, candidates) = self.sidecar_target(id, owners)?;
        let p = self.catalog.photo(id).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
        let job = Job {
            id,
            file_name: p.file_name.clone(),
            original,
            preferred,
            candidates,
            packet: crate::sidecar::sidecar_packet(p, &self.catalog),
            stamp: (self.clock)(),
        };
        let Some(q) = self.sidecar_queue.as_mut() else { return Ok(false) };
        Ok(q.push(job, &self.activity))
    }
}

#[cfg(test)]
impl crate::Session {
    /// Tests: hold the queue's worker (see [`SidecarQueue::hold`]).
    pub(crate) fn hold_sidecar_writes(&self) {
        if let Some(q) = &self.sidecar_queue {
            q.hold();
        }
    }
}
