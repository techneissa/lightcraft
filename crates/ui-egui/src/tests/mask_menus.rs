//! The mask and submask right-click menus (issue #61): Add to Mask, Subtract from Mask and Intersect Mask with, Delete
//! Empty Masks, Delete All Masks (asks first), the submask menu's Mode, and several submasks selected together.

use std::time::Duration;

use lightcraft_develop::{Mask, MaskOp, MaskShape};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn masking() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
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
    h.step();
}

fn right_click(h: &mut Headless, id: &str, fx: f64) {
    let r = h.request("ui.clickWidget", json!({"id": id, "button": "right", "fx": fx}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

/// A click on a submask row with ⌘ or ⇧ held.
fn modified_click(h: &mut Headless, part: usize, cmd: bool, shift: bool) {
    let id = format!("component:{part}");
    let r = h.app.widgets.iter().find(|(w, _)| *w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"));
    let at = r.min + (r.size() * egui::vec2(0.2, 0.5));
    let events = json!([{"kind": "down", "x": at.x, "y": at.y}, {"kind": "up", "x": at.x, "y": at.y}]);
    let r = h.request("ui.pointer", json!({"events": events, "screen": true, "cmd": cmd, "shift": shift}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
}

fn masks(h: &Headless) -> Vec<Mask> {
    let id = h.app.session.active().expect("active photo");
    h.app.session.develop_of(id).expect("develop").masks.clone()
}

fn ops(h: &Headless) -> Vec<MaskOp> {
    masks(h)[0].components.iter().map(|c| c.op).collect()
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// Every mode of the mask menu adds the kind it is given with that mode, selects it, in one undo step.
#[test]
fn the_mask_menu_adds_a_submask_in_each_mode() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    h.settle(SETTLE);
    let id = masks(&h)[0].id;
    for (menu, kind, op, shape) in [
        ("add", "linear", MaskOp::Add, "linear"),
        ("subtract", "luminanceRange", MaskOp::Subtract, "luminance"),
        ("intersect", "brush", MaskOp::Intersect, "brush"),
    ] {
        let before = masks(&h)[0].components.len();
        right_click(&mut h, &format!("mask:{id}"), 0.3);
        click(&mut h, &format!("maskMenu:{menu}"));
        click(&mut h, &format!("maskMenu:{menu}:{kind}"));
        h.settle(SETTLE);
        let m = &masks(&h)[0];
        assert_eq!(m.components.len(), before + 1, "{menu} {kind}");
        let last = m.components.last().unwrap();
        assert_eq!(last.op, op, "{menu} {kind}");
        let ok = matches!(
            (&last.shape, shape),
            (MaskShape::Linear { .. }, "linear") | (MaskShape::LuminanceRange { .. }, "luminance") | (MaskShape::Brush { .. }, "brush")
        );
        assert!(ok, "{menu} {kind}: {:?}", last.shape);
        assert_eq!(h.app.session.active_part(), Some(before), "the new submask is selected");
        // one undo step takes it away again
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(masks(&h)[0].components.len(), before);
        exec(&mut h, "edit.redo", json!({}));
        h.settle(SETTLE);
    }
    assert_eq!(h.app.ui.tool, "brush", "the Brush starts when a brush submask is added");
}

#[test]
fn delete_empty_masks_and_delete_all_masks_from_the_menu() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "brush"}));
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    h.settle(SETTLE);
    let (empty, kept) = (masks(&h)[0].id, masks(&h)[1].id);
    right_click(&mut h, &format!("mask:{kept}"), 0.3);
    click(&mut h, "maskMenu:deleteEmpty");
    assert_eq!(masks(&h).iter().map(|m| m.id).collect::<Vec<_>>(), [kept], "{empty} went");
    // Delete All asks first
    right_click(&mut h, &format!("mask:{kept}"), 0.3);
    click(&mut h, "maskMenu:deleteAll");
    assert_eq!(masks(&h).len(), 1, "nothing is deleted before it is confirmed");
    assert!(has(&h, "button:maskDeleteAllConfirm"));
    click(&mut h, "button:maskDeleteAllCancel");
    assert_eq!(masks(&h).len(), 1);
    right_click(&mut h, &format!("mask:{kept}"), 0.3);
    click(&mut h, "maskMenu:deleteAll");
    click(&mut h, "button:maskDeleteAllConfirm");
    assert!(masks(&h).is_empty());
}

/// ⌘-click and ⇧-click select several submasks; the menu duplicates them together, sets their Mode together, and
/// Esc goes back to one.
#[test]
fn several_submasks_are_selected_and_changed_together() {
    let mut h = masking();
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    for op in ["subtract", "add", "add"] {
        exec(&mut h, "mask.addComponent", json!({"kind": "linear", "op": op}));
    }
    h.settle(SETTLE);
    click(&mut h, "component:0");
    modified_click(&mut h, 2, true, false);
    modified_click(&mut h, 3, true, false);
    assert_eq!(super::super::panels::mask_menus::selected_parts(&h.app, masks(&h)[0].id, 4), [0, 2, 3]);
    // Mode applies to all of them (the base stays Add)
    right_click(&mut h, "component:2", 0.2);
    click(&mut h, "partMenu:mode");
    click(&mut h, "partMenu:mode:intersect");
    assert_eq!(ops(&h), [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect, MaskOp::Intersect]);
    // Duplicate N Submasks: after the last, in order, and they become the selection
    right_click(&mut h, "component:2", 0.2);
    click(&mut h, "partMenu:duplicate");
    assert_eq!(masks(&h)[0].components.len(), 7);
    assert_eq!(ops(&h), [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect, MaskOp::Intersect, MaskOp::Add, MaskOp::Intersect, MaskOp::Intersect]);
    assert_eq!(super::super::panels::mask_menus::selected_parts(&h.app, masks(&h)[0].id, 7), [4, 5, 6]);
    // a ⇧-click selects the run from the selected one; Esc goes back to the last clicked
    modified_click(&mut h, 1, false, true);
    assert_eq!(super::super::panels::mask_menus::selected_parts(&h.app, masks(&h)[0].id, 7), [1, 2, 3, 4, 5, 6]);
    exec(&mut h, "view.back", json!({}));
    h.step();
    assert_eq!(super::super::panels::mask_menus::selected_parts(&h.app, masks(&h)[0].id, 7), [1], "Esc goes back to the last clicked");
    // Delete takes every selected submask
    modified_click(&mut h, 4, true, false);
    modified_click(&mut h, 5, true, false);
    assert_eq!(super::super::panels::mask_menus::selected_parts(&h.app, masks(&h)[0].id, 7), [1, 4, 5]);
    let r = h.request("ui.key", json!({"key": "Delete"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(masks(&h)[0].components.len(), 4);
}
