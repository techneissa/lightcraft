//! The Masking panel's parts (issue #26): the Add · Subtract · Intersect control, the buttons for each kind of part,
//! picking a part to paint into it again, what Add and Erase mean in a Subtract part, and the steps of Esc.

use std::time::Duration;

use lightcraft_develop::{MaskOp, MaskShape};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn masking() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "brushSize": 0.15, "brushFeather": 0.0, "brushFlow": 100.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "panel.masking", json!({}));
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
}

/// A click on the label end of a part's row (the middle is under its buttons once they show).
fn click_row(h: &mut Headless, part: usize) {
    let r = h.request("ui.clickWidget", json!({"id": format!("component:{part}"), "fx": 0.2}), T);
    assert_eq!(r["ok"], true, "{r}");
}

fn brush_at(h: &mut Headless, x: f64, y: f64) {
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": x, "y": y}, {"kind": "up", "x": x, "y": y}]}), T);
    assert_eq!(r["ok"], true, "{r}");
}

fn mask(h: &Headless) -> lightcraft_develop::Mask {
    let id = h.app.session.active().expect("active photo");
    h.app.session.develop_of(id).expect("develop").masks[0].clone()
}

fn ops(h: &Headless) -> Vec<MaskOp> {
    mask(h).components.iter().map(|c| c.op).collect()
}

fn strokes(h: &Headless, part: usize) -> Vec<lightcraft_develop::BrushStroke> {
    match &mask(h).components[part].shape {
        MaskShape::Brush { strokes } => strokes.clone(),
        other => panic!("part {part} is {other:?}"),
    }
}

#[test]
fn the_mode_control_sets_the_mode_of_the_next_part_and_selects_it() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    h.settle(SETTLE);
    click(&mut h, "button:maskMode-subtract");
    assert_eq!(h.app.ui.mask_op, MaskOp::Subtract);
    click(&mut h, "icon:maskPart:linear");
    click(&mut h, "button:maskMode-intersect");
    click(&mut h, "icon:maskPart:sky");
    assert_eq!(ops(&h), [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect]);
    assert_eq!(h.app.session.active_part(), Some(2), "the part just added is the selected one");
    // the mode stays where it was put, for the next part too
    click(&mut h, "icon:maskPart:linear");
    assert_eq!(ops(&h).last(), Some(&MaskOp::Intersect));
    // a brush can intersect: the painted area is what stays
    let before = mask(&h).components.len();
    click(&mut h, "icon:maskPart:brush");
    assert_eq!(mask(&h).components.len(), before + 1);
    assert_eq!(ops(&h).last(), Some(&MaskOp::Intersect));
    h.settle(SETTLE);
}

#[test]
fn a_part_row_selects_the_part_and_its_buttons_change_it() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "linear"}));
    h.settle(SETTLE);
    click_row(&mut h, 1);
    assert_eq!(h.app.session.active_part(), Some(1));
    // the base part has no mode dropdown (it is always Add); every row has Invert and Delete
    assert!(h.app.widgets.iter().all(|(id, _)| id != "partMode:0"), "the first part is the base: no mode");
    assert!(h.app.widgets.iter().any(|(id, _)| id == "partInvert:0" && id != "partMode:0"));
    // the mode dropdown shows the mode and changes it in one click
    click(&mut h, "partMode:1");
    click(&mut h, "partMode:1:subtract");
    assert_eq!(ops(&h), [MaskOp::Add, MaskOp::Subtract]);
    assert_eq!(h.app.ui.mask_op, MaskOp::Subtract, "the mode control follows the selected part");
    click(&mut h, "partMode:1");
    click(&mut h, "partMode:1:intersect");
    assert_eq!(ops(&h), [MaskOp::Add, MaskOp::Intersect]);
    click(&mut h, "partInvert:1");
    assert!(mask(&h).components[1].invert);
    assert_eq!(h.app.session.active_part(), Some(1), "its buttons do not pick the row again, nor lose it");
    click_row(&mut h, 0);
    assert_eq!(h.app.session.active_part(), Some(0));
    click_row(&mut h, 1);
    click(&mut h, "partDelete:1");
    assert_eq!(mask(&h).components.len(), 1);
    h.settle(SETTLE);
}

#[test]
fn a_rows_mode_and_invert_change_only_that_part_in_one_undo_step() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "linear"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "radial"}));
    h.settle(SETTLE);
    let before = mask(&h);
    click_row(&mut h, 1);
    click(&mut h, "partMode:1");
    click(&mut h, "partMode:1:subtract");
    let after = mask(&h);
    assert_eq!((after.components[0].clone(), after.components[2].clone()), (before.components[0].clone(), before.components[2].clone()));
    assert_eq!(after.components[1].op, MaskOp::Subtract);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(mask(&h), before, "one undo step takes the mode back");
    click(&mut h, "partInvert:1");
    let after = mask(&h);
    assert_eq!((after.components[0].clone(), after.components[2].clone()), (before.components[0].clone(), before.components[2].clone()));
    assert!(after.components[1].invert);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(mask(&h), before, "one undo step takes the Invert back");
    h.settle(SETTLE);
}

