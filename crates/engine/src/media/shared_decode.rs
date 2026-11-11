//! One decode of a file at a size, shared by the jobs that ask for it while it runs (issue #221). The loupe, the
//! photos it prepares ahead, the background preview build and a grid thumbnail's view each make their own job, from
//! what the session's source cache held when the job was made; two of them often reach the same raw at the same
//! moment, and each decoded it (half a second or more of work, and its memory twice). The job that asks second now
//! waits for the first one's picture and takes it.
//!
//! Only a decode that succeeds is shared: one that fails or is cancelled (its own job stopped) leaves the jobs that
//! waited to decode the file themselves. A job on one of rayon's threads never waits: the decode it would wait for
//! could be running further up that same thread's stack (rayon runs other jobs while it waits for its own), so it
//! decodes alone, as before.

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use super::source::DecodedSource;

/// A decode running: its result once there (`None` while it runs), and the signal that it is.
#[derive(Default)]
struct Running {
    done: Mutex<Option<Option<DecodedSource>>>,
    finished: Condvar,
    /// Jobs waiting for it (tests).
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicUsize,
}

static RUNNING: Mutex<Vec<(u128, Arc<Running>)>> = Mutex::new(Vec::new());

/// `decode()`, or the result of the same decode (`key`) another job is running now.
pub(super) fn decode_shared(key: u128, decode: impl FnOnce() -> Result<DecodedSource, String>) -> Result<DecodedSource, String> {
    if rayon::current_thread_index().is_some() {
        return decode();
    }
    let (running, mine) = {
        let mut all = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        match all.iter().find(|(k, _)| *k == key) {
            Some((_, r)) => (r.clone(), false),
            None => {
                let r = Arc::new(Running::default());
                all.push((key, r.clone()));
                (r, true)
            }
        }
    };
    if !mine {
        let mut done = running.done.lock().unwrap_or_else(PoisonError::into_inner);
        #[cfg(test)]
        running.waiting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        while done.is_none() {
            done = running.finished.wait(done).unwrap_or_else(PoisonError::into_inner);
        }
        if let Some(Some(source)) = done.as_ref() {
            return Ok(source.clone());
        }
        drop(done);
        // (the decode failed or was cancelled: this job's own decode, whatever stopped that one)
        return decode();
    }
    // (finished on every way out, a panic included: nobody waits for a decode that is gone)
    let mut finish = Finish { key, running, result: None };
    let r = decode();
    finish.result = r.as_ref().ok().cloned();
    r
}

/// Jobs waiting for the decode running under `key` (tests).
#[cfg(test)]
fn waiting(key: u128) -> usize {
    let all = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
    all.iter().find(|(k, _)| *k == key).map_or(0, |(_, r)| r.waiting.load(std::sync::atomic::Ordering::SeqCst))
}

/// Wait (at most 10 s, against a hang) until a job waits for the decode under `key` (tests).
#[cfg(test)]
fn until_waiting(key: u128) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while waiting(key) == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
}

/// Ends a running decode: hands its result (`None`: failed) to the jobs waiting and takes it off the list.
struct Finish {
    key: u128,
    running: Arc<Running>,
    result: Option<DecodedSource>,
}

impl Drop for Finish {
    fn drop(&mut self) {
        RUNNING.lock().unwrap_or_else(PoisonError::into_inner).retain(|(k, r)| *k != self.key || !Arc::ptr_eq(r, &self.running));
        *self.running.done.lock().unwrap_or_else(PoisonError::into_inner) = Some(self.result.take());
        self.running.finished.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use super::{decode_shared, until_waiting};
    use crate::media::source::DecodedSource;

    fn picture(v: f32) -> DecodedSource {
        let mut image = lightcraft_raster::Rgb32f::new(4, 4);
        image.data.fill([v; 3]);
        DecodedSource::new(Arc::new(image), None)
    }

    /// Two jobs asking for the same decode at once get one decode and the same picture.
    #[test]
    fn a_decode_asked_for_twice_at_once_runs_once() {
        let key = u128::from_le_bytes(*b"shared-decode-01");
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let first = {
            let (calls, started, release) = (calls.clone(), started.clone(), release.clone());
            std::thread::spawn(move || {
                decode_shared(key, || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started.wait();
                    release.wait();
                    Ok(picture(0.5))
                })
            })
        };
        // (the first decode is running: the second job finds it)
        started.wait();
        let second = {
            let calls = calls.clone();
            std::thread::spawn(move || {
                decode_shared(key, || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(picture(0.9))
                })
            })
        };
        until_waiting(key);
        release.wait();
        let (a, b) = (first.join().unwrap().unwrap(), second.join().unwrap().unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one decode");
        assert!(Arc::ptr_eq(&a.image, &b.image), "the same picture");
    }

    /// A decode that fails (or was cancelled) is not handed on: the job that waited decodes the file itself.
    #[test]
    fn a_failed_decode_leaves_the_waiting_job_to_decode() {
        let key = u128::from_le_bytes(*b"shared-decode-02");
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let first = {
            let (started, release) = (started.clone(), release.clone());
            std::thread::spawn(move || {
                decode_shared(key, || {
                    started.wait();
                    release.wait();
                    Err("cancelled".to_string())
                })
            })
        };
        started.wait();
        let second = std::thread::spawn(move || decode_shared(key, || Ok(picture(0.7))));
        until_waiting(key);
        release.wait();
        assert!(first.join().unwrap().is_err());
        let own = second.join().unwrap().expect("its own decode");
        assert_eq!(own.image.data[0], [0.7; 3]);
    }
}
