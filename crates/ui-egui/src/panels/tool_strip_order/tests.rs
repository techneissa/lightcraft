use std::time::Duration;

use serde_json::{Value, json};

use super::Tool::{Activity, Crop, Edit, Info, Keywords, Masking, Portrait, Presets, RedEye, Remove, Versions};
use super::*;
use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn order(o: &ToolStripOrder) -> Vec<Tool> {
    o.tools().to_vec()
}

const USUAL: [Tool; 11] = [Presets, Edit, Crop, Remove, Masking, Portrait, RedEye, Versions, Activity, Keywords, Info];

#[test]
fn a_new_strip_has_the_usual_order() {
    assert_eq!(order(&ToolStripOrder::default()), USUAL);
    assert!(ToolStripOrder::default().is_default());
}

#[test]
fn moving_steps_one_place_and_stops_at_the_ends() {
    let mut o = ToolStripOrder::default();
    assert!(o.move_by(Crop, true));
    assert_eq!(order(&o)[1..3], [Crop, Edit]);
    assert!(o.move_by(Crop, false) && o.is_default());
    assert!(!o.can_move(Presets, true) && !o.move_by(Presets, true));
    assert!(!o.can_move(Info, false) && !o.move_by(Info, false));
    assert!(o.move_before(Info, Some(Presets)));
    assert_eq!(o.tools().first(), Some(&Info));
    assert!(o.move_before(Info, None));
    assert_eq!(o.tools().last(), Some(&Info));
    assert!(!o.move_before(Edit, Some(Edit)), "above itself is no move");
}

#[test]
fn the_order_survives_saving_and_an_old_state_loads_in_the_usual_order() {
    let mut state = crate::state::UiState::default();
    state.tool_strip.move_before(Info, Some(Presets));
    let saved = serde_json::to_value(&state).unwrap();
    assert_eq!(saved["toolStrip"][0], "info");
    let back: crate::state::UiState = serde_json::from_value(saved.clone()).unwrap();
    assert_eq!(back.tool_strip, state.tool_strip);
    // saved before the strip could be arranged
    let mut old = saved;
    old.as_object_mut().unwrap().remove("toolStrip");
    let loaded: crate::state::UiState = serde_json::from_value(old).unwrap();
    assert!(loaded.tool_strip.is_default());
}

#[test]
fn a_damaged_saved_order_is_made_whole() {
    let read = |v: Value| serde_json::from_value::<ToolStripOrder>(v).unwrap();
    let o = read(json!(["info", "nope", "info", 3, null, "crop"]));
    assert_eq!(order(&o), [Info, Crop, Presets, Edit, Remove, Masking, Portrait, RedEye, Versions, Activity, Keywords]);
    for bad in [json!(null), json!("edit"), json!({"edit": 1}), json!([])] {
        assert!(read(bad).is_default());
    }
}

/// Fork: an order saved before the Portrait icon existed gets it after Masking, so it does not take a bottom place.
#[test]
fn an_order_saved_before_portrait_gets_it_after_masking() {
    let o = serde_json::from_value::<ToolStripOrder>(json!([
        "presets", "edit", "crop", "remove", "masking", "redeye", "versions", "activity", "keywords", "info"
    ]))
    .unwrap();
    assert!(o.is_default(), "{:?}", order(&o));
    let o = serde_json::from_value::<ToolStripOrder>(json!(["info", "masking", "crop"])).unwrap();
    assert_eq!(order(&o)[..4], [Info, Masking, Portrait, Crop]);
}

#[test]
fn requests_are_read_strictly() {
    let o = ToolStripOrder::with_request(&json!(["info", "crop"])).unwrap();
    assert_eq!(order(&o)[..3], [Info, Crop, Presets]);
    for (v, says) in [
        (json!("info"), "must be a list"),
        (json!(["info", "info"]), "listed twice"),
        (json!(["inf"]), "unknown tool `inf`"),
        (json!([4]), "names no tool"),
    ] {
        let e = ToolStripOrder::with_request(&v).unwrap_err();
        assert!(e.contains(says), "{e}");
    }
}

