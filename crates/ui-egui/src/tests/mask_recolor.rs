//! Headless tests of the Masking panel's Recolor row (issue #83): the eyedropper picks the colour from the photo.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn adjust(h: &Headless) -> lightcraft_develop::LocalAdjustments {
    let id = h.app.session.active().expect("active photo");
    let mid = h.app.session.active_mask.expect("a selected mask");
    h.app.session.develop_of(id).unwrap_or_default().masks.iter().find(|m| m.id == mid).map(|m| m.adjust).unwrap_or_default()
}

/// The eyedropper button arms the picker; a click on the photo sets the Recolor colour (and turns it on) from that
/// point, and disarms it.
#[test]
fn the_eyedropper_picks_the_recolor_colour_from_the_photo() {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 2400.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "mask.add", "params": {"kind": "radial"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(adjust(&h).recolor, 0.0);
    let r = h.request("ui.clickWidget", json!({"id": "icon:recolorPicker"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.tool, crate::panels::mask_recolor::PICK_TOOL);
    let img = h.app.image_rect.expect("the photo on screen");
    let pos = img.center();
    h.app.synthetic.push(egui::Event::PointerMoved(pos));
    h.app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.settle(SETTLE);
    let a = adjust(&h);
    assert_eq!(a.recolor, 100.0, "picking turns Recolor on");
    assert!(a.recolor_r + a.recolor_g + a.recolor_b > 0.0, "a colour from the photo: {a:?}");
    assert_eq!(h.app.ui.tool, "", "one pick disarms it");
}

/// The Colour to Replace eyedropper sets the mask's Colour to Replace (not its new colour); Automatic clears it.
#[test]
fn the_colour_to_replace_eyedropper_sets_the_source_colour() {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 2400.0], 1.0);
    h.request("ui.set", json!({"view": "detail"}), T);
    h.request("engine.execute", json!({"command": "panel.masking"}), T);
    h.request("engine.execute", json!({"command": "mask.add", "params": {"kind": "radial"}}), T);
    h.request("engine.execute", json!({"command": "mask.recolor", "params": {"color": "#7a3fb0"}}), T);
    h.settle(SETTLE);
    let r = h.request("ui.clickWidget", json!({"id": "icon:recolorFromPicker"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    let pos = h.app.image_rect.expect("the photo on screen").center();
    h.app.synthetic.push(egui::Event::PointerMoved(pos));
    h.app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.settle(SETTLE);
    let a = adjust(&h);
    assert!(lightcraft_pipeline::recolor::source_rgb(&a).is_some(), "a Colour to Replace: {a:?}");
    assert!((a.recolor_r - 0x7a as f64 / 255.0).abs() < 1e-9, "the new colour stays");
    assert_eq!(h.app.ui.tool, "");
    let r = h.request("ui.clickWidget", json!({"id": "button:recolorFromAuto"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(lightcraft_pipeline::recolor::source_rgb(&adjust(&h)).is_none(), "Automatic clears it");
}
