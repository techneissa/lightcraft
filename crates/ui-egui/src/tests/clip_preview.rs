//! Lightroom's ⌥-drag preview: while ⌥ is held during a drag of Exposure, Whites, Highlights, Blacks,
//! Shadows or a Sharpening slider, the loupe shows a mask instead of the photo; it ends with the drag
//! or with ⌥. `view.clippingPreview` holds the same preview for agents.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton};
use lightcraft_pipeline::Overlay;
use serde_json::json;

use crate::headless::Headless;
use crate::panels::clipping::Preview;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The Edit panel with `section` open alone, so its sliders are on screen.
fn edit_panel(section: &str) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    let id = h.app.session.visible_cloned()[0].0;
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [id]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "openSections": [section]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn shown(h: &Headless) -> Overlay {
    let d = h.app.session.active().and_then(|id| h.app.session.develop_of(id)).map(|d| (*d).clone()).unwrap_or_default();
    crate::panels::detail::view_overlay(&h.app, &d, h.view.ctx.cumulative_frame_nr())
}

fn track(h: &Headless, control: &str) -> egui::Rect {
    let id = format!("slider:{control}");
    h.app.widgets.iter().rev().find(|(w, _)| *w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id} on screen"))
}

fn press(h: &mut Headless, at: egui::Pos2, modifiers: Modifiers, pressed: bool) {
    h.app.synthetic.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers });
}

fn frames(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

/// A drag of `control`'s track with ⌥ held down from the press: the mask shows while it goes,
/// goes when ⌥ is let go, comes back with ⌥, and is gone for good after the release.
fn alt_drag(control: &str, want: Preview, overlay: Overlay) {
    let mut h = edit_panel(if control.starts_with("detail.") { "detail" } else { "light" });
    if control.starts_with("detail.") {
        // Radius, Detail and Masking are greyed out until something sharpens
        let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"values": {"detail.sharpenAmount": 40}}}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(SETTLE);
    }
    assert_eq!(shown(&h), Overlay::None);
    alt_drag_in(&mut h, control, want, overlay);
}

/// The ⌥ press-drag-release on the slider `control` of whatever panel `h` shows.
fn alt_drag_in(h: &mut Headless, control: &str, want: Preview, overlay: Overlay) {
    let t = track(h, control);
    // from the middle of the track (the demo photo's defaults sit at or near it) and rightwards
    let from = egui::pos2(t.center().x, t.center().y);
    let step = egui::vec2(t.width() * 0.04, 0.0);
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from));
    press(h, from, Modifiers::ALT, true);
    for i in 1..=4 {
        h.app.synthetic.push(Event::PointerMoved(from + step * i as f32));
    }
    frames(h, 10);
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some(control), "the drag is going");
    assert_eq!(h.app.ui.alt_preview, Some(want), "⌥ held: {control} previews {want:?}");
    assert_eq!(shown(h), overlay, "and the loupe asks for it");
    // ⌥ let go while the mouse is still down: the photo is back
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
    h.app.synthetic.push(Event::PointerMoved(from + step * 5.0));
    frames(h, 10);
    assert!(h.app.ui.dragging_control.is_some(), "the drag still goes");
    assert_eq!(h.app.ui.alt_preview, None);
    assert_eq!(shown(h), Overlay::None);
    // ⌥ again
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from + step * 6.0));
    frames(h, 10);
    assert_eq!(h.app.ui.alt_preview, Some(want));
    // the release ends it, ⌥ still down
    press(h, from + step * 6.0, Modifiers::ALT, false);
    h.settle(SETTLE);
    assert!(h.app.ui.dragging_control.is_none());
    assert_eq!(h.app.ui.alt_preview, None, "the preview ends with the drag");
    assert_eq!(shown(h), Overlay::None);
}

#[test]
fn alt_drag_of_exposure_whites_and_highlights_shows_clipped_highlights() {
    for control in ["light.exposure", "light.whites", "light.highlights"] {
        alt_drag(control, Preview::Highlights, Overlay::ClipHighlights);
    }
}

#[test]
fn alt_drag_of_blacks_and_shadows_shows_crushed_shadows() {
    for control in ["light.blacks", "light.shadows"] {
        alt_drag(control, Preview::Shadows, Overlay::ClipShadows);
    }
}

#[test]
fn alt_drag_of_sharpening_masking_shows_the_mask_and_the_others_a_greyscale_photo() {
    alt_drag("detail.sharpenMasking", Preview::SharpenMask, Overlay::SharpenMask);
    for control in ["detail.sharpenAmount", "detail.sharpenRadius", "detail.sharpenDetail"] {
        alt_drag(control, Preview::Greyscale, Overlay::Greyscale);
    }
}