fn strip_app() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1000.0], 1.0);
    let r = h.request("ui.set", json!({"view": "photoGrid"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    h.step();
    h
}

fn icon(h: &Headless, tool: Tool) -> egui::Rect {
    let id = format!("icon:{}", tool.id());
    h.app.widgets.iter().find(|(w, _)| *w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// Press at `from`, move to `to` and stay there (the button still down).
fn press_and_move(h: &mut Headless, from: egui::Pos2, to: egui::Pos2) {
    h.app.synthetic.push(egui::Event::PointerMoved(from));
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    for i in 1..=8 {
        h.app.synthetic.push(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0)));
        h.step();
    }
    h.step();
}

fn release(h: &mut Headless, at: egui::Pos2) {
    h.app.synthetic.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    for _ in 0..3 {
        h.step();
    }
}

/// Given the strip, when an icon is dragged above another and released, then it sits there, a line
/// shows where it would go meanwhile, the drag opens no panel, and the order is part of the saved
/// UI state.
#[test]
fn dragging_an_icon_moves_it_and_shows_where_it_goes() {
    let mut h = strip_app();
    let usual: Vec<Tool> = USUAL.to_vec();
    assert_eq!(order(&h.app.ui.tool_strip), usual);
    let right_before = h.app.ui.right;
    let (from, to) = (icon(&h, Keywords).center(), icon(&h, Edit).center() - egui::vec2(0.0, 12.0));
    press_and_move(&mut h, from, to);
    assert_eq!(h.app.ui.dragging_tool, Some(Keywords));
    let line = h.app.widgets.iter().find(|(w, _)| w == "toolStripDrop").map(|(_, r)| *r).expect("the drop line is drawn");
    let edit = icon(&h, Edit);
    assert!(line.center().y < edit.top() && line.center().y > edit.top() - 8.0, "the line is in the gap above Edit: {line:?} / {edit:?}");
    release(&mut h, to);
    assert_eq!(h.app.ui.dragging_tool, None);
    assert!(!has(&h, "toolStripDrop"));
    assert_eq!(order(&h.app.ui.tool_strip), [Presets, Keywords, Edit, Crop, Remove, Masking, Portrait, RedEye, Versions, Activity, Info]);
    assert!(icon(&h, Keywords).top() < icon(&h, Edit).top(), "drawn in the new order");
    assert_eq!(h.app.ui.right, right_before, "a drag opens nothing");
    // last: dragged below everything
    let (from, to) = (icon(&h, Presets).center(), icon(&h, Info).center() + egui::vec2(0.0, 8.0));
    press_and_move(&mut h, from, to);
    release(&mut h, to);
    assert_eq!(order(&h.app.ui.tool_strip).last(), Some(&Presets));
    // saved
    let back = serde_json::from_value::<crate::UiState>(serde_json::to_value(&h.app.ui).unwrap()).unwrap();
    assert_eq!(back.tool_strip, h.app.ui.tool_strip);
}

/// A click still opens the icon's panel and leaves the order; a drag given up (Esc, or released
/// away from the strip) moves nothing.
#[test]
fn a_click_opens_and_a_given_up_drag_moves_nothing() {
    let mut h = strip_app();
    let r = h.request("ui.clickWidget", json!({"id": "icon:crop"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.ui.right, RightPanel::Crop);
    assert!(h.app.ui.tool_strip.is_default() && h.app.ui.dragging_tool.is_none());
    let (from, to) = (icon(&h, Keywords).center(), icon(&h, Edit).center());
    press_and_move(&mut h, from, to);
    assert_eq!(h.app.ui.dragging_tool, Some(Keywords));
    h.app.synthetic.push(egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
    release(&mut h, to);
    assert!(h.app.ui.tool_strip.is_default() && h.app.ui.dragging_tool.is_none(), "Esc");
    // released over the photos
    let away = egui::pos2(500.0, to.y);
    press_and_move(&mut h, from, away);
    release(&mut h, away);
    assert!(h.app.ui.tool_strip.is_default() && h.app.ui.dragging_tool.is_none(), "released outside");
}

fn exec(h: &mut Headless, id: &str, params: Value) -> Value {
    let r = h.request("engine.execute", json!({"command": id, "params": params}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
    r["result"].clone()
}

#[test]
fn the_commands_move_read_and_reset() {
    let mut h = strip_app();
    let r = exec(&mut h, "view.toolStripMove", json!({"tool": "info", "direction": "up"}));
    assert_eq!(r["moved"], true);
    assert_eq!(r["order"][9], "info");
    let r = exec(&mut h, "view.toolStripMove", json!({"tool": "info", "before": "presets"}));
    assert_eq!(r["order"][0], "info");
    assert!(icon(&h, Info).top() < icon(&h, Presets).top(), "the strip is redrawn");
    let r = exec(&mut h, "view.toolStrip", json!({"order": ["crop"]}));
    assert_eq!(r["order"][0], "crop");
    assert_eq!(exec(&mut h, "view.toolStrip", json!({}))["order"][0], "crop", "no params reads it");
    assert_eq!(exec(&mut h, "view.toolStripReset", json!({}))["default"], true);
    let e = h.request("engine.execute", json!({"command": "view.toolStripMove", "params": {"tool": "nope", "direction": "up"}}), T);
    assert_eq!(e["ok"], false, "{e}");
}

#[test]
fn the_icon_menu_moves_and_resets() {
    let mut h = strip_app();
    let click = |h: &mut Headless, id: &str, button: &str| {
        let r = h.request("ui.clickWidget", json!({"id": id, "button": button}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        h.step();
        h.step();
    };
    click(&mut h, "icon:crop", "right");
    click(&mut h, "toolStripMenu:moveUp", "left");
    assert_eq!(order(&h.app.ui.tool_strip)[1..3], [Crop, Edit]);
    click(&mut h, "icon:edit", "right");
    click(&mut h, "toolStripMenu:moveDown", "left");
    assert_eq!(order(&h.app.ui.tool_strip)[2..4], [Remove, Edit]);
    click(&mut h, "icon:presets", "right");
    click(&mut h, "toolStripMenu:reset", "left");
    assert!(h.app.ui.tool_strip.is_default());
}
