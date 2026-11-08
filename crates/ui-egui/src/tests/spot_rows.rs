//! Fork feature (`fork/spot-rows.md`): the Remove panel's row for each spot (select, hide or show, the circle setting,
//! delete), and the Tool Overlay's Never covering nothing (headless; issues #69 and #71).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::ToolOverlay;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn remove_panel(spots: usize) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    assert_eq!(h.request("engine.execute", json!({"command": "panel.remove"}), T)["ok"], true);
    for k in 0..spots {
        let (x, y) = (0.1 + 0.8 * (k % 9) as f64 / 9.0, 0.2 + 0.6 * (k / 9) as f64 / 5.0);
        exec(&mut h, "spot.add", json!({"mode": if k % 2 == 0 { "heal" } else { "clone" }, "points": [[x, y]], "size": 0.02, "source": [0.05, 0.0]}));
    }
    h.step();
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn spots(h: &Headless) -> Vec<lightcraft_develop::Spot> {
    h.app.session.develop_of(h.app.session.active().unwrap()).unwrap_or_default().spots.clone()
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn pins(h: &Headless) -> Vec<String> {
    let mut v: Vec<String> = h.app.widgets.iter().filter(|(id, _)| id.starts_with("spotPin:")).map(|(id, _)| id.clone()).collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn each_spot_has_a_row_and_a_click_selects_it() {
    let mut h = remove_panel(3);
    for i in 0..3 {
        assert!(has(&h, &format!("spotRow:{i}")), "row {i}");
        assert!(has(&h, &format!("spotVisible:{i}")) && has(&h, &format!("spotDelete:{i}")) && has(&h, &format!("spotCircle:{i}")));
    }
    assert!(!has(&h, "spotRow:3"));
    assert_eq!(h.app.session.active_spot, Some(2));
    click(&mut h, "spotRow:0");
    assert_eq!(h.app.session.active_spot, Some(0), "the row selects its spot");
    // …and the row follows a spot picked on the photo (the same state the pin sets)
    exec(&mut h, "spot.select", json!({"index": 1}));
    h.step();
    assert_eq!(h.app.session.active_spot, Some(1));
    assert_eq!(spots(&h).len(), 3, "selecting changes no spot");
}

#[test]
fn the_eye_hides_and_shows_one_spot_and_the_trash_deletes_it() {
    let mut h = remove_panel(3);
    click(&mut h, "spotVisible:1");
    assert_eq!(spots(&h).iter().map(|s| s.hidden).collect::<Vec<_>>(), vec![false, true, false]);
    assert_eq!(h.app.session.active_spot, Some(2), "the eye does not select the row");
    click(&mut h, "spotVisible:1");
    assert!(spots(&h).iter().all(|s| !s.hidden));
    exec(&mut h, "edit.undo", json!({}));
    assert!(spots(&h)[1].hidden, "one undo step per click");
    click(&mut h, "spotDelete:0");
    assert_eq!(spots(&h).len(), 2);
    assert!(spots(&h)[0].hidden, "row 0 is now the spot that was row 1");
    assert!(!has(&h, "spotRow:2"));
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(spots(&h).len(), 3);
}

#[test]
fn a_rows_circle_setting_beats_the_global_tool_overlay_for_that_spot_only() {
    let mut h = remove_panel(3);
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    h.step();
    assert_eq!(pins(&h), ["spotPin:0", "spotPin:1", "spotPin:2"]);
    click(&mut h, "spotCircle:1");
    click(&mut h, "spotCircle:1:never");
    assert_eq!(spots(&h)[1].circle, Some(lightcraft_develop::SpotCircle::Never));
    assert_eq!(pins(&h), ["spotPin:0", "spotPin:2"], "its circle is gone, the others follow Always");
    assert!(!spots(&h)[1].hidden, "the circle setting leaves the effect alone");
    // a spot set to Never can't be grabbed on the photo; its row selects it
    click(&mut h, "spotRow:1");
    assert_eq!(h.app.session.active_spot, Some(1));
    // Use Global follows the panel again
    click(&mut h, "spotCircle:1");
    click(&mut h, "spotCircle:1:global");
    assert_eq!(spots(&h)[1].circle, None);
    assert_eq!(pins(&h), ["spotPin:0", "spotPin:1", "spotPin:2"]);
    // …and a spot set to Always shows at the global Never
    exec(&mut h, "spot.circle", json!({"index": 2, "mode": "always"}));
    exec(&mut h, "view.toolOverlay", json!({"mode": "never"}));
    h.step();
    assert_eq!(pins(&h), ["spotPin:2"]);
}

#[test]
fn many_spots_scroll_inside_their_own_height() {
    let few = remove_panel(3);
    let mut many = remove_panel(40);
    // the list scrolled to the selected (last) spot: select the first one to measure from the top
    exec(&mut many, "spot.select", json!({"index": 0}));
    many.step();
    many.step();
    let rect = |h: &Headless, id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"));
    let (a, b) = (rect(&many, "spotRow:0"), rect(&many, "spotRow:39"));
    assert!(b.top() - a.top() > 39.0 * 28.0, "all the rows are laid out: {a:?} {b:?}");
    // the part of the panel under the list sits one list height under its first row, not 40 rows
    let below = |h: &Headless| rect(h, "flyout:reflections").top() - rect(h, "spotRow:0").top();
    assert!(below(&many) < 8.0 * 28.0 + 60.0, "the list has its own limit: {}", below(&many));
    assert!(below(&many) > below(&few), "more rows fill more of the limit");
}

#[test]
fn never_covers_nothing_but_a_click_or_a_drag_still_adds_a_spot() {
    let mut h = remove_panel(1);
    let on = |h: &mut Headless, x: f64, y: f64| {
        let r = h.request("ui.pointer", json!({"events": [{"kind": "move", "x": x, "y": y}]}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
    };
    // Auto: the brush's rings follow the pointer and the pointer itself is hidden
    on(&mut h, 0.4, 0.9);
    assert!(has(&h, "removeBrushRing"));
    assert_eq!(h.last_cursor, egui::CursorIcon::None);
    // Never: no rings, a small pointer instead
    exec(&mut h, "view.toolOverlay", json!({"mode": "never"}));
    assert_eq!(h.app.ui.tool_overlay, ToolOverlay::Never);
    on(&mut h, 0.45, 0.9);
    assert!(!has(&h, "removeBrushRing"), "no brush ring at Never");
    assert_eq!(h.last_cursor, egui::CursorIcon::Crosshair);
    // painting: no white fill at Never, the stroke still becomes a spot on release
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.4, "y": 0.8}, {"kind": "drag", "x": 0.45, "y": 0.8}, {"kind": "drag", "x": 0.5, "y": 0.8}]}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(!has(&h, "removePaintFill"), "no fill while painting at Never");
    let r = h.request("ui.pointer", json!({"events": [{"kind": "up", "x": 0.5, "y": 0.8}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(spots(&h).len(), 2, "the drag added a spot");
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.6, "y": 0.85}, {"kind": "up", "x": 0.6, "y": 0.85}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(spots(&h).len(), 3, "a click added one too");
    // Auto still fills while painting
    exec(&mut h, "view.toolOverlay", json!({"mode": "auto"}));
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.4, "y": 0.7}, {"kind": "drag", "x": 0.45, "y": 0.7}, {"kind": "drag", "x": 0.5, "y": 0.7}]}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(has(&h, "removePaintFill"), "Auto: the fill is as it was");
}

/// Moving to another photo in the Survey forgets the selected spot: its index would name that photo's spot, and Delete
/// would remove it.
#[test]
fn a_survey_step_forgets_the_selected_spot_and_mask() {
    let mut h = remove_panel(3);
    let vis = h.app.session.visible_cloned();
    let ids: Vec<u64> = vis.iter().take(3).map(|p| p.0).collect();
    exec(&mut h, "library.select", json!({"ids": ids, "active": ids[0]}));
    assert_eq!(h.app.session.active_spot, None, "a new selection starts clean");
    h.app.session.active_spot = Some(2);
    h.app.session.active_mask = Some(1);
    h.app.session.active_component = Some((1, 0));
    crate::panels::compare::survey_step(&mut h.app, 1).unwrap();
    assert_eq!(h.app.session.selection.active, Some(vis[1]));
    assert_eq!((h.app.session.active_spot, h.app.session.active_mask, h.app.session.active_component), (None, None, None));
}
