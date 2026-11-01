//! The line under a Subject, Sky or Background part that the classical heuristic draws (issue #37): it says the mask is
//! rough, and without SAM 3 a click on it offers the download. A build without SAM 3 at all never shows it.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn masking() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    // a model folder the app could install SAM 3 into, with nothing in it
    app.session.segmenter.dir = Some(std::env::temp_dir().join(format!("lc-rough-mask-{}", std::process::id())));
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    for (method, params) in [("ui.set", json!({"view": "detail"})), ("engine.execute", json!({"command": "panel.masking"}))] {
        let r = h.request(method, params, T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h
}

fn rough_line(h: &Headless) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == "button:maskRough")
}

#[test]
fn a_rough_sky_says_so_and_a_click_offers_sam_3() {
    let mut h = masking();
    let r = h.request("engine.execute", json!({"command": "mask.add", "params": {"kind": "sky"}}), T);
    assert_eq!(r["ok"], true, "no SAM 3: the mask is made at once the current way: {r}");
    h.settle(T);
    if !lightcraft_engine::segment::Segmenter::AVAILABLE {
        assert!(!rough_line(&h), "a build without SAM 3 has nothing to offer");
        return;
    }
    assert!(rough_line(&h), "the rough Sky part says so");
    let r = h.request("ui.clickWidget", json!({"id": "button:maskRough"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SamModel { .. })), "the click offers the download");
}

#[test]
fn a_part_with_a_stored_selection_is_not_rough() {
    let mut h = masking();
    let seg = lightcraft_develop::SegMask::from_logits(4, &[5.0; 16]);
    let r = h.request("engine.execute", json!({"command": "mask.add", "params": {"kind": "sky", "seg": seg}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(!rough_line(&h), "a selection from SAM 3 is not rough");
}
