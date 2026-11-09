//! Headless tests of clicking a pin on the photo: the Brush and Colour Range tools select another mask's pin instead
//! of painting or sampling, a Red Eye ellipse has a pin and handles, and every pin is at least 10 px to hit.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn detail(panel: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": panel}), T);
    assert_eq!(r["ok"], true, "{r}");
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn develop(h: &Headless) -> lightcraft_develop::DevelopSettings {
    let id = h.app.session.active().expect("active photo");
    (*h.app.session.develop_of(id).unwrap_or_default()).clone()
}

/// The screen position of a widget the overlay registered.
fn at(h: &Headless, id: &str) -> egui::Pos2 {
    h.app
        .widgets
        .iter()
        .find(|(w, _)| w == id)
        .map(|(_, r)| r.center())
        .unwrap_or_else(|| panic!("no widget {id}: {:?}", h.app.widgets.iter().map(|w| w.0.clone()).collect::<Vec<_>>()))
}

/// A gesture in screen points: press at the first point, move through the rest, release at the last.
fn gesture(h: &mut Headless, pts: &[egui::Pos2]) {
    let mut events = Vec::new();
    for (i, q) in pts.iter().enumerate() {
        let kind = if i == 0 { "down" } else { "drag" };
        events.push(json!({"kind": kind, "x": q.x, "y": q.y}));
    }
    if let Some(q) = pts.last() {
        events.push(json!({"kind": "up", "x": q.x, "y": q.y}));
    }
    let r = h.request("ui.pointer", json!({"events": events, "screen": true}), T);
    assert_eq!(r["ok"], true, "{r}");
}

fn click(h: &mut Headless, q: egui::Pos2) {
    gesture(h, &[q]);
}

/// Two radial masks, the second selected, pins always shown, `tool` active.
fn two_masks(tool: &str) -> Headless {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.5], "rx": 0.05, "ry": 0.05}));
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.7, 0.5], "rx": 0.05, "ry": 0.05}));
    assert_eq!(h.app.session.active_mask, Some(2));
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    h.app.ui.tool = tool.into();
    h.settle(SETTLE);
    h
}

/// With the Brush tool, a plain click on another mask's pin selects that mask and paints no dab. A drag that starts on
/// the pin is an ordinary stroke into the selected mask. The selected mask's own pin is painted over as anywhere else.
#[test]
fn the_brush_tool_selects_on_a_click_but_a_drag_from_a_pin_paints() {
    let mut h = two_masks("brush");
    let before = develop(&h).masks;
    let other = at(&h, "maskPin:1:0");
    click(&mut h, other);
    h.settle(SETTLE);
    assert_eq!(h.app.session.active_mask, Some(1), "the pinned mask is selected");
    assert_eq!(develop(&h).masks, before, "no dab was painted");
    assert_eq!(h.app.ui.tool, "brush", "the tool stays on");
    // a drag that starts on the other pin paints into the selected mask and selects nothing
    let pin2 = at(&h, "maskPin:2:0");
    gesture(&mut h, &[pin2, pin2 + egui::vec2(20.0, 0.0), pin2 + egui::vec2(40.0, 10.0)]);
    h.settle(SETTLE);
    assert_eq!(h.app.session.active_mask, Some(1), "a drag selects nothing");
    assert_ne!(develop(&h).masks, before, "the stroke went into the selected mask");
    // the selected mask's own pin: a click paints
    let before = develop(&h).masks;
    let own = at(&h, "maskPin:1:0");
    click(&mut h, own);
    h.settle(SETTLE);
    assert_eq!(h.app.session.active_mask, Some(1));
    assert_ne!(develop(&h).masks, before, "a dab went into the selected mask");
}

