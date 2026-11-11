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
//!
//! Interactive work never waits behind background work. A background decode (a grid thumbnail's view, the photos
//! prepared ahead: [`crate::memory::in_background`]) can sit at the memory gate, where interactive loads never wait
//! (`files::fs`). An interactive job (the loupe) that finds a background decode of its file running decodes it
//! itself, ungated, and the jobs that ask after it wait for that decode instead.

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use super::source::DecodedSource;

/// A decode running: its result once there (`None` while it runs), and the signal that it is.
#[derive(Default)]
struct Running {
    done: Mutex<Option<Option<DecodedSource>>>,
    finished: Condvar,
    /// Run as background work: it may wait at the memory gate, so an interactive job doesn't wait for it.
    background: bool,
    /// Jobs that joined it.
    waiting: std::sync::atomic::AtomicUsize,
}

static RUNNING: Mutex<Vec<(u128, Arc<Running>)>> = Mutex::new(Vec::new());

/// `decode()`, or the result of the same decode (`key`) another job is running now.
pub(super) fn decode_shared(key: u128, decode: impl FnOnce() -> Result<DecodedSource, String>) -> Result<DecodedSource, String> {
    if rayon::current_thread_index().is_some() {
        return decode();
    }
    let background = crate::memory::is_background();
    let (running, mine) = {
        let mut all = RUNNING.lock().unwrap_or_else(PoisonError::into_inner);
        match all.iter().position(|(k, _)| *k == key) {
            // (an interactive job never waits for a background decode: see the module's note)
            Some(at) if background || !all.get(at).is_some_and(|(_, r)| r.background) => match all.get(at) {
                Some((_, r)) => (r.clone(), false),
                None => return decode(),
            },
            found => {
                // this decode is the one later jobs wait for; a background one it replaces keeps its own waiters
                if let Some(at) = found {
                    all.swap_remove(at);
                }
                let r = Arc::new(Running { background, ..Running::default() });
                all.push((key, r.clone()));
                (r, true)
            }
        }
    };
    if !mine {
        let mut done = running.done.lock().unwrap_or_else(PoisonError::into_inner);
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

/// Jobs that joined the decode running under `key` (0 once it has ended).
pub(super) fn waiting(key: u128) -> usize {
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

    use super::{decode_shared, until_waiting, waiting};
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

    /// Review of #246: the loupe (interactive) never waits behind a background decode of the same file, which can be
    /// queued at the memory gate (where interactive loads never wait). It decodes on its own, and a job asking after
    /// it waits for that decode, not the background one.
    #[test]
    fn an_interactive_load_never_waits_on_a_background_decode_queued_at_the_gate() {
        let key = u128::from_le_bytes(*b"shared-decode-03");
        let started = Arc::new(Barrier::new(2));
        let (release, held) = std::sync::mpsc::channel::<()>();
        // the grid's view decode: background work, held as if queued at the gate
        let background = {
            let started = started.clone();
            std::thread::spawn(move || {
                crate::memory::in_background(|| {
                    decode_shared(key, || {
                        started.wait();
                        let _ = held.recv_timeout(std::time::Duration::from_secs(60));
                        Ok(picture(0.2))
                    })
                })
            })
        };
        started.wait();
        // the loupe: its own decode, while the background one is still held
        let (loupe_in, loupe_out) = std::sync::mpsc::channel::<()>();
        let (loupe_started, loupe_decoding) = std::sync::mpsc::channel::<()>();
        let (loupe_done, loupe_result) = std::sync::mpsc::channel();
        let loupe = std::thread::spawn(move || {
            let r = decode_shared(key, || {
                let _ = loupe_started.send(());
                let _ = loupe_out.recv_timeout(std::time::Duration::from_secs(60));
                Ok(picture(0.6))
            });
            let _ = loupe_done.send(r.map(|s| s.image.data[0]));
        });
        loupe_decoding.recv_timeout(std::time::Duration::from_secs(60)).expect("the loupe decodes while the background decode is held");
        // a third job (the build) asks while the loupe decodes: it waits for the loupe's decode
        let build = std::thread::spawn(move || crate::memory::in_background(|| decode_shared(key, || Ok(picture(0.9)))));
        until_waiting(key);
        assert_eq!(waiting(key), 1, "the build waits for the loupe's decode");
        loupe_in.send(()).unwrap();
        let got = loupe_result.recv_timeout(std::time::Duration::from_secs(60)).expect("the loupe is not held by the background decode");
        assert_eq!(got, Ok([0.6; 3]), "the loupe's own decode");
        assert_eq!(build.join().unwrap().unwrap().image.data[0], [0.6; 3], "the build took the loupe's picture");
        loupe.join().unwrap();
        release.send(()).unwrap();
        assert_eq!(background.join().unwrap().unwrap().image.data[0], [0.2; 3], "the background decode ends on its own");
    }
}
