//! A long mask name in the Masks list takes a smaller font, then ends with "…" (issue #37).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn rect(h: &Headless, id: &str) -> Option<egui::Rect> {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r)
}

#[test]
fn a_long_mask_name_fits_its_row() {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    for (method, params) in [
        ("ui.set", json!({"view": "detail"})),
        ("engine.execute", json!({"command": "panel.masking"})),
        ("engine.execute", json!({"command": "mask.add", "params": {"kind": "radial"}})),
        (
            "engine.execute",
            json!({"command": "mask.add", "params": {"kind": "linear", "name": "A long name for a mask that the row cannot hold in one line"}}),
        ),
    ] {
        let r = h.request(method, params, T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.settle(T);
    let (short, long) = (rect(&h, "maskName:1").expect("the first row"), rect(&h, "maskName:2").expect("the second row"));
    let row = rect(&h, "mask:2").expect("the row");
    assert!(long.right() <= row.right() - 30.0, "the long name stays inside its row: {long:?} in {row:?}");
    assert!(long.height() < short.height() - 0.4, "the long name is set smaller ({} against {})", long.height(), short.height());
    // the mask's own generated name
    assert_eq!(h.app.session.develop_of(h.app.session.active().unwrap()).unwrap().masks[0].name, "Radial Mask 1");
}
