//! Issue #97: a spot edit no longer waits on a second full render or on the source search. With the Remove panel open,
//! the open photo's thumbnail renders once at the end of a burst of spot edits; a spot painted on the photo is added at
//! with its source, found on a worker, a frame or two later.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn remove_panel() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail", "filmstrip": true}), T)["ok"], true);
    assert_eq!(h.request("engine.execute", json!({"command": "panel.remove"}), T)["ok"], true);
    assert!(h.settle(SETTLE));
    h
}

fn spots(h: &Headless) -> Vec<lightcraft_develop::Spot> {
    h.app.session.develop_of(h.app.session.active().unwrap()).unwrap_or_default().spots.clone()
}

#[test]
fn a_burst_of_spot_edits_renders_the_thumbnail_once_at_the_end() {
    let mut h = remove_panel();
    let id = h.app.session.active().expect("a photo is open");
    assert!(h.app.renderer.thumb(id).is_some(), "the filmstrip shows its thumbnail");
    let mut asked = HashSet::new();
    asked.insert(h.app.renderer.wanted(Slot::Thumb(id)));
    // five spots, about a twentieth of a second apart
    for k in 0..5 {
        let add =
            json!({"command": "spot.add", "params": {"mode": "heal", "points": [[0.2 + 0.1 * k as f64, 0.5]], "size": 0.02, "source": [0.05, 0.0]}});
        assert_eq!(h.request("engine.execute", add, T)["ok"], true);
        for _ in 0..3 {
            h.step();
            asked.insert(h.app.renderer.wanted(Slot::Thumb(id)));
        }
    }
    assert_eq!(asked.len(), 1, "no thumbnail render during the burst");
    // the edits pause and the loupe has its render: one thumbnail render
    let t0 = std::time::Instant::now();
    let mut frames = 0;
    while frames < 60 || h.app.renderer.is_pending(Slot::Main) {
        assert!(t0.elapsed() < SETTLE, "the loupe never finished");
        h.step();
        frames += 1;
        asked.insert(h.app.renderer.wanted(Slot::Thumb(id)));
        std::thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..30 {
        h.step();
        asked.insert(h.app.renderer.wanted(Slot::Thumb(id)));
    }
    assert_eq!(asked.len(), 2, "one thumbnail render once the burst paused");
    assert_eq!(spots(&h).len(), 5);
}

#[test]
fn a_painted_spot_gets_its_source_from_the_search_on_a_worker() {
    let mut h = remove_panel();
    // the window sleeps between frames: the worker must wake it when the search answers, or the spot stays unseen
    // (and a second click adds another) until some later redraw
    assert!(h.app.session.spot_sources.has_waker(), "the window gave the search a way to wake it");
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.4, "y": 0.6}, {"kind": "up", "x": 0.4, "y": 0.6}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.settle(SETTLE));
    assert_eq!(spots(&h).len(), 1, "the spot is added once its source is found");
    assert!(spots(&h)[0].source_offset.is_some(), "with its source");
    assert_eq!(h.app.session.spot_sources.pending(), 0);
}