/// With Colour Range, a click on another mask's pin selects that mask and samples no colour for the selected one.
#[test]
fn colour_range_selects_the_mask_whose_pin_is_clicked_without_sampling() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "colorRange"}));
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.7, 0.5], "rx": 0.05, "ry": 0.05}));
    exec(&mut h, "mask.select", json!({"id": 1}));
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    h.app.ui.tool = "colorRange".into();
    h.settle(SETTLE);
    let samples = |h: &Headless| {
        develop(h)
            .masks
            .iter()
            .flat_map(|m| &m.components)
            .map(|c| if let lightcraft_develop::MaskShape::ColorRange { samples, .. } = &c.shape { samples.len() } else { 0 })
            .sum::<usize>()
    };
    assert_eq!(samples(&h), 0);
    let pin = at(&h, "maskPin:2:0");
    click(&mut h, pin);
    h.settle(SETTLE);
    assert_eq!(h.app.session.active_mask, Some(2), "the pinned mask is selected");
    assert_eq!(samples(&h), 0, "no colour was sampled");
    // a drag that starts on a pin selects nothing and samples nothing either
    exec(&mut h, "mask.select", json!({"id": 1}));
    h.settle(SETTLE);
    gesture(&mut h, &[pin, pin + egui::vec2(20.0, 0.0), pin + egui::vec2(40.0, 10.0)]);
    h.settle(SETTLE);
    assert_eq!(h.app.session.active_mask, Some(1), "a drag from another mask's pin does not select it");
    assert_eq!(samples(&h), 0);
    // (a click elsewhere on the photo samples for the selected colour range, so the check above can fail)
    exec(&mut h, "mask.select", json!({"id": 1}));
    h.settle(SETTLE);
    click(&mut h, pin + egui::vec2(0.0, 150.0));
    h.settle(SETTLE);
    assert_eq!(samples(&h), 1, "a click on the photo samples a colour");
}

