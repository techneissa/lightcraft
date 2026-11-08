//! Issue #82: zoomed in, a new spot keeps the sharp window on screen until the window with the spot arrives, as the
//! whole-frame view keeps its picture. It used to drop out at once, so the photo went soft and sharp again after every
//! spot, as if the whole photo rendered again.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use serde_json::json;

use crate::headless::{Headless, RenderGate};
use crate::render::Slot;
use crate::state::Zoom;
use crate::tests_render_fixtures::RenderFixture;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// Whether `slot`'s texture is drawn in the last frame.
fn drawn(h: &Headless, slot: Slot) -> bool {
    let Some(tex) = h.app.renderer.textures.get(&slot) else { return false };
    h.view.shapes.iter().any(|c| matches!(&c.shape, egui::epaint::Shape::Mesh(m) if m.texture_id == tex.tex.id()))
}

#[test]
fn zoomed_in_a_new_spot_keeps_the_sharp_window_until_its_own_arrives() {
    let fixture = RenderFixture::new("remove-lag-window", 1);
    let app = LightcraftApp::new(fixture.session(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "remove", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.zoom = Zoom::Percent(300.0);
    h.app.ui.pan = (0.5, 0.5);
    assert!(h.settle(SETTLE), "the zoomed view settles");
    assert!(drawn(&h, Slot::Region), "zoomed in, the sharp window is drawn");
    let old = h.app.renderer.textures[&Slot::Region].key;
    // renders wait from now on
    let open = Rc::new(Cell::new(false));
    h.app.renderer.set_offload(Box::new(RenderGate::new(open.clone())));
    let add = json!({"command": "spot.add", "params": {"mode": "heal", "points": [[0.5, 0.5]], "size": 0.01, "source": [0.02, 0.0]}});
    let r = h.request("engine.execute", add, T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..5 {
        h.step();
    }
    assert!(h.app.renderer.is_pending(Slot::Region), "the window with the spot is rendering");
    assert_eq!(h.app.renderer.textures[&Slot::Region].key, old);
    assert!(drawn(&h, Slot::Region), "the sharp window stays on screen meanwhile");
    open.set(true);
    assert!(h.settle(SETTLE), "the new window renders");
    assert_ne!(h.app.renderer.textures[&Slot::Region].key, old, "and replaces the old one");
    assert!(drawn(&h, Slot::Region));
}
