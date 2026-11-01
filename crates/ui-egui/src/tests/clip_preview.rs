//! Official's ⌥-drag preview (`Overlay::for_control`) on a mask's own sliders (Masking panel), and the
//! clipping overlay while a mask is edited: the mask's tint steps aside while a clipping side shows.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton};
use lightcraft_pipeline::Overlay;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

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

/// The ⌥ press-drag-release on the slider `control` of whatever panel `h` shows.
fn alt_drag_in(h: &mut Headless, control: &str, overlay: Overlay) {
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
    assert_eq!(shown(h), overlay, "and the loupe asks for it");
    // ⌥ let go while the mouse is still down: the photo is back
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
    h.app.synthetic.push(Event::PointerMoved(from + step * 5.0));
    frames(h, 10);
    assert!(h.app.ui.dragging_control.is_some(), "the drag still goes");
    assert_eq!(shown(h), Overlay::None);
    // ⌥ again
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from + step * 6.0));
    frames(h, 10);
    // the release ends it, ⌥ still down
    press(h, from + step * 6.0, Modifiers::ALT, false);
    h.settle(SETTLE);
    assert!(h.app.ui.dragging_control.is_none());
    assert_eq!(shown(h), Overlay::None);
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
fn mask_alt_drag(control: &str, overlay: Overlay) {
    let mut h = mask_panel(control);
    assert!(matches!(shown(&h), Overlay::Mask { .. }), "the mask overlay is up before the drag");
    alt_drag_in(&mut h, control, overlay);
}

#[test]
fn alt_drag_of_a_masks_exposure_whites_and_highlights_shows_clipped_highlights() {
    for control in ["exposure", "whites", "highlights"] {
        mask_alt_drag(control, Overlay::ClipHighlights);
    }
}

#[test]
fn alt_drag_of_a_masks_blacks_and_shadows_shows_crushed_shadows() {
    for control in ["blacks", "shadows"] {
        mask_alt_drag(control, Overlay::ClipShadows);
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
    assert!(crate::panels::clipping::alt_overlay(&h.app).is_none(), "Contrast has no ⌥ preview");
    press(&mut h, from, Modifiers::ALT, false);
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
    h.settle(SETTLE);
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
