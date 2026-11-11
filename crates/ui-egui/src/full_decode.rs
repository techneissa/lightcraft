//! The original of the photo open for editing, decoded in the background so the first zoom to 100 % finds it ready
//! (#182; the engine side is `lightcraft_engine::media::FullDecodeJob`).
//!
//! - **When:** the photo has stayed on screen in the Edit view (an editing panel on the right), at rest, for
//!   [`DWELL_SECS`]: stepping through photos never starts one (each would hold the memory gate and the cores the
//!   neighbours' previews need). Only a photo larger than the preview level whose original fits the source caches'
//!   share, and while no window job is decoding it already. Once per photo opened.
//! - **One at a time:** a decode for another photo, or one still running when the Edit view closes or the app
//!   quits, is stopped: still queued at the memory gate it gives up there, decoding its result is dropped on the
//!   worker. None starts until it has gone.
//! - **Shared:** a window of the photo waits for this decode once it is past the memory gate, instead of starting a
//!   second one (`panels/detail/window.rs`). While it is still queued there (AI Denoise holds the gate for its whole
//!   model run) the window stops it and decodes the original itself, as it did before.
//! - **Memory:** the engine keeps the original pinned while the photo is on screen and lets it go when another is
//!   shown, as it does for one a zoom decoded.
//! - Shows in the activity stack ("Reading the full-size photo") when it lasts more than a moment, without ✕: it is
//!   never worth asking about when quitting.

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::SourceLevel;
use lightcraft_engine::media::{FullDecodeJob, FullDecodeState, FullDecoded};

use crate::LightcraftApp;
use crate::render::Slot;
use crate::state::{RightPanel, ViewMode};

/// The activity stack's row (translated in `locales/`).
pub(crate) const LABEL: &str = "Reading the full-size photo";
/// Seconds a photo stays on screen at rest before its original is read ahead.
pub(crate) const DWELL_SECS: f64 = 1.0;
/// Bytes per pixel of a decoded original (linear RGB as `f32`).
const BYTES_PER_PIXEL: u64 = 12;

/// The read-ahead of the open photo's original ([`tick`]).
#[derive(Default)]
pub struct FullDecodeAhead {
    running: Option<Run>,
    /// The photo last read ahead (or found not to need it): once per photo opened.
    tried: Option<PhotoId>,
    /// The photo on screen at rest, and since when (egui time, s).
    resting: Option<(PhotoId, f64)>,
    /// Decodes started since launch (tests).
    started: u64,
}

struct Run {
    photo: PhotoId,
    state: Arc<FullDecodeState>,
}

impl FullDecodeAhead {
    /// Whether photo `id`'s original is being read ahead (queued or decoding, not stopped).
    pub fn running_for(&self, id: PhotoId) -> bool {
        self.running.as_ref().is_some_and(|r| r.photo == id && !r.state.is_stopped())
    }

    /// Whether photo `id`'s original is being read ahead, or will be once the photo has rested in the Edit view
    /// (`right`) and not been tried yet: another job that would decode it (the loupe's full-size picture, #178) waits.
    pub fn coming_for(&self, right: RightPanel, id: PhotoId) -> bool {
        self.running_for(id) || (!cfg!(target_arch = "wasm32") && editing(right) && self.tried != Some(id))
    }

    /// Whether a decode, stopped or not, is still on its worker.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Decodes started since launch.
    pub fn started(&self) -> u64 {
        self.started
    }

    /// Stop the read-ahead under way, if any (the photo may be read ahead again when it is next shown).
    pub fn stop(&mut self) {
        if let Some(run) = &self.running {
            run.state.stop();
        }
        self.tried = None;
        self.resting = None;
    }
}

/// Should a window of photo `id` wait for its read-ahead? Yes once that is decoding. One still queued at the memory
/// gate is stopped instead, and the window decodes the original itself.
pub(crate) fn window_waits(app: &mut LightcraftApp, id: PhotoId) -> bool {
    reap(app);
    let Some(run) = app.full_decode.running.as_ref().filter(|r| r.photo == id) else { return false };
    if run.state.stop_if_queued() {
        return false;
    }
    run.state.is_decoding()
}

/// Is the right panel one of the Edit view's (where a zoom to 100 % comes next)? (Also what tells parsing ahead the user
/// edits rather than culls, `crate::parse_ahead`.)
pub(crate) fn editing(right: RightPanel) -> bool {
    matches!(right, RightPanel::Edit | RightPanel::Crop | RightPanel::Remove | RightPanel::Masking | RightPanel::RedEye | RightPanel::Portrait)
}

/// Forget a read-ahead whose worker is gone without answering (it panicked): else a window would wait for it forever.
fn reap(app: &mut LightcraftApp) {
    if app.full_decode.running.is_some() && !app.tasks.is_running(LABEL) {
        app.full_decode.running = None;
    }
}

/// Every frame, whatever the view: a read-ahead stops when the Edit view closes (another view, or no editing panel).
pub(crate) fn frame(app: &mut LightcraftApp) {
    reap(app);
    if app.full_decode.busy() && !(app.ui.view == ViewMode::Detail && editing(app.ui.right)) {
        app.full_decode.stop();
    }
}

/// Called by the loupe each frame it shows photo `id` (`at_rest`: no drag or gesture under way): stop a read-ahead of
/// another photo, and start this one's when it is due (see the module docs).
pub(crate) fn tick(app: &mut LightcraftApp, ctx: &egui::Context, id: PhotoId, at_rest: bool) {
    reap(app);
    if app.full_decode.running.as_ref().is_some_and(|r| r.photo != id) {
        app.full_decode.stop();
    }
    // once the photo is on screen (its first render goes first), and while no window job decodes the original itself
    let shown = !app.renderer.is_pending(Slot::Main) && app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == id);
    // (the loupe's full-size picture rendering decodes it too, #178)
    let windowing = app.renderer.is_pending(Slot::Region) || app.renderer.is_pending(Slot::RegionBefore) || app.renderer.full.renders(id);
    if cfg!(target_arch = "wasm32") || !editing(app.ui.right) || !at_rest || !shown || windowing {
        app.full_decode.resting = None;
        return;
    }
    if app.full_decode.busy() || app.full_decode.tried == Some(id) {
        return;
    }
    let now = ctx.input(|i| i.time);
    let since = match app.full_decode.resting {
        Some((rid, t)) if rid == id => t,
        _ => {
            app.full_decode.resting = Some((id, now));
            now
        }
    };
    if now - since < DWELL_SECS {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((DWELL_SECS - (now - since)).max(0.01)));
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

fn start(app: &mut LightcraftApp, job: FullDecodeJob) {
    let photo = job.photo;
    let state = Arc::new(FullDecodeState::default());
    let worker = state.clone();
    let done = move |app: &mut LightcraftApp, _: &egui::Context, done: Option<FullDecoded>| {
        let current = app.full_decode.running.take_if(|r| r.photo == photo);
        let stopped = current.is_none_or(|r| r.state.is_stopped());
        if let Some(done) = done.filter(|_| !stopped)
            && !app.session.accept_full_decode(&done)
            && let Err(why) = &done.loaded
        {
            log::info!("photo {}: the full-size original was not read ahead: {why}", photo.0);
        }
    };
    // (no ✕ and nothing to ask about when quitting: the app stops it itself, see `frame`)
    match crate::tasks::spawn_background(app, LABEL, "decode", move || job.run(&worker), done) {
        Ok(()) => {
            app.full_decode.running = Some(Run { photo, state });
            app.full_decode.started += 1;
        }
        Err(why) => log::warn!("{why}"),
    }
}
