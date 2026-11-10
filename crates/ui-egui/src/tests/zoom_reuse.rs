//! Zooming and panning a photo with masks and spots (#174): the zoomed window reuses what is already rendered,
//! so a zoom or pan builds no mask and draws no spot again.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::render::{RenderOffload, Slot};
use crate::tests_render_fixtures::RenderFixture;
use crate::{LightcraftApp, Services};
use lightcraft_engine::RenderJob;
use lightcraft_engine::media::RenderResult;

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// Runs each render on the test's thread when it is asked for, so the pipeline's per-thread counts of masks built and
/// spots drawn are this test's.
#[derive(Default)]
struct Inline(Vec<(Slot, RenderResult, f64)>);

impl RenderOffload for Inline {
    fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob> {
        let result = job.run();
        self.0.push((slot, result, 0.0));
        None
    }

    fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
        std::mem::take(&mut self.0)
    }
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

/// Masks built and spots drawn on this thread so far.
fn work() -> (usize, usize) {
    (lightcraft_pipeline::masks::mask_evaluations(), lightcraft_pipeline::spots_keep::spots_drawn())
}

fn view(h: &mut Headless, zoom: serde_json::Value, pan: [f32; 2]) {
    exec(h, "view.navigate", json!({"zoom": zoom, "pan": pan}));
    assert!(h.settle(SETTLE), "the view settles");
}

// Given a 24 MP photo with a brush, a linear and a radial mask and ten spots (Heal, Clone and Fill), zoomed to 100 %
// once, when I zoom to 200 % and back, go to Fit and back to 100 %, and pan a little, then no mask is built again and
// no spot is drawn again: each view reuses the window already rendered
#[test]
fn zooming_and_panning_a_photo_with_masks_and_spots_redoes_none_of_them() {
    let fixture = RenderFixture::new("zoom-reuse", 1);
    let app = LightcraftApp::new(fixture.session(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.app.renderer.set_offload(Box::new(Inline::default()));
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.5, 0.1], "end": [0.5, 0.6]}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": -0.5}}));
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5]}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": 0.5}}));
    exec(&mut h, "mask.add", json!({"kind": "brush"}));
    exec(&mut h, "mask.brushStroke", json!({"points": [[0.3, 0.5], [0.7, 0.5]], "size": 0.05}));
    exec(&mut h, "mask.adjust", json!({"values": {"saturation": 30}}));
    for (i, mode) in ["heal", "heal", "heal", "heal", "clone", "clone", "clone", "fill", "fill", "fill"].iter().enumerate() {
        let x = 0.42 + 0.02 * i as f64;
        exec(&mut h, "spot.add", json!({"mode": mode, "points": [[x, 0.48 + 0.005 * i as f64]], "size": 0.006, "source": [0.0, 0.03]}));
    }
    let d = h.app.session.develop_of(h.app.session.active().expect("a photo")).expect("settings");
    assert_eq!((d.masks.len(), d.spots.len()), (3, 10));
    assert!(h.settle(SETTLE), "Fit settles");
    view(&mut h, json!({"percent": 100}), [0.5, 0.5]);
    assert!(h.app.region_view.is_some(), "a window at 100 %");
    let first = work();
    assert!(first.0 > 0 && first.1 > 0, "the first window built the masks and drew the spots: {first:?}");
    let steps: [(&str, serde_json::Value, [f32; 2]); 6] = [
        ("200 %", json!({"percent": 200}), [0.5, 0.5]),
        ("back to 100 %", json!({"percent": 100}), [0.5, 0.5]),
        ("Fit", json!("fit"), [0.5, 0.5]),
        ("100 % again", json!({"percent": 100}), [0.5, 0.5]),
        ("a pan", json!({"percent": 100}), [0.52, 0.51]),
        ("200 % there", json!({"percent": 200}), [0.52, 0.51]),
    ];
    let mut redone = Vec::new();
    for (name, zoom, pan) in steps {
        let before = work();
        view(&mut h, zoom, pan);
        let after = work();
        redone.push((name, after.0 - before.0, after.1 - before.1));
    }
    assert!(redone.iter().all(|(_, masks, spots)| (*masks, *spots) == (0, 0)), "masks built and spots drawn again: {redone:?}");
}