/// A click on a part's row brings its tool back: the Brush for a brush part, the gradient's handles for a gradient
/// (blue, the ones a drag takes), the picker for a colour range; and selects the part.
#[test]
fn a_part_row_brings_its_tool_back() {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 1500.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "panel.masking", json!({}));
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    exec(&mut h, "tool.brush", json!({"op": "add"}));
    exec(&mut h, "mask.addComponent", json!({"op": "subtract", "kind": "linear"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "colorRange"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "sky"}));
    let id = mask(&h).id;
    exec(&mut h, "mask.select", json!({"id": id}));
    h.app.ui.tool.clear();
    h.settle(SETTLE);
    for (part, tool) in [(0, "radial"), (1, "brush"), (2, "linear"), (3, "colorRange"), (4, "")] {
        click_row(&mut h, part);
        assert_eq!((h.app.session.active_part(), h.app.ui.tool.as_str()), (Some(part), tool), "row {part}");
    }
    // a gradient's tool is the default one: a drag on its handle moves it
    click_row(&mut h, 0);
    h.settle(SETTLE);
    let before = mask(&h).components[0].shape.clone();
    let handle = h.app.widgets.iter().find(|(id, _)| id == "maskHandle:1:0:1").map(|(_, r)| r.center()).expect("the radial's handle");
    let r = h.request("ui.drag", json!({"x": handle.x, "y": handle.y, "toX": handle.x + 40.0, "toY": handle.y, "steps": 6}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_ne!(mask(&h).components[0].shape, before, "the handle moved the selected radial part");
    h.settle(SETTLE);
}

#[test]
fn picking_a_brush_part_resumes_painting_into_it_and_esc_steps_back() {
    let mut h = masking();
    exec(&mut h, "tool.brush", json!({"new": true}));
    brush_at(&mut h, 0.3, 0.3);
    exec(&mut h, "tool.brush", json!({"op": "add"}));
    brush_at(&mut h, 0.7, 0.7);
    assert_eq!((strokes(&h, 0).len(), strokes(&h, 1).len()), (1, 1));
    assert_eq!(h.app.session.active_part(), Some(1));

    // Esc: the brush first, the part still selected
    exec(&mut h, "view.back", json!({}));
    assert_eq!(h.app.ui.tool, "");
    assert_eq!(h.app.session.active_part(), Some(1));
    // the first brush part picked again: the brush is on, on Add, and strokes go into that part, not the last one
    h.app.ui.brush_erase = true;
    click_row(&mut h, 0);
    assert_eq!((h.app.ui.tool.as_str(), h.app.ui.brush_erase, h.app.session.active_part()), ("brush", false, Some(0)));
    brush_at(&mut h, 0.5, 0.5);
    assert_eq!((strokes(&h, 0).len(), strokes(&h, 1).len()), (2, 1));
    // Esc: the brush, then the part (the mask stays), then the photo goes back to the grid
    exec(&mut h, "view.back", json!({}));
    assert_eq!((h.app.ui.tool.as_str(), h.app.session.active_part()), ("", Some(0)));
    exec(&mut h, "view.back", json!({}));
    assert_eq!((h.app.session.active_part(), h.app.session.active_mask.is_some()), (None, true));
    exec(&mut h, "view.back", json!({}));
    assert_eq!(h.app.ui.view, crate::state::ViewMode::PhotoGrid);
    h.settle(SETTLE);
}

#[test]
fn add_and_erase_name_what_a_stroke_does_to_the_mask_in_a_subtract_part_too() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "sky"}));
    click(&mut h, "button:maskMode-subtract");
    click(&mut h, "icon:maskPart:brush");
    // a Subtract part starts on Erase: its strokes paint the part, which takes the mask away
    assert!(h.app.ui.brush_erase);
    brush_at(&mut h, 0.4, 0.5);
    assert!(!strokes(&h, 1)[0].erase);
    // Add: the stroke takes paint out of the part, giving the mask back
    h.app.ui.brush_erase = false;
    brush_at(&mut h, 0.5, 0.5);
    assert!(strokes(&h, 1)[1].erase);
    // ⌥ turns it around for one stroke
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.6, "y": 0.5}, {"kind": "up", "x": 0.6, "y": 0.5}], "alt": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!strokes(&h, 1)[2].erase);
    h.settle(SETTLE);
}

#[test]
fn tool_brush_takes_a_part_and_the_mode() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "brush"}));
    let r = h.request("engine.execute", json!({"command": "tool.brush", "params": {"component": 0}}), T);
    assert_eq!(r["ok"], false, "part 0 is a radial gradient: {r}");
    exec(&mut h, "tool.brush", json!({"component": 1, "erase": true}));
    assert_eq!((h.app.session.active_part(), h.app.ui.brush_erase), (Some(1), true));
    h.settle(SETTLE);
}