/// The Masking panel with a linear mask selected and its sliders on screen (the dock header pushes
/// them below the first viewport: scroll until `control` is inside the panel).
fn mask_panel(control: &str) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    for (command, params) in [
        ("panel.masking", json!({})),
        ("mask.add", json!({"kind": "linear", "start": [0.5, 0.2], "end": [0.5, 0.7]})),
        // something to clip: the mask's own adjustment is part of the render the preview shows
        ("mask.adjust", json!({"values": {"exposure": 1.0}})),
        ("view.maskOverlay", json!({"show": true})),
    ] {
        let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(r["ok"], true, "{command}: {r}");
    }
    h.settle(SETTLE);
    let panel = h.app.widgets.iter().find(|(id, _)| id == "panel:right_panel").map(|(_, r)| *r).expect("the right panel");
    for _ in 0..8 {
        if panel.contains_rect(track(&h, control)) {
            break;
        }
        h.request("ui.move", json!({"x": panel.center().x, "y": panel.center().y}), T);
        h.request("ui.scroll", json!({"dy": -160.0}), T);
        h.settle(SETTLE);
    }
    assert!(panel.contains_rect(track(&h, control)), "{control} is on screen");
    h
}

/// A mask's own sliders preview like the global ones, with the mask overlay up before the drag.
fn mask_alt_drag(control: &str, want: Preview, overlay: Overlay) {
    let mut h = mask_panel(control);
    assert!(matches!(shown(&h), Overlay::Mask { .. }), "the mask overlay is up before the drag");
    alt_drag_in(&mut h, control, want, overlay);
}

#[test]
fn alt_drag_of_a_masks_exposure_whites_and_highlights_shows_clipped_highlights() {
    for control in ["exposure", "whites", "highlights"] {
        mask_alt_drag(control, Preview::Highlights, Overlay::ClipHighlights);
    }
}

#[test]
fn alt_drag_of_a_masks_blacks_and_shadows_shows_crushed_shadows() {
    for control in ["blacks", "shadows"] {
        mask_alt_drag(control, Preview::Shadows, Overlay::ClipShadows);
    }
}

#[test]
fn a_mask_drag_without_alt_or_of_another_mask_slider_shows_the_mask() {
    let mut h = mask_panel("contrast");
    let t = track(&h, "contrast");
    let from = t.center();
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from));
    press(&mut h, from, Modifiers::ALT, true);
    for i in 1..=4 {
        h.app.synthetic.push(Event::PointerMoved(from + egui::vec2(t.width() * 0.04 * i as f32, 0.0)));
    }
    frames(&mut h, 10);
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some("contrast"));
    assert_eq!(h.app.ui.alt_preview, None, "Contrast has no ⌥ preview");
    press(&mut h, from, Modifiers::ALT, false);
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
    h.settle(SETTLE);
}

#[test]
fn a_drag_without_alt_or_of_another_slider_shows_the_photo() {
    let mut h = edit_panel("light");
    for (control, modifiers) in [("light.exposure", Modifiers::NONE), ("light.contrast", Modifiers::ALT)] {
        let t = track(&h, control);
        let from = t.center();
        h.app.synthetic.push(Event::ModifiersChanged(modifiers));
        h.app.synthetic.push(Event::PointerMoved(from));
        press(&mut h, from, modifiers, true);
        for i in 1..=4 {
            h.app.synthetic.push(Event::PointerMoved(from + egui::vec2(t.width() * 0.04 * i as f32, 0.0)));
        }
        frames(&mut h, 10);
        assert_eq!(h.app.ui.dragging_control.as_deref(), Some(control));
        assert_eq!(h.app.ui.alt_preview, None, "{control} with {modifiers:?}");
        assert_eq!(shown(&h), Overlay::None);
        press(&mut h, from, modifiers, false);
        h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
        h.settle(SETTLE);
    }
}

#[test]
fn the_command_holds_a_preview_until_it_is_cleared() {
    let mut h = edit_panel("light");
    for (params, want, overlay) in [
        (json!({"mode": "highlights"}), Preview::Highlights, Overlay::ClipHighlights),
        (json!({"mode": "shadows"}), Preview::Shadows, Overlay::ClipShadows),
        (json!({"mode": "sharpenMask"}), Preview::SharpenMask, Overlay::SharpenMask),
        (json!({"mode": "greyscale"}), Preview::Greyscale, Overlay::Greyscale),
        (json!({"control": "light.blacks"}), Preview::Shadows, Overlay::ClipShadows),
    ] {
        let r = h.app.run("view.clippingPreview", params).unwrap();
        assert_eq!(r["mode"], want.name());
        h.step();
        assert_eq!(shown(&h), overlay);
    }
    assert!(h.app.run("view.clippingPreview", json!({"mode": "sepia"})).is_err());
    assert!(h.app.run("view.clippingPreview", json!({"control": "light.contrast"})).is_err());
    assert_eq!(h.app.ui.preview_forced, Some(Preview::Shadows), "a refused request leaves the preview as it was");
    for off in [json!({"mode": "off"}), json!({})] {
        h.app.run("view.clippingPreview", json!({"mode": "greyscale"})).unwrap();
        let r = h.app.run("view.clippingPreview", off).unwrap();
        assert!(r["mode"].is_null());
        assert_eq!(shown(&h), Overlay::None);
    }
}

