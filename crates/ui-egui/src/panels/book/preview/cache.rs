//! The preview's pages: each one composed at the screen's size on a worker thread (the export's
//! composer, [`lightcraft_engine::book::preview`]) and kept as a texture. The spread on show comes
//! first, then the next and the previous one, so a page turn finds its pages ready. Until a page
//! is ready the preview draws it from the grid's thumbnails (never a blank page); like the loupe's
//! "Rendering…" stand-in, that is a view whose pixels aren't final, so the renders hold no row in
//! the activity stack.
//!
//! One texture is uploaded per frame at most, so a frame never pays for two pages at once.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use lightcraft_engine::book::model::Book;
use lightcraft_engine::book::{preview, render};

use crate::LightcraftApp;

/// Page renders running at once.
const MAX_JOBS: usize = 2;

enum State {
    Pending(Receiver<Result<ColorImage, String>>),
    Ready(TextureHandle),
    Failed,
}

struct Entry {
    state: State,
    cancel: Arc<AtomicBool>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        // (a render that has not finished is not wanted any more)
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The pages of the preview that are ready or on their way, by [`key`].
#[derive(Default)]
pub struct Cache {
    entries: HashMap<u64, Entry>,
    /// Render threads still running, also those of pages dropped from the cache meanwhile (a thread
    /// stops only between photos), so the cap on decodes at once holds.
    in_flight: Arc<AtomicUsize>,
    /// Pages whose render failed, for the preview to say once.
    pub failures: Vec<String>,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").field("pages", &self.entries.len()).finish()
    }
}

// (a copy of the view's state starts with no pages: they are made again when the preview draws)
impl Clone for Cache {
    fn clone(&self) -> Self {
        Cache::default()
    }
}

/// What a page looks like to the cache: its place, what is on it and how large it is drawn.
fn key(book: &Book, page: usize, edge: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (book.id, page, edge).hash(&mut h);
    format!("{:?}{:?}", book.pages.get(page), book.settings).hash(&mut h);
    h.finish()
}

impl Cache {
    /// The texture of `page` when it is ready.
    pub fn texture(&self, book: &Book, page: usize, edge: usize) -> Option<&TextureHandle> {
        match &self.entries.get(&key(book, page, edge))?.state {
            State::Ready(t) => Some(t),
            _ => None,
        }
    }

    /// Has the render of this page ended (a texture, or a failure that leaves the stand-in as it is)?
    pub fn settled(&self, book: &Book, page: usize, edge: usize) -> bool {
        self.entries.get(&key(book, page, edge)).is_some_and(|e| !matches!(e.state, State::Pending(_)))
    }

    /// Render threads running now.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    /// Pages being rendered.
    pub fn pending(&self) -> usize {
        self.entries.values().filter(|e| matches!(e.state, State::Pending(_))).count()
    }

    /// Pages ready.
    pub fn ready(&self) -> usize {
        self.entries.values().filter(|e| matches!(e.state, State::Ready(_))).count()
    }

    /// Keeps the `wanted` pages (most wanted first) at `edge` pixels: starts the renders that are
    /// missing, takes in the one that finished, and drops everything else. Returns the milliseconds
    /// the texture upload took and the milliseconds setting the renders up took.
    pub fn update(&mut self, app: &mut LightcraftApp, ctx: &Context, book: &Book, wanted: &[usize], edge: usize) -> (f32, f32) {
        let keys: Vec<u64> = wanted.iter().map(|p| key(book, *p, edge)).collect();
        self.entries.retain(|k, _| keys.contains(k));
        let started = std::time::Instant::now();
        let mut uploaded = false;
        let mut failed = Vec::new();
        for (k, entry) in &mut self.entries {
            let State::Pending(rx) = &entry.state else { continue };
            if uploaded {
                // (the next frame takes this one)
                ctx.request_repaint();
                continue;
            }
            match rx.try_recv() {
                Ok(Ok(img)) => {
                    entry.state = State::Ready(ctx.load_texture(format!("book-preview-{k:x}"), img, TextureOptions::LINEAR));
                    uploaded = true;
                }
                Ok(Err(e)) => {
                    if e != render::CANCELLED {
                        failed.push(e);
                    }
                    entry.state = State::Failed;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => entry.state = State::Failed,
            }
        }
        let upload_ms = if uploaded { started.elapsed().as_secs_f32() * 1000.0 } else { 0.0 };
        self.failures.extend(failed);
        let setup = std::time::Instant::now();
        for (page, k) in wanted.iter().zip(&keys) {
            if self.in_flight() >= MAX_JOBS {
                break;
            }
            if self.entries.contains_key(k) {
                continue;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let state = match start(app, ctx, book, *page, edge, cancel.clone(), &self.in_flight) {
                Ok(rx) => State::Pending(rx),
                Err(e) => {
                    self.failures.push(e);
                    State::Failed
                }
            };
            self.entries.insert(*k, Entry { state, cancel });
        }
        (upload_ms, setup.elapsed().as_secs_f32() * 1000.0)
    }
}

/// Sets `page` up and renders it on a thread; the image arrives on the channel.
fn start(
    app: &mut LightcraftApp,
    ctx: &Context,
    book: &Book,
    page: usize,
    edge: usize,
    cancel: Arc<AtomicBool>,
    in_flight: &Arc<AtomicUsize>,
) -> Result<Receiver<Result<ColorImage, String>>, String> {
    let job = preview::prepare_page(&mut app.session, book, page, edge)?;
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    spawn_counted(in_flight, move || {
        let r = job.run(&|| cancel.load(Ordering::Relaxed)).map(|px| ColorImage::from_rgba_unmultiplied([px.width, px.height], &px.rgba));
        let _ = tx.send(r);
        ctx.request_repaint();
    })?;
    Ok(rx)
}

/// Counts a running render down when its thread ends, however it ends.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Runs `work` on a thread that holds one of `counter`'s slots until it ends.
fn spawn_counted(counter: &Arc<AtomicUsize>, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    counter.fetch_add(1, Ordering::Relaxed);
    let slot = Slot(counter.clone());
    let run = move || {
        let _slot = slot;
        work();
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("book-preview".into()).spawn(run).map(|_| ()).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page dropped from the cache keeps its thread, and so its slot, until the thread ends.
    #[test]
    fn a_running_thread_holds_its_slot_until_it_ends() {
        let counter = Arc::new(AtomicUsize::new(0));
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(std::sync::Mutex::new(gate));
        for _ in 0..3 {
            let gate = gate.clone();
            spawn_counted(&counter, move || {
                let _ = gate.lock().map(|g| g.recv());
            })
            .unwrap();
        }
        assert_eq!(counter.load(Ordering::Relaxed), 3);
        drop(release);
        let t = std::time::Instant::now();
        while counter.load(Ordering::Relaxed) > 0 && t.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(counter.load(Ordering::Relaxed), 0, "every slot is given back");
    }

    /// A thread that panics gives its slot back too.
    #[test]
    fn a_panicking_thread_gives_its_slot_back() {
        let counter = Arc::new(AtomicUsize::new(0));
        spawn_counted(&counter, || {
            #[allow(clippy::panic)]
            {
                panic!("a render that panics")
            }
        })
        .unwrap();
        let t = std::time::Instant::now();
        while counter.load(Ordering::Relaxed) > 0 && t.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
