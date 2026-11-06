//! The mask overlay is drawn over the loupe's plain render ([`crate::mask_tint`]): showing it, hiding it and changing its
//! opacity, colour or view start no render of the photo, and with it off no plane is made. The tests count the photo renders
//! that finish (the renders that finished, less the answers to the planes' asks) and the planes that came.

use std::time::Duration;

use lightcraft_pipeline::MaskView;
use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn masking() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    for (command, params) in [
        ("panel.masking", json!({})),
        ("mask.add", json!({"kind": "radial", "center": [0.4, 0.5], "rx": 0.2, "ry": 0.2})),
        ("mask.add", json!({"kind": "linear", "start": [0.2, 0.3], "end": [0.8, 0.7]})),
    ] {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
    }
    assert!(h.settle(SETTLE), "the renders settle");
    h
}

fn main_tex(h: &Headless) -> (lightcraft_catalog::PhotoId, u64) {
    let t = h.app.renderer.textures.get(&Slot::Main).expect("the loupe's render");
    (t.photo, t.key)
}

/// Photo renders finished so far (not counting the answers to asks for planes).
fn photo_renders(h: &Headless) -> u64 {
    h.app.renderer.completed - h.app.renderer.tints.answers as u64
}

/// Run frames until nothing renders, and say how many photo renders and plane deliveries came meanwhile.
fn during(h: &mut Headless, change: impl FnOnce(&mut Headless)) -> (u64, usize) {
    let (renders, planes) = (photo_renders(h), h.app.renderer.tints.delivered);
    change(h);
    assert!(h.settle(SETTLE), "the renders settle");
    (photo_renders(h) - renders, h.app.renderer.tints.delivered - planes)
}

fn key(h: &mut Headless, k: &str) {
    let r = h.request("ui.key", json!({"key": k}), T);
    assert_eq!(r["ok"], true, "{r}");
}

// Given a mask overlay over the loupe, toggling it with O any number of times, changing its opacity, colour and view, and
// showing every mask at once start no render of the photo, and the planes are asked for once, not for each of them
#[test]
fn the_overlay_toggle_and_its_opacity_start_no_render() {
    let mut h = masking();
    assert!(h.app.ui.mask_overlay, "a new mask shows its overlay");
    let (photo, k) = main_tex(&h);
    assert!(h.app.renderer.tints.has_masks(Slot::Main), "the planes of the selected mask came");
    assert!(h.app.renderer.tints.drawn(Slot::Main, photo, k), "the tint is up");
    let builds = h.app.renderer.tints.builds;

    for flip in 0..4 {
        let (renders, planes) = during(&mut h, |h| key(h, "o"));
        assert_eq!((renders, planes), (0, 0), "flip {flip} of O rendered or made planes");
        assert_eq!(h.app.ui.mask_overlay, flip % 2 == 1, "O flipped the overlay");
        assert_eq!(h.app.renderer.tints.drawn(Slot::Main, photo, k), h.app.ui.mask_overlay, "the tint follows the flag");
    }
    assert_eq!(h.app.renderer.tints.builds, builds, "a toggle makes no tint texture either");
    assert_eq!(main_tex(&h).1, k, "the photo's render is the one it was");

    for opacity in [80.0, 30.0, 5.0, 0.0, 100.0] {
        let got = during(&mut h, |h| h.app.ui.mask_overlay_opacity = opacity);
        assert_eq!(got, (0, 0), "opacity {opacity} rendered or made planes");
    }
    assert_eq!(h.app.renderer.tints.builds, builds, "the opacity is a tint of the draw: no texture is made");

    // the colour and the view change the tint's texture, not the photo, and ask for no planes
    for view in MaskView::ALL {
        let got = during(&mut h, |h| h.app.ui.mask_overlay_mode = view.name().to_string());
        assert_eq!(got, (0, 0), "{view:?} rendered or made planes");
        assert!(h.app.renderer.tints.drawn(Slot::Main, photo, k), "{view:?} is drawn");
    }
    let got = during(&mut h, |h| h.app.ui.mask_overlay_color = [30, 200, 60]);
    assert_eq!(got, (0, 0), "a colour rendered or made planes");

    // every mask at once: its planes are all the masks', and no photo render for it
    let (renders, planes) = during(&mut h, |h| {
        let r = h.request("engine.execute", json!({"command": "view.maskOverlayAll", "params": {}}), T);
        assert_eq!(r["ok"], true, "{r}");
    });
    assert_eq!((renders, planes), (0, 1), "Show All Masks rendered the photo, or asked for the planes other than once");
    assert_eq!(h.app.renderer.tints.planes(Slot::Main).expect("planes").planes.len(), 2, "both masks' planes");
    assert!(h.app.renderer.tints.drawn(Slot::Main, photo, k), "all the masks are drawn");
    // a mask's effect does change the photo's render, and the planes (the same ones) are not made again
    let (renders, planes) = during(&mut h, |h| {
        let r = h.request("engine.execute", json!({"command": "mask.adjust", "params": {"values": {"exposure": 0.5}}}), T);
        assert_eq!(r["ok"], true, "{r}");
    });
    assert!(renders >= 1, "an edit renders");
    assert_eq!(planes, 0, "an effect changes no plane");
}

