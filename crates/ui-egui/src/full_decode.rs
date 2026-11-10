//! The original of the photo open for editing, decoded in the background so the first zoom to 100 % finds it ready
//! (#182; the engine side is `lightcraft_engine::media::FullDecodeJob`).
//!
//! - **When:** the photo is on screen in the Edit view (an editing panel on the right), at rest, larger than the
//!   preview level, and no window job is decoding it already. Once per photo opened.
//! - **One at a time:** a decode for another photo is stopped (its flag set; a decode under way finishes and is
//!   dropped on its worker) and none starts until it has.
//! - **Shared:** a window job of the photo waits for this decode instead of starting a second one
//!   (`panels/detail/window.rs`), and every job that needs the original then reads it from the source cache.
//! - **Memory:** only an original that fits the source caches' share is read ahead; the engine keeps it pinned while
//!   the photo is on screen and lets it go when another is shown, as it does for one a zoom decoded.
//! - Shows in the activity stack ("Reading the full-size photo") when it lasts more than a moment.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::PhotoId;
use lightcraft_engine::SourceLevel;

use crate::LightcraftApp;
use crate::render::Slot;
use crate::state::RightPanel;

/// The activity stack's row (translated in `locales/`).
pub(crate) const LABEL: &str = "Reading the full-size photo";
/// Bytes per pixel of a decoded original (linear RGB as `f32`).
const BYTES_PER_PIXEL: u64 = 12;

/// The read-ahead of the open photo's original ([`tick`]).
#[derive(Default)]
pub struct FullDecodeAhead {
    running: Option<Run>,
    /// The photo last read ahead (or found not to need it): once per photo opened.
    tried: Option<PhotoId>,
    /// Decodes started since launch (tests).
    started: u64,
}

struct Run {
    photo: PhotoId,
    cancel: Arc<AtomicBool>,
}

impl FullDecodeAhead {
    /// Whether photo `id`'s original is being read ahead (not stopped).
    pub fn running_for(&self, id: PhotoId) -> bool {
        self.running.as_ref().is_some_and(|r| r.photo == id && !r.cancel.load(Ordering::Relaxed))
    }

    /// Whether a decode, stopped or not, is still on its worker.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Decodes started since launch.
    pub fn started(&self) -> u64 {
        self.started
    }
}

/// Is the right panel one of the Edit view's (where a zoom to 100 % comes next)?
fn editing(right: RightPanel) -> bool {
    matches!(right, RightPanel::Edit | RightPanel::Crop | RightPanel::Remove | RightPanel::Masking | RightPanel::RedEye | RightPanel::Portrait)
}

/// Called by the loupe each frame it shows photo `id` (`at_rest`: no drag or gesture under way): stop a read-ahead of
/// another photo, and start this one's when it is due (see the module docs).
pub(crate) fn tick(app: &mut LightcraftApp, id: PhotoId, at_rest: bool) {
    let editing = editing(app.ui.right);
    // (another panel over the same photo keeps it: a zoom there still wants the original)
    if let Some(run) = &app.full_decode.running
        && run.photo != id
    {
        run.cancel.store(true, Ordering::Relaxed);
    }
    if cfg!(target_arch = "wasm32") || !editing || !at_rest || app.full_decode.busy() || app.full_decode.tried == Some(id) {
        return;
    }
    // once the photo is on screen (its first render goes first), and while no window job decodes the original itself
    let shown = !app.renderer.is_pending(Slot::Main) && app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == id);
    if !shown || app.renderer.is_pending(Slot::Region) || app.renderer.is_pending(Slot::RegionBefore) {
        return;
    }
    app.full_decode.tried = Some(id);
    let Some(p) = app.session.catalog.photo(id) else { return };
    let (w, h) = (p.width as u64, p.height as u64);
    // a photo the preview level holds whole never needs its original for a zoom
    if SourceLevel::for_size(w.max(h) as usize) != SourceLevel::Full {
        return;
    }
    let bytes = w.saturating_mul(h).saturating_mul(BYTES_PER_PIXEL);
    if bytes > app.session.media.budget() as u64 {
        return;
    }
    let Some(job) = app.session.full_decode_job(id) else { return };
    start(app, job);
}

fn start(app: &mut LightcraftApp, job: lightcraft_engine::media::FullDecodeJob) {
    let photo = job.photo;
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let work = move |_progress| {
        if flag.load(Ordering::Relaxed) {
            return None;
        }
        let done = job.run();
        // (stopped meanwhile: the original is dropped here, on the worker)
        (!flag.load(Ordering::Relaxed)).then_some(done)
    };
    let done = move |app: &mut LightcraftApp, _: &egui::Context, done: Option<lightcraft_engine::media::FullDecoded>| {
        let current = app.full_decode.running.take_if(|r| r.photo == photo);
        let stopped = current.is_none_or(|r| r.cancel.load(Ordering::Relaxed));
        match done {
            Some(done) if !stopped => {
                if !app.session.accept_full_decode(&done)
                    && let Err(why) = &done.loaded
                {
                    log::info!("photo {}: the full-size original was not read ahead: {why}", photo.0);
                }
            }
            _ => {}
        }
    };
    match crate::tasks::spawn_tracked(app, LABEL, "decode", cancel.clone(), work, done) {
        Ok(()) => {
            app.full_decode.running = Some(Run { photo, cancel });
            app.full_decode.started += 1;
        }
        Err(why) => log::warn!("{why}"),
    }
}