/// The loupe renders the preview: black and white pixels only for the clipping views.
#[test]
fn the_loupe_draws_the_clipping_preview() {
    let mut h = edit_panel("light");
    let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"values": {"light.exposure": 2.0}}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.run("view.clippingPreview", json!({"mode": "highlights"})).unwrap();
    h.settle(SETTLE);
    let pixels = h.app.renderer.textures.get(&crate::render::Slot::Main).and_then(|t| t.pixels.clone()).expect("the loupe keeps its pixels");
    // (at rest the render is shrunk to the window with a Lanczos filter, which softens the edges)
    let extreme = pixels.pixels.iter().filter(|c| [c.r(), c.g(), c.b()].iter().all(|&v| v <= 8 || v >= 247)).count();
    assert!(extreme * 100 >= pixels.pixels.len() * 90, "only clipped channels show: {extreme} of {}", pixels.pixels.len());
    assert!(pixels.pixels.iter().any(|c| c.r() == 255), "+2 EV clips something");
    h.app.run("view.clippingPreview", json!({})).unwrap();
    h.settle(SETTLE);
    let plain = h.app.renderer.textures.get(&crate::render::Slot::Main).and_then(|t| t.pixels.clone()).expect("pixels");
    assert!(plain.pixels.iter().any(|c| c.r() != 0 && c.r() != 255), "the photo is back");
}

/// The J overlay (`view.clippingHighlights`) marks what the photo clips: the mask's red tint would
/// show as clipping in the render it reads, so the tint steps aside while a clipping side is on.
#[test]
fn the_clipping_overlay_shows_while_a_mask_is_edited_and_the_tint_steps_aside() {
    let mut h = mask_panel("exposure");
    assert!(matches!(shown(&h), Overlay::Mask { .. }), "the mask tint is up");
    h.app.run("view.clippingHighlights", json!({})).unwrap();
    h.settle(SETTLE);
    h.step();
    assert!(h.app.ui.clip_highlights);
    assert_eq!(shown(&h), Overlay::None, "the render is the photo itself");
    let o = h.app.renderer.clip_overlay.as_ref().expect("the clipping overlay is built over the mask's photo");
    assert!(o.made_for.3 && !o.made_for.2, "highlights only");
    let src = h.app.renderer.textures.get(&crate::render::Slot::Main).and_then(|t| t.pixels.clone()).expect("pixels");
    let hit = crate::panels::clipping::overlay_image(&src, false, true).pixels.iter().filter(|c| **c != egui::Color32::TRANSPARENT).count();
    assert!(hit > 1000, "the mask's +1 EV clips the sky: {hit} pixels");
    // off again: the tint is back
    h.app.run("view.clippingHighlights", json!({})).unwrap();
    h.settle(SETTLE);
    assert!(matches!(shown(&h), Overlay::Mask { .. }));
}

/// A pointer resting on a histogram triangle shows that side's clipping overlay, so the mask tint
/// steps aside for as long as the hover is fresh, and is back once it ends.
#[test]
fn hovering_a_clipping_triangle_hides_the_mask_tint_until_the_hover_ends() {
    let mut h = mask_panel("exposure");
    assert!(matches!(shown(&h), Overlay::Mask { .. }), "the mask tint is up");
    assert!(!h.app.ui.clip_highlights && !h.app.ui.clip_shadows, "no toggle is on");
    // the histogram sets the hover each frame the pointer rests on its highlights triangle
    h.app.ui.clip_hover = ([false, true], h.view.ctx.cumulative_frame_nr());
    h.step();
    assert_eq!(shown(&h), Overlay::None, "the hovered side's overlay reads the photo, not the tint");
    // the pointer leaves: the histogram stores no hover, and the tint is back a frame later
    h.app.ui.clip_hover = ([false, false], h.view.ctx.cumulative_frame_nr());
    h.step();
    assert!(matches!(shown(&h), Overlay::Mask { .. }), "the tint is back after the hover");
    // a hover nothing refreshes (a hidden histogram) goes stale instead of hiding the tint for good
    h.app.ui.clip_hover = ([true, false], h.view.ctx.cumulative_frame_nr());
    frames(&mut h, 4);
    assert!(matches!(shown(&h), Overlay::Mask { .. }), "a stale hover is ignored");
}
