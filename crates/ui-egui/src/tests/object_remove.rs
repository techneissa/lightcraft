//! Fork feature (`fork/object-remove.md`): Object mode in the Remove panel, the outline and row of a spot in an
//! object's shape, no source pin for LaMa, and a mask's "Remove This Area" (headless).

use std::time::Duration;

use lightcraft_develop::{SegMask, SpotFill};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn detail(panel: &str) -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    exec(&mut h, panel, json!({}));
    h.step();
    h.step();
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
    h.step();
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn spots(h: &Headless) -> Vec<lightcraft_develop::Spot> {
    h.app.session.develop_of(h.app.session.active().unwrap()).unwrap_or_default().spots.clone()
}

fn box_seg() -> serde_json::Value {
    let l: Vec<f32> = (0..64 * 64).map(|i| if (20..40).contains(&(i % 64)) && (20..44).contains(&(i / 64)) { 8.0 } else { -8.0 }).collect();
    serde_json::to_value(SegMask::from_logits(64, &l)).unwrap()
}

// Given Remove or Content-Aware, the panel offers Brush | Object; Object without SAM 3 offers its download, and a
// click on the photo asks for it instead of making a spot
#[test]
fn object_mode_without_sam_offers_the_download() {
    let mut h = detail("panel.remove");
    // (a model folder with no model in it, as on a Mac that never downloaded SAM 3)
    h.app.session.segmenter.dir = Some(std::env::temp_dir().join(format!("lc-ui-object-remove-none-{}", std::process::id())));
    assert!(has(&h, "removeSelect-brush") && has(&h, "removeSelect-object"));
    click(&mut h, "button:removeMode-heal");
    assert!(!has(&h, "removeSelect-object"), "Heal copies a source: no Object mode");
    click(&mut h, "button:removeMode-remove");
    click(&mut h, "removeSelect-object");
    assert!(h.app.ui.remove_object);
    assert!(has(&h, "button:removeObjectDownload"));
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.5, "y": 0.5}, {"kind": "up", "x": 0.5, "y": 0.5}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(spots(&h).is_empty(), "no spot without the model");
    if lightcraft_engine::segment::Segmenter::AVAILABLE {
        assert!(matches!(h.app.ui.dialog, Some(crate::state::Dialog::SamModel { .. })), "the download is offered: {:?}", h.app.ui.toast);
    } else {
        assert!(h.app.ui.toast.is_some(), "a build without AI masks says so");
    }
    // a loose stroke still removes, as a brush-shaped spot
    h.app.ui.dialog = None;
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "drag", "x": 0.35, "y": 0.6}, {"kind": "drag", "x": 0.4, "y": 0.6}, {"kind": "up", "x": 0.4, "y": 0.6}]}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let all = spots(&h);
    assert_eq!(all.len(), 1);
    assert!(all[0].area.is_none() && all[0].points.len() > 1);
    // back to Brush
    click(&mut h, "removeSelect-brush");
    assert!(!h.app.ui.remove_object && !has(&h, "button:removeObjectDownload"));
}

// Given a spot in an object's shape, its row names the shape and it has a pin but no source; Object mode's fill is its own
#[test]
fn a_shaped_spot_has_a_named_row_and_no_source_pin() {
    let mut h = detail("panel.remove");
    exec(&mut h, "view.toolOverlay", json!({"mode": "always"}));
    exec(&mut h, "spot.object", json!({"x": 0.45, "y": 0.5, "seg": box_seg()}));
    exec(&mut h, "spot.add", json!({"mode": "remove", "points": [[0.8, 0.8]]}));
    exec(&mut h, "spot.add", json!({"mode": "remove", "points": [[0.2, 0.2]], "fill": "lama"}));
    h.step();
    h.step();
    assert_eq!(crate::panels::right::spot_name_for_tests(&spots(&h), 0).as_deref(), Some("Remove 1 · Object"));
    assert_eq!(crate::panels::right::spot_name_for_tests(&spots(&h), 1).as_deref(), Some("Remove 2"));
    for (i, source) in [(0, false), (1, true), (2, false)] {
        exec(&mut h, "spot.select", json!({"index": i}));
        h.step();
        h.step();
        assert!(has(&h, &format!("spotPin:{i}")), "spot {i} has its pin");
        assert_eq!(has(&h, &format!("spotSource:{i}")), source, "spot {i}: a source pin only for Classic Remove");
        assert_eq!(has(&h, "button:spotRefresh"), source, "spot {i}: Refresh Source only with a source");
    }
    // Object mode has its own fill choice, LaMa unless Classic is picked; the brush's stays Classic
    click(&mut h, "removeSelect-object");
    assert_eq!(h.app.ui.remove_object_fill, SpotFill::Lama);
    assert_eq!(h.app.ui.remove_fill, SpotFill::Classic);
}

// Given a mask, its menu's Remove This Area makes one spot in its shape and opens the Remove panel
#[test]
fn remove_this_area_from_the_mask_menu() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.1, "ry": 0.1}));
    h.step();
    h.step();
    let id = h.app.session.develop_of(h.app.session.active().unwrap()).unwrap().masks[0].id;
    let r = h.request("ui.clickWidget", json!({"id": format!("mask:{id}"), "button": "right", "fx": 0.3}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    click(&mut h, "maskMenu:removeArea");
    let all = spots(&h);
    assert_eq!(all.len(), 1);
    assert!(all[0].area.as_ref().is_some_and(|a| a.kind == "mask"));
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Remove);
}

#[test]
fn the_size_slider_is_expand_for_a_shaped_spot() {
    assert_eq!(crate::panels::right::remove_object::size_label_for_tests(true), ("Expand", 50.0));
    assert_eq!(crate::panels::right::remove_object::size_label_for_tests(false), ("Size", 250.0));
}
