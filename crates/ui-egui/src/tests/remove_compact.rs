//! Fork feature (`fork/remove-panel-layout.md`): the Remove panel's tiles at the narrowest side bar (headless).

use std::time::Duration;

use egui::Rect;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn panel(width: f32) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1100.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail", "rightWidth": width}), T)["ok"], true);
    assert_eq!(h.request("engine.execute", json!({"command": "panel.remove"}), T)["ok"], true);
    h.settle(Duration::from_secs(10));
    h
}

fn rect(h: &Headless, id: &str) -> Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

/// The tiles of a row sit side by side at one height, none overlapping, and the row fills the bar.
fn assert_row(h: &Headless, ids: &[&str]) {
    let rects: Vec<Rect> = ids.iter().map(|id| rect(h, id)).collect();
    for pair in rects.windows(2) {
        assert!(pair[0].right() < pair[1].left(), "tiles overlap: {rects:?}");
        assert_eq!(pair[0].height(), pair[1].height(), "tiles differ in height: {rects:?}");
        assert_eq!(pair[0].width(), pair[1].width(), "tiles differ in width: {rects:?}");
    }
}

#[test]
fn tiles_fit_in_one_row_at_the_narrowest_and_a_wide_side_bar() {
    for width in [crate::state::RIGHT_WIDTH.min, 340.0] {
        let h = panel(width);
        assert_row(&h, &["button:removeMode-remove", "button:removeMode-fill", "button:removeMode-heal", "button:removeMode-clone"]);
        assert_row(&h, &["button:findDust", "button:findPeople", "button:findWires"]);
        let (first, last) = (rect(&h, "button:removeMode-remove"), rect(&h, "button:removeMode-clone"));
        assert!(last.right() - first.left() > width - 40.0, "the tools fill the bar at {width}: {first:?} {last:?}");
    }
}

#[test]
fn choosing_heal_hides_the_fill_picker_and_marks_the_tile() {
    let mut h = panel(crate::state::RIGHT_WIDTH.min);
    assert!(h.app.widgets.iter().any(|(id, _)| id == "dropdown:removeFill"), "Remove takes a fill");
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:removeMode-heal"}), T)["ok"], true);
    h.step();
    assert_eq!(h.app.ui.tool, "heal");
    assert!(h.app.widgets.iter().all(|(id, _)| id != "dropdown:removeFill"), "Heal copies a source: no fill picker");
}
