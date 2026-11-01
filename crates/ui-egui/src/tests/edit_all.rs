//! The "Edit All N Selected" switch on the Edit panel: Auto Sync on by default in the app, shown only for
//! a selection of two or more, and a click on it turns `develop.autoSync` on or off.

use std::time::Duration;

use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::headless::Headless;
use crate::panels::edit_all::SWITCH_ID;
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(60);

fn demo() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(T);
    h
}

fn drawn(h: &Headless) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == SWITCH_ID)
}

fn select(h: &mut Headless, count: usize) -> Vec<PhotoId> {
    let ids: Vec<PhotoId> = h.app.session.catalog.photos().take(count).map(|p| p.id).collect();
    h.app.run("library.select", json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "active": ids[0].0})).unwrap();
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Edit;
    h.step();
    h.step();
    ids
}

#[test]
fn editing_the_whole_selection_is_on_by_default_and_the_edit_panel_has_a_switch_for_it() {
    let mut h = demo();
    assert!(h.app.session.auto_sync, "the app starts with Auto Sync on");
    select(&mut h, 1);
    assert!(!drawn(&h), "one photo selected: nothing to switch");
    let ids = select(&mut h, 3);
    assert!(drawn(&h), "three photos selected: the switch is on the Edit panel");
    // a slider move reaches all three
    h.app.run("develop.set", json!({"control": "light.exposure", "value": 0.5})).unwrap();
    for id in &ids {
        assert_eq!(h.app.session.develop_of(*id).unwrap().light.exposure, 0.5);
    }
    // the switch is Auto Sync: a click turns it off, another on
    for want in [false, true] {
        assert_eq!(h.request("ui.clickWidget", json!({"id": SWITCH_ID}), T)["ok"], true);
        for _ in 0..4 {
            h.step();
        }
        assert_eq!(h.app.session.auto_sync, want);
    }
}