#[test]
fn delete_removes_the_selected_part_else_the_mask() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    exec(&mut h, "mask.addComponent", json!({"op": "add", "kind": "linear"}));
    let id = mask(&h).id;
    exec(&mut h, "mask.select", json!({"id": id, "component": 1}));
    let r = h.request("ui.key", json!({"key": "Delete"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(mask(&h).components.len(), 1, "the part, not the mask");
    let r = h.request("ui.key", json!({"key": "Delete"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let d = h.app.session.develop_of(h.app.session.active().expect("photo")).expect("develop");
    assert!(d.masks.is_empty(), "no part selected: the mask goes");
    h.settle(SETTLE);
}

#[test]
fn tool_brush_refuses_a_malformed_component_and_changes_nothing() {
    let mut h = masking();
    exec(&mut h, "tool.brush", json!({"new": true}));
    exec(&mut h, "tool.brush", json!({"op": "add"}));
    exec(&mut h, "view.back", json!({}));
    let part = h.app.session.active_part();
    assert_eq!((part, h.app.ui.tool.as_str()), (Some(1), ""));
    for bad in [json!(-1), json!("2"), json!(1.5), json!(9), json!(null).clone()] {
        let r = h.request("engine.execute", json!({"command": "tool.brush", "params": {"component": bad}}), T);
        if bad.is_null() {
            // null means no part given, like an omitted one
            assert_eq!(r["ok"], true, "{bad}: {r}");
            exec(&mut h, "view.back", json!({}));
            continue;
        }
        assert_eq!(r["ok"], false, "{bad}: {r}");
        assert_eq!((h.app.session.active_part(), h.app.ui.tool.as_str()), (part, ""), "{bad}: nothing changed");
    }
    h.settle(SETTLE);
}

/// A part with Invert on is turned around by the engine before it is combined, so a stroke in it does the opposite:
/// Add (to the mask) has to take paint out of the part, and Erase has to paint it.
#[test]
fn an_inverted_part_turns_add_and_erase_around() {
    let mut h = masking();
    exec(&mut h, "tool.brush", json!({"new": true}));
    exec(&mut h, "mask.component", json!({"component": 0, "action": "invert"}));
    h.app.ui.brush_erase = false;
    brush_at(&mut h, 0.3, 0.5);
    assert!(strokes(&h, 0)[0].erase, "Add in an inverted part takes paint out of it");
    h.app.ui.brush_erase = true;
    brush_at(&mut h, 0.6, 0.5);
    assert!(!strokes(&h, 0)[1].erase, "Erase paints it, which takes the mask away");
    // and in an inverted Subtract part they are the same as in a plain Add part again
    exec(&mut h, "tool.brush", json!({"op": "subtract"}));
    exec(&mut h, "mask.component", json!({"component": 1, "action": "invert"}));
    h.app.ui.brush_erase = false;
    brush_at(&mut h, 0.4, 0.4);
    assert!(!strokes(&h, 1)[0].erase);
    h.settle(SETTLE);
}

/// Official `mask_adjustments_hide_overlay_without_disabling_the_mask` expects the Exposure slider below the fold; the
/// reworked panel is shorter, so that test fails on its precondition. This keeps the behaviour: a slider change hides
/// the overlay, the mask stays enabled, with the slider scrolled into view only when it needs it.
#[test]
fn a_mask_slider_hides_the_overlay_and_keeps_the_mask_enabled() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.5, 0.2], "end": [0.5, 0.7]}));
    assert!(h.app.ui.mask_overlay);
    let rect = |h: &Headless, id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).expect(id);
    let panel = rect(&h, "panel:right_panel");
    if !panel.contains_rect(rect(&h, "slider:exposure")) {
        h.request("ui.move", json!({"x": panel.center().x, "y": panel.center().y}), T);
        h.request("ui.scroll", json!({"dy": -160.0}), T);
        h.settle(SETTLE);
    }
    assert!(panel.contains_rect(rect(&h, "slider:exposure")), "exposure is visible before clicking");
    let r = h.request("ui.clickWidget", json!({"id": "slider:exposure", "fx": 0.7}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!h.app.ui.mask_overlay, "the exposure slider hides the overlay after release");
    assert!(mask(&h).adjust.exposure > 0.0);
    assert!(mask(&h).visible, "hiding the overlay must not disable the mask");
    let id = h.app.session.active().expect("active photo");
    let d = (*h.app.session.develop_of(id).unwrap_or_default()).clone();
    let none = lightcraft_pipeline::Overlay::None;
    assert_eq!(crate::panels::detail::view_overlay(&h.app, &d, h.view.ctx.cumulative_frame_nr()), none);
    let r = h.request("ui.key", json!({"key": "o"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.mask_overlay, "O can restore the overlay");
}