/// A selected Red Eye ellipse has a pin and edge handles: the pin moves it, an edge resizes it about its centre,
/// and Delete removes it. One undo step each.
#[test]
fn a_red_eye_ellipse_moves_with_its_pin_and_resizes_with_its_edges() {
    let mut h = detail("panel.redeye");
    exec(&mut h, "redeye.add", json!({"center": [0.5, 0.5], "rx": 0.05, "ry": 0.04}));
    h.app.ui.eye = 0;
    h.settle(SETTLE);
    let start = develop(&h).red_eye[0];
    // the pin: drag it right and down by (60, 30) points
    let pin = at(&h, "eyePin:0");
    gesture(&mut h, &[pin, pin + egui::vec2(20.0, 10.0), pin + egui::vec2(60.0, 30.0)]);
    h.settle(SETTLE);
    let moved = develop(&h).red_eye[0];
    assert!(moved.center.x > start.center.x && moved.center.y > start.center.y, "{start:?} -> {moved:?}");
    assert_eq!((moved.rx, moved.ry), (start.rx, start.ry), "moving keeps the size");
    // the right edge: drag it out by 40 points
    let edge = at(&h, "eyeHandle:0:1");
    gesture(&mut h, &[edge, edge + egui::vec2(20.0, 0.0), edge + egui::vec2(40.0, 0.0)]);
    h.settle(SETTLE);
    let wider = develop(&h).red_eye[0];
    assert!(wider.rx > moved.rx, "{moved:?} -> {wider:?}");
    assert_eq!((wider.center, wider.ry), (moved.center, moved.ry), "only the horizontal radius changed");
    // the top edge: drag it up
    let top = at(&h, "eyeHandle:0:4");
    gesture(&mut h, &[top, top - egui::vec2(0.0, 15.0), top - egui::vec2(0.0, 30.0)]);
    h.settle(SETTLE);
    let taller = develop(&h).red_eye[0];
    assert!(taller.ry > wider.ry, "{wider:?} -> {taller:?}");
    assert_eq!(taller.rx, wider.rx);
    // no new eye came of any of it
    assert_eq!(develop(&h).red_eye.len(), 1);
    // Delete removes the selected eye
    let r = h.request("ui.key", json!({"key": "delete"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(develop(&h).red_eye.is_empty());
}

/// A click on another eye's pin selects that eye, even when the ellipse is tiny.
#[test]
fn a_click_on_a_red_eye_pin_selects_that_eye() {
    let mut h = detail("panel.redeye");
    exec(&mut h, "redeye.add", json!({"center": [0.3, 0.5], "rx": 0.002, "ry": 0.002}));
    exec(&mut h, "redeye.add", json!({"center": [0.7, 0.5], "rx": 0.05, "ry": 0.05}));
    h.app.ui.eye = 1;
    h.settle(SETTLE);
    let pin = at(&h, "eyePin:0");
    click(&mut h, pin + egui::vec2(7.0, 0.0));
    h.settle(SETTLE);
    assert_eq!(h.app.ui.eye, 0);
    assert_eq!(develop(&h).red_eye.len(), 2);
}

/// A spot of the smallest size still has a pin 10 px wide: a click 9 points off its centre selects it, where the
/// spot's own radius (about a point) would have missed and painted a new spot.
#[test]
fn a_tiny_spots_pin_is_still_clickable_at_ten_points() {
    let mut h = detail("panel.remove");
    exec(&mut h, "spot.add", json!({"mode": "heal", "points": [[0.3, 0.4]], "size": 0.001}));
    exec(&mut h, "spot.add", json!({"mode": "heal", "points": [[0.7, 0.4]], "size": 0.001}));
    assert_eq!(h.app.session.active_spot, Some(1));
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    h.settle(SETTLE);
    let pin = at(&h, "spotPin:0");
    click(&mut h, pin + egui::vec2(6.0, 6.0));
    assert!(h.settle_spot_sources(), "a spot's source search never answered");
    assert_eq!(h.app.session.active_spot, Some(0), "the pin was hit about 8.5 points off its centre");
    assert_eq!(develop(&h).spots.len(), 2, "no spot was added");
}

#[test]
fn the_pin_nearest_the_pointer_within_the_hit_distance_is_the_one_found() {
    use crate::panels::detail::masks::{PIN_HIT, nearest_within};
    let pins = [egui::pos2(100.0, 100.0), egui::pos2(106.0, 100.0)];
    assert_eq!(nearest_within(pins, egui::pos2(104.0, 100.0), PIN_HIT), Some(pins[1]));
    assert_eq!(nearest_within(pins, egui::pos2(100.0, 109.0), PIN_HIT), Some(pins[0]));
    assert_eq!(nearest_within(pins, egui::pos2(100.0, 111.0), PIN_HIT), None);
}

/// On a small eye a press near the centre takes the pin (the handles are within reach of it): the eye moves and keeps
/// its size. A big eye's edge handle still resizes (see the move-and-resize test above).
#[test]
fn a_press_inside_the_pins_circle_of_a_small_red_eye_moves_it() {
    let mut h = detail("panel.redeye");
    exec(&mut h, "redeye.add", json!({"center": [0.5, 0.5], "rx": 0.008, "ry": 0.008}));
    h.app.ui.eye = 0;
    h.settle(SETTLE);
    let start = develop(&h).red_eye[0];
    let (pin, edge) = (at(&h, "eyePin:0"), at(&h, "eyeHandle:0:1"));
    assert!(pin.distance(edge) < 10.0, "the handle is inside the pin's hit circle: {}", pin.distance(edge));
    // the press is on the handle, which is inside the pin's circle: it moves the eye
    gesture(&mut h, &[edge, edge + egui::vec2(10.0, 5.0), edge + egui::vec2(30.0, 15.0)]);
    h.settle(SETTLE);
    let after = develop(&h).red_eye[0];
    assert!(after.center.x > start.center.x && after.center.y > start.center.y, "{start:?} -> {after:?}");
    assert_eq!((after.rx, after.ry), (start.rx, start.ry), "moved, not resized");
}

/// The selected eye is per photo: another photo starts at its first correction, and a drag that began on one photo
/// ends when the photo changes.
#[test]
fn the_selected_red_eye_follows_the_photo() {
    let mut h = detail("panel.redeye");
    let first = h.app.session.active().expect("active photo");
    exec(&mut h, "redeye.add", json!({"center": [0.3, 0.5], "rx": 0.03, "ry": 0.03}));
    exec(&mut h, "redeye.add", json!({"center": [0.6, 0.5], "rx": 0.03, "ry": 0.03}));
    h.app.ui.eye = 1;
    h.settle(SETTLE);
    assert_eq!(h.app.ui.eye_for(first), 1);
    // a drag in progress on the first photo
    let pin = at(&h, "eyePin:1");
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": pin.x, "y": pin.y}, {"kind": "drag", "x": pin.x + 10.0, "y": pin.y}, {"kind": "drag", "x": pin.x + 20.0, "y": pin.y}], "screen": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(matches!(h.app.gesture, Some(crate::panels::detail::Gesture::EyeEdit { photo, .. }) if photo == first), "{:?}", h.app.gesture);
    // another photo becomes active
    let other = h.app.session.visible().iter().copied().find(|id| *id != first).expect("another photo");
    exec(&mut h, "library.select", json!({"ids": [other.0]}));
    h.settle(SETTLE);
    assert!(h.app.gesture.is_none(), "the drag ended with the photo change: {:?}", h.app.gesture);
    assert_eq!(h.app.ui.eye_for(other), 0, "the other photo starts at its first correction");
    // …and a photo with fewer corrections never shows a stale index
    assert_eq!(h.app.ui.eye_for(other), 0);
    let r = h.request("ui.pointer", json!({"events": [{"kind": "up", "x": pin.x, "y": pin.y}], "screen": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(develop(&h).red_eye.len(), 0, "the other photo got no eye");
}

/// Where two pins are both within reach, a press takes the nearer one, even when the farther is the selected mask's;
/// the selected mask's goes first only when they are equally near. The pin that lights is the one taken.
#[test]
fn the_nearest_mask_pin_is_taken_not_the_selected_ones() {
    use crate::panels::detail::masks::grip_at;
    let grips = [(1, 0, 0u8, egui::pos2(100.0, 100.0)), (2, 0, 0u8, egui::pos2(108.0, 100.0))];
    // mask 2 is selected; the press is nearer mask 1's pin
    assert_eq!(grip_at(&grips, Some(2), egui::pos2(102.0, 100.0)).map(|g| g.0), Some(1));
    assert_eq!(grip_at(&grips, Some(2), egui::pos2(107.0, 100.0)).map(|g| g.0), Some(2));
    // equally near: the selected one
    assert_eq!(grip_at(&grips, Some(2), egui::pos2(104.0, 100.0)).map(|g| g.0), Some(2));
    assert_eq!(grip_at(&grips, Some(1), egui::pos2(104.0, 100.0)).map(|g| g.0), Some(1));
    assert_eq!(grip_at(&grips, Some(1), egui::pos2(120.0, 100.0)), None);

    // and on the photo: mask 2 (selected) sits a few points from mask 1; a click near mask 1's pin selects mask 1
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.05, "ry": 0.05}));
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.52, 0.5], "rx": 0.05, "ry": 0.05}));
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    h.settle(SETTLE);
    let (p1, p2) = (at(&h, "maskPin:1:0"), at(&h, "maskPin:2:0"));
    assert!(p1.distance(p2) < 10.0, "both pins are within reach: {}", p1.distance(p2));
    assert_eq!(h.app.session.active_mask, Some(2));
    click(&mut h, p1 - egui::vec2(1.0, 0.0));
    h.settle(SETTLE);
    assert_eq!(h.app.session.active_mask, Some(1), "the nearer pin was taken");
}
