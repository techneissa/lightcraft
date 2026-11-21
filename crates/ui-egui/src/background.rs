//! Fork feature (issue #304, `fork/idle-memory.md`): a window that has not been in front for a while lets go of what the
//! photo in front can do without.
//!
//! The window counts as in the background while it has no keyboard focus or is minimized or hidden. Models and
//! caches use the shorter [`lightcraft_engine::idle::BACKGROUND_IDLE`] then; once it has stayed there for
//! [`RELEASE_AFTER`] seconds the full-size originals, the renderer's stage caches and GPU buffers, and the allocator's
//! free pages are released, once. Coming back lets the next frame take what it needs again.

use crate::LightcraftApp;

/// Seconds in the background before the caches are released.
pub const RELEASE_AFTER: f64 = 20.0;

/// Where the window is in its time in the background.
#[derive(Default)]
pub struct Background {
    /// When it went to the background (egui time, s).
    since: Option<f64>,
    /// The caches were released for this stay.
    released: bool,
    /// The AI mask models are kept loaded (fork, issue #353): the Masking panel is open in the front window.
    mask_models: bool,
}

/// Called every frame (also from hidden ticks).
pub fn tick(app: &mut LightcraftApp, ctx: &egui::Context, now: f64) {
    let (focused, minimized, visible) = ctx.input(|i| (i.raw.focused, i.viewport().minimized == Some(true), i.viewport().visible()));
    let background = !focused || minimized || visible == Some(false);
    lightcraft_engine::idle::set_background(background);
    // (fork, issue #353: a mask made after a pause in the Masking panel does not load and plan the models again)
    let masking = !background
        && app.ui.right == crate::state::RightPanel::Masking
        && !app.ui.fullscreen
        && crate::docking::exposed(app, crate::docking::Panel::Tools);
    if masking != app.background.mask_models {
        app.background.mask_models = masking;
        app.session.keep_mask_models(masking);
    }
    if !background {
        (app.background.since, app.background.released) = (None, false);
        return;
    }
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
