//! Fork feature (issue #304, `fork/idle-memory.md`): a window that has not been in front for a while lets go of what the
//! photo in front can do without.
//!
//! The window counts as in the background while it has no keyboard focus or is minimized or hidden. Models and
//! caches use the shorter [`lightcraft_engine::idle::BACKGROUND_IDLE`] then; once it has stayed there for
//! [`RELEASE_AFTER`] seconds the full-size originals, the renderer's stage caches and GPU buffers, and the allocator's
//! free pages are released, once. Coming back lets the next frame take what it needs again.
//!
//! In front, a view that has rested [`REST_AFTER`] seconds with nothing rendering and nothing on screen drawn from the
//! full-size original (Fit, or any view the preview level covers) lets go of the originals and the full-size picture,
//! once per rest (issue #436): about 400 MB for a 33 MP raw. The next zoom past Fit decodes the original again and
//! reads the picture back from disk; slider ticks at Fit never use either.

use crate::LightcraftApp;

/// Seconds in the background before the caches are released.
pub const RELEASE_AFTER: f64 = 20.0;
/// Seconds a view in front rests without the full-size original before the original and the picture go.
pub const REST_AFTER: f64 = 30.0;

/// Where the window is in its time in the background.
#[derive(Default)]
pub struct Background {
    /// When it went to the background (egui time, s).
    since: Option<f64>,
    /// The caches were released for this stay.
    released: bool,
    /// Since when the view in front has rested without the original (egui time, s), and whether that rest released
    /// it ([`REST_AFTER`]).
    pub(crate) rest: Option<(f64, bool)>,
    /// The AI mask models kept loaded (fork, issues #353 and #407): while the Masking panel is open in the front window,
    /// the ones the open photo's AI masks use.
    mask_models: lightcraft_engine::segment::MaskModels,
    /// What each mask of the open photo uses (fork, issue #407).
    pub ai: crate::panels::mask_stop_ai::Uses,
}

/// Called every frame (also from hidden ticks).
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    let (focused, minimized, visible) = ctx.input(|i| (i.raw.focused, i.viewport().minimized == Some(true), i.viewport().visible()));
    let background = !focused || minimized || visible == Some(false);
    lightcraft_engine::idle::set_background(background);
    // (fork, issue #353: a mask made after a pause in the Masking panel does not load and plan the models again; #407:
    // only the models the open photo's AI masks use, so brush, gradient and range work holds none)
    let masking = !background
        && app.ui.right == crate::state::RightPanel::Masking
        && !app.ui.fullscreen
        && crate::docking::exposed(app, crate::docking::Panel::Tools);
    let models = if masking { crate::panels::mask_stop_ai::photo_models(app, now) } else { lightcraft_engine::segment::MaskModels::NONE };
    if models != app.background.mask_models {
        app.background.mask_models = models;
        app.session.keep_mask_models(models);
    }
    if !background {
        (app.background.since, app.background.released) = (None, false);
        rest(app, ctx, now);
        return;
    }
    app.background.rest = None;
    let since = *app.background.since.get_or_insert(now);
    if app.background.released {
        return;
    }
    let left = RELEASE_AFTER - (now - since);
    if left > 0.0 {
        // (a window in the background draws no frames of its own)
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
        return;
    }
    app.background.released = true;
    app.session.release_for_background();
    app.renderer.release_for_background();
    lightcraft_engine::memory::release();
}

/// In front: once the view has rested [`REST_AFTER`] seconds without the full-size original, let go of the originals
/// and the full-size picture (see the module docs).
fn rest(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    // (a read-ahead of the original under way brings it back when it ends: the rest starts after it)
    if !app.renderer.rests_without_original() || app.full_decode.busy() {
        app.background.rest = None;
        return;
    }
    let (since, done) = *app.background.rest.get_or_insert((now, false));
    if done {
        return;
    }
    let left = REST_AFTER - (now - since);
    if left > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left + 0.05));
        return;
    }
    app.background.rest = Some((since, true));
    app.session.release_at_rest();
    app.renderer.release_at_rest();
    lightcraft_engine::memory::release();
}
