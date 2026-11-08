//! Issue #68: every submask row's Edge slider is its own widget, so a drag changes only its own submask.

use std::time::Duration;

use lightcraft_develop::{MaskShape, SegMask};
use lightcraft_geom::Point;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

/// The Edge of each submask of the first mask.
fn edges(h: &Headless) -> Vec<f64> {
    let id = h.app.session.active().expect("active photo");
    let d = h.app.session.develop_of(id).unwrap_or_default();
    d.masks[0]
        .components
        .iter()
        .map(|c| match &c.shape {
            MaskShape::Object { edge, .. } | MaskShape::Prompt { edge, .. } => *edge,
            other => panic!("not an AI submask: {other:?}"),
        })
        .collect()
}

fn two_ai_submasks() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    for (method, params) in [("ui.set", json!({"view": "detail"})), ("ui.set", json!({"right": "masking"}))] {
        let r = h.request(method, params, T);
        assert_eq!(r["ok"], true, "{r}");
    }
    // a selection each (found by the AI): the Edge slider is enabled for a submask that has one
    let seg = SegMask::from_logits(16, &vec![8.0; 16 * 16]);
    let object = MaskShape::Object { hint: vec![Point::new(0.6, 0.5)], exclude: vec![], seg: Some(seg.clone()), detail: vec![], edge: 0.0 };
    let prompt = MaskShape::Prompt { text: "circle".into(), seg: Some(seg), detail: vec![], edge: 0.0 };
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.1, "ry": 0.1}));
    exec(&mut h, "mask.update", json!({"component": 0, "shape": object}));
    exec(&mut h, "mask.addComponent", json!({"kind": "radial", "op": "add", "center": [0.5, 0.5], "rx": 0.1, "ry": 0.1}));
    exec(&mut h, "mask.update", json!({"component": 1, "shape": prompt}));
    h.settle(SETTLE);
    h
}

fn drag(h: &mut Headless, id: &str, dx: f64) {
    let r = h.request("ui.dragWidget", json!({"id": id, "dx": dx}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

/// Dragging the first submask's Edge slider changes only the first submask, and the second one's changes only the
/// second (before the fix both moved together and the upper one could not be grabbed).
#[test]
fn an_edge_slider_changes_only_its_own_submask() {
    let mut h = two_ai_submasks();
    assert_eq!(edges(&h), [0.0, 0.0]);
    // (the bare name is the first row's, as before there were rows)
    drag(&mut h, "slider:edge", 60.0);
    let after_bare = edges(&h);
    assert!(after_bare[0] > 0.0, "the bare slider:edge moves the first submask: {after_bare:?}");
    assert_eq!(after_bare[1], 0.0, "and only that one: {after_bare:?}");
    // (a drag sets the value from the pointer: 30 points right of the middle is about 29, not 58)
    drag(&mut h, "slider:edge:0", 30.0);
    let after_first = edges(&h);
    assert!(after_first[0] > 0.0 && after_first[0] < after_bare[0], "the first slider moves its submask: {after_first:?}");
    assert_eq!(after_first[1], 0.0, "and only that one: {after_first:?}");
    drag(&mut h, "slider:edge:1", -60.0);
    let after_second = edges(&h);
    assert_eq!(after_second[0], after_first[0], "the second slider leaves the first alone: {after_second:?}");
    assert!(after_second[1] < 0.0, "and moves its own: {after_second:?}");
}

/// The same for Refine, which two Color Range submasks of one mask both show.
#[test]
fn a_refine_slider_changes_only_its_own_submask() {
    let mut h = two_ai_submasks();
    let range = MaskShape::ColorRange { samples: vec![[0.5, 0.0, 0.0]], refine: 50.0 };
    exec(&mut h, "mask.update", json!({"component": 0, "shape": range}));
    exec(&mut h, "mask.update", json!({"component": 1, "shape": range}));
    h.settle(SETTLE);
    let refines = |h: &Headless| -> Vec<f64> {
        let id = h.app.session.active().expect("active photo");
        let d = h.app.session.develop_of(id).unwrap_or_default();
        d.masks[0]
            .components
            .iter()
            .map(|c| match &c.shape {
                MaskShape::ColorRange { refine, .. } => *refine,
                other => panic!("not a Color Range: {other:?}"),
            })
            .collect()
    };
    assert_eq!(refines(&h), [50.0, 50.0]);
    drag(&mut h, "slider:refine:0", 40.0);
    let after = refines(&h);
    assert!(after[0] > 50.0, "the first slider moves its submask: {after:?}");
    assert_eq!(after[1], 50.0, "and only that one: {after:?}");
}

/// An Edge row and a Refine row of one mask keep one value column width, as the rows did before each had a scope.
#[test]
fn the_rows_share_one_value_column_width() {
    let mut h = two_ai_submasks();
    let range = MaskShape::ColorRange { samples: vec![[0.5, 0.0, 0.0]], refine: 50.0 };
    exec(&mut h, "mask.update", json!({"component": 1, "shape": range}));
    h.settle(SETTLE);
    let width = |id: &str| h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| r.width()).unwrap_or_else(|| panic!("no widget {id}"));
    assert_eq!(width("sliderValue:edge:0"), width("sliderValue:refine:1"));
}
