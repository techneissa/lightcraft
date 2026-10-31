//! Lightroom's ⌥-drag preview: while ⌥ is held during a drag of Exposure, Whites, Highlights, Blacks,
//! Shadows or a Sharpening slider, the loupe shows a mask instead of the photo; it ends with the drag
//! or with ⌥. `view.clippingPreview` holds the same preview for agents.

use std::time::Duration;

use egui::{Event, Modifiers, PointerButton};
use lightcraft_develop::DevelopSettings;
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
    crate::panels::detail::view_overlay(&h.app, &DevelopSettings::default())
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
    let t = track(&h, control);
    // from the middle of the track (the demo photo's defaults sit at or near it) and rightwards
    let from = egui::pos2(t.center().x, t.center().y);
    let step = egui::vec2(t.width() * 0.04, 0.0);
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from));
    press(&mut h, from, Modifiers::ALT, true);
    for i in 1..=4 {
        h.app.synthetic.push(Event::PointerMoved(from + step * i as f32));
    }
    frames(&mut h, 10);
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some(control), "the drag is going");
    assert_eq!(h.app.ui.alt_preview, Some(want), "⌥ held: {control} previews {want:?}");
    assert_eq!(shown(&h), overlay, "and the loupe asks for it");
    // ⌥ let go while the mouse is still down: the photo is back
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::NONE));
    h.app.synthetic.push(Event::PointerMoved(from + step * 5.0));
    frames(&mut h, 10);
    assert!(h.app.ui.dragging_control.is_some(), "the drag still goes");
    assert_eq!(h.app.ui.alt_preview, None);
    assert_eq!(shown(&h), Overlay::None);
    // ⌥ again
    h.app.synthetic.push(Event::ModifiersChanged(Modifiers::ALT));
    h.app.synthetic.push(Event::PointerMoved(from + step * 6.0));
    frames(&mut h, 10);
    assert_eq!(h.app.ui.alt_preview, Some(want));
    // the release ends it, ⌥ still down
    press(&mut h, from + step * 6.0, Modifiers::ALT, false);
    h.settle(SETTLE);
    assert!(h.app.ui.dragging_control.is_none());
    assert_eq!(h.app.ui.alt_preview, None, "the preview ends with the drag");
    assert_eq!(shown(&h), Overlay::None);
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
