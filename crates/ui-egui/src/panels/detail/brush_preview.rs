//! The brush tool's renders for the live stroke ([`crate::brush_live`]): the loupe keeps a CPU
//! copy of its pixels, and a second render shows the photo with the active mask's brush covering
//! everything (or nothing, when erasing), so painting can blend the adjustment in under the brush.

use crate::LightcraftApp;
use crate::brush_live::{LiveImages, target};
use crate::render::Slot;
use egui::Rect;
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, LocalAdjustments};
use std::sync::Arc;

/// Changes the loupe's result key while the brush tool is open, so the render that follows keeps
/// its pixels even when the tool was opened after the loupe was last rendered.
pub(super) const BRUSH_KEY_SALT: u64 = 0x5bd1_e995_0b4a_c7d3;

/// Priority of the full-coverage render: below the loupe's own.
const PRIORITY: u32 = 95;

/// What the brush preview's settings are made from: the photo's settings (by their remembered hash), the mask and part
/// being painted into, the mode and the adjustment a new mask would start with.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MemoKey {
    id: PhotoId,
    settings: u64,
    mask: Option<u32>,
    part: Option<usize>,
    erase: bool,
    fresh: LocalAdjustments,
}

/// The settings made for a [`MemoKey`] (`None`: painting would show no adjustment) and their hash.
pub(crate) type Memo = (MemoKey, Option<(Arc<DevelopSettings>, u64)>);

/// The identity of the settings made for `key` (FNV-1a of its text: it holds the photo's remembered settings hash, the mask, the part,
/// the mode and the adjustment, which fix the settings).
fn key_hash(key: &MemoKey) -> u64 {
    format!("{key:?}").bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

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
    // holding ⌥ (Alt) paints with the other mode; in a Subtract part a stroke goes the other way
    let (part, turned) = super::super::mask_parts::painting_into(app, &d);
    let erase = super::super::mask_parts::stroke_erase(app.ui.brush_erase != ui.input(|i| i.modifiers.alt), turned);
    let fresh = lightcraft_engine::cmd::mask_effects::new_mask_adjust(&app.session, "brush");
    // (the same settings are made once, not on every frame: a copy of a mask with many strokes, and its hash, are slow)
    let key = MemoKey { id, settings: app.session.settings_hash(id).unwrap_or_default(), mask: app.session.active_mask, part, erase, fresh };
    let made = match &app.renderer.brush_settings {
        Some((k, made)) if *k == key => made.clone(),
        last => {
            // (the settings are a function of what the key holds, so their identity is a hash of it: hashing the settings
            // themselves writes every stroke of every mask to JSON, once per committed stroke)
            let hash = key_hash(&key);
            let made = target::settings_in(&d, key.mask, part, erase, fresh);
            // a stroke into the part being painted changes the photo's settings, not these (the part is covered whole):
            // the full-coverage render stays the one made, and the next stroke blends at once (issue #175: it was
            // rendered again after each stroke, ~200 ms on a 45 MP photo, and a stroke started meanwhile showed no adjustment)
            let kept = last.as_ref().and_then(|(_, m)| m.as_ref()).filter(|(s, _)| made.as_ref() == Some(&**s)).cloned();
            let made = kept.or_else(|| made.map(|s| (Arc::new(s), hash)));
            app.renderer.brush_settings = Some((key, made.clone()));
            made
        }
    };
    let Some((settings, hash)) = made else { return stop(app) };
    if let Some(job) = app.session.preview_job_shared(id, rw.max(8), rh.max(8), with_crop, &settings, hash) {
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
