//! The brush tool's renders for the live stroke ([`crate::brush_live`]): the loupe keeps a CPU
//! copy of its pixels, and a second render shows the photo with the active mask's brush covering
//! everything (or nothing, when erasing), so painting can blend the adjustment in under the brush.

use crate::LightcraftApp;
use crate::brush_live::{LiveImages, target};
use crate::render::Slot;
use egui::Rect;
use lightcraft_catalog::PhotoId;

/// Changes the loupe's result key while the brush tool is open, so the render that follows keeps
/// its pixels even when the tool was opened after the loupe was last rendered.
pub(super) const BRUSH_KEY_SALT: u64 = 0x5bd1_e995_0b4a_c7d3;

/// Priority of the full-coverage render: below the loupe's own.
const PRIORITY: u32 = 95;

/// What the loupe render being asked for is.
#[derive(Clone, Copy)]
pub(super) struct Ask {
    pub id: PhotoId,
    /// The loupe's render size.
    pub size: [usize; 2],
    pub with_crop: bool,
    pub fit_px: [usize; 2],
    pub interacting: bool,
    /// No diagnostic overlay is on the loupe.
    pub plain: bool,
}

/// Keep the loupe's pixels while the brush tool is open and request the full-coverage render of
/// the photo at the loupe's size. Nothing is requested during a slider drag, over a diagnostic
/// overlay or a soft proof, or when painting would show no adjustment.
pub(super) fn request(app: &mut LightcraftApp, ui: &egui::Ui, a: &Ask) {
    let Ask { id, size: [rw, rh], with_crop, fit_px, interacting, plain } = *a;
    let open = app.ui.tool == "brush";
    app.renderer.keep_brush_pixels = open;
    let stop = |app: &mut LightcraftApp| {
        app.renderer.brush_target = None;
        app.renderer.release(Slot::BrushFull);
    };
    if !open || !plain || app.ui.soft_proof {
        return stop(app);
    }
    if interacting {
        // (the sliders change what the picture should be: it is drawn again when they stop)
        app.renderer.brush_target = None;
        return;
    }
    let Some(d) = app.session.develop_of(id) else { return stop(app) };
    // holding ⌥ (Alt) paints with the other mode
    let erase = app.ui.brush_erase != ui.input(|i| i.modifiers.alt);
    let fresh = lightcraft_engine::cmd::mask_effects::new_mask_adjust(&app.session, "brush");
    let Some(settings) = target::settings(&d, app.session.active_mask, erase, fresh) else { return stop(app) };
    if let Some(job) = app.session.preview_job(id, rw.max(8), rh.max(8), with_crop, &settings) {
        let job = job.with_display_fit(fit_px[0], fit_px[1]).with_hidden_grading(app.ui.grading_hidden_for(id));
        // the picture to blend is the one of these settings, and no other
        app.renderer.brush_target = Some(job.key);
        // one render at a time: the loupe's own, with the committed stroke, is what the user waits for
        if !app.renderer.is_pending(Slot::Main) {
            app.renderer.request(Slot::BrushFull, job, PRIORITY);
        }
    }
}

/// The two finished pictures a live stroke blends, when both are the current renders of the open
/// photo (not drafts, stand-ins or stale).
pub(super) fn images(app: &LightcraftApp, id: PhotoId, img_rect: Rect) -> Option<LiveImages> {
    let r = &app.renderer;
    let ready = |slot: Slot, key: Option<u64>| {
        r.textures.get(&slot).filter(|t| t.photo == id && key.is_some() && Some(t.key) == key && !r.is_pending(slot) && t.quick.is_none())
    };
    let (base, target) = (ready(Slot::Main, r.wanted(Slot::Main))?, ready(Slot::BrushFull, r.brush_target)?);
    Some(LiveImages { base: base.pixels.clone()?, target: target.pixels.clone()?, key: (base.key, target.key), img_rect })
}