// Given the overlay hidden, nothing is asked of the masks: edits, drags and strokes make no plane; showing it asks once,
// and what a slider drag does not change is not made again
#[test]
fn planes_are_made_only_while_the_overlay_is_shown() {
    let mut h = masking();
    key(&mut h, "o");
    assert!(!h.app.ui.mask_overlay);
    assert!(h.settle(SETTLE));
    let off = (h.app.renderer.tints.answers, h.app.renderer.tints.delivered);
    // edits and a slider drag while it is hidden
    for (command, params) in [
        ("mask.adjust", json!({"values": {"saturation": 20.0}})),
        ("develop.set", json!({"control": "light.exposure", "value": 0.3})),
        ("develop.beginInteraction", json!({"label": "Exposure"})),
        ("develop.set", json!({"control": "light.exposure", "value": 0.4})),
        ("develop.set", json!({"control": "light.exposure", "value": 0.5})),
        ("develop.endInteraction", json!({})),
    ] {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
        h.settle(SETTLE);
    }
    assert_eq!((h.app.renderer.tints.answers, h.app.renderer.tints.delivered), off, "a hidden overlay asked for planes");
    // shown again: one ask beside the render the photo has now (no photo render); the planes held are still the right ones
    // (only effects changed meanwhile), so none come, and they are drawn
    let asks = h.app.renderer.tints.answers;
    let (renders, planes) = during(&mut h, |h| key(h, "o"));
    assert_eq!((renders, planes), (0, 0), "showing it renders the photo, or makes planes the held ones are");
    assert_eq!(h.app.renderer.tints.answers, asks + 1, "showing it asks once");
    let (photo, k) = main_tex(&h);
    assert!(h.app.renderer.tints.drawn(Slot::Main, photo, k));
    // a slider drag with it shown asks for nothing, and the planes after it are the ones held
    h.app.ui.dragging_control = Some("light.exposure".into());
    let before = h.app.renderer.tints.answers;
    for (command, params) in [
        ("develop.beginInteraction", json!({"label": "Exposure"})),
        ("develop.set", json!({"control": "light.exposure", "value": 0.6})),
        ("develop.set", json!({"control": "light.exposure", "value": 0.7})),
    ] {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
        h.settle(SETTLE);
    }
    assert_eq!(h.app.renderer.tints.answers, before, "a slider drag asked for planes");
    h.app.ui.dragging_control = None;
    let (_, planes) = during(&mut h, |h| {
        let r = h.request("engine.execute", json!({"command": "develop.endInteraction", "params": {}}), T);
        assert_eq!(r["ok"], true, "{r}");
    });
    assert_eq!(planes, 0, "the planes held were still the right ones after the drag");
    assert!(h.app.renderer.tints.answers > before, "it asks once the drag is over");
}

// Given a render that came with the planes of the selected mask, they are that mask's alpha
#[test]
fn the_tint_is_the_selected_masks_alpha() {
    let h = masking();
    let selected = h.app.session.active_mask.expect("a selected mask");
    let held = h.app.renderer.tints.planes(Slot::Main).expect("the masks");
    assert_eq!(held.planes.len(), 1, "only the selected mask's plane is made");
    assert_eq!(held.planes[0].0, selected);
    assert!(held.plane(selected).is_some_and(|p| p.iter().any(|a| *a > 0)), "the mask selects something");
}

struct NoScreen;

impl crate::render::ScreenTextures for NoScreen {
    fn show(&mut self, _: &lightcraft_engine::media::ScreenFrame, _: Option<egui::TextureId>) -> Option<egui::TextureId> {
        None
    }
    fn free(&mut self, _: egui::TextureId) {}
}

// Given Color Overlay on B&W (its grey is made from the photo's pixels), a drag's drafts are not left on the GPU, where there are no
// pixels and the tint would show over the colour photo; every other view keeps them there
#[test]
fn color_overlay_on_bw_keeps_its_drafts_off_the_gpu() {
    let mut h = masking();
    h.app.renderer.set_screen_textures(Box::new(NoScreen));
    let id = h.app.session.active().expect("a photo");
    let draft = |h: &mut Headless, slot: Slot| {
        let job = h.app.session.loupe_job(id, 300, 300, true).expect("a job").draft();
        h.app.renderer.job_for(slot, job).on_screen
    };
    assert!(draft(&mut h, Slot::Main), "a draft is left on the GPU where the window can draw it");
    h.app.ui.mask_overlay_mode = MaskView::ColorOnBw.name().to_string();
    h.settle(SETTLE);
    assert!(h.app.renderer.tints.keep_region, "the view makes its grey from the pixels");
    assert!(!draft(&mut h, Slot::Main), "Color Overlay on B&W: the draft comes back as pixels");
    assert!(!draft(&mut h, Slot::Region));
    h.app.ui.mask_overlay_mode = MaskView::Color.name().to_string();
    h.settle(SETTLE);
    assert!(draft(&mut h, Slot::Main), "another view: back on the GPU");
    // (the photo's own full renders were never left there)
    let full = h.app.session.loupe_job(id, 300, 300, true).expect("a job");
    assert!(!h.app.renderer.job_for(Slot::Main, full).on_screen);
}
