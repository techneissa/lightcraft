//! The finished full-size picture in the loupe (#178): once it is there, zoom and pan render nothing; an edit never
//! shows the old one; going back to a photo reads it from disk.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::render::Slot;
use crate::tests_render_fixtures::RenderFixture;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(180);

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
}

fn view(h: &mut Headless, zoom: serde_json::Value, pan: [f32; 2]) {
    exec(h, "view.navigate", json!({"zoom": zoom, "pan": pan}));
    assert!(h.settle(SETTLE), "the view settles");
}

/// The loupe on the fixture's first photo, with full-size pictures on (and, with `disk`, a disk cache there).
fn loupe(fixture: &RenderFixture, disk: Option<&std::path::Path>) -> Headless {
    let mut session = fixture.session();
    if let Some(dir) = disk {
        session.media.attach_disk_cache(dir, 4 << 30);
    }
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.app.renderer.full.enabled = true;
    let r = h.request("ui.set", json!({"view": "detail", "right": "none", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
    h
}

/// Run frames until the photo's finished picture is up, with nothing being made for it.
fn until_ready(h: &mut Headless) {
    let ready = h.step_until(SETTLE, |h| h.app.renderer.full.ready() && !h.app.renderer.is_pending(Slot::Full));
    assert!(ready, "the finished picture comes: {}", h.app.renderer.full.to_json());
}

// Given a photo with a mask and spots whose finished picture is ready, when I zoom Fit → 50 % → 100 % → 200 %, pan
// anywhere and go back to Fit, then no render starts, and every zoomed view is drawn from the picture
#[test]
fn zoom_and_pan_draw_the_finished_picture_and_start_no_render() {
    let fixture = RenderFixture::new("full-picture-zoom", 1);
    let mut h = loupe(&fixture, None);
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.5, 0.1], "end": [0.5, 0.6]}));
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": -0.5}}));
    for (i, mode) in ["heal", "clone", "fill"].iter().enumerate() {
        let x = 0.42 + 0.04 * i as f64;
        exec(&mut h, "spot.add", json!({"mode": mode, "points": [[x, 0.5]], "size": 0.006, "source": [0.0, 0.03]}));
    }
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    assert_eq!(h.app.renderer.full.stats.rendered, 1);
    let started = h.app.renderer.started;
    let steps: [(&str, serde_json::Value, [f32; 2]); 8] = [
        ("50 %", json!({"percent": 50}), [0.5, 0.5]),
        ("100 %", json!({"percent": 100}), [0.5, 0.5]),
        ("200 %", json!({"percent": 200}), [0.5, 0.5]),
        ("a pan to a corner", json!({"percent": 200}), [0.05, 0.05]),
        ("a pan to the other", json!({"percent": 200}), [0.95, 0.95]),
        ("100 % there", json!({"percent": 100}), [0.95, 0.95]),
        ("Fit", json!("fit"), [0.5, 0.5]),
        ("100 % again", json!({"percent": 100}), [0.3, 0.7]),
    ];
    for (name, zoom, pan) in steps {
        view(&mut h, zoom, pan);
        let fit = name == "Fit";
        assert_eq!(h.app.renderer.full.on_screen, !fit, "{name}: drawn from the finished picture");
        assert!(h.app.region_view.is_none(), "{name}: no window render");
        assert_eq!(h.app.renderer.started, started, "{name}: no render started");
    }
}

// Given the finished picture of a photo, when I change Exposure and zoom in at once, then the old picture is never
// drawn: the zoomed view renders its window as before, and the new picture follows
#[test]
fn an_edit_then_a_zoom_never_draws_the_old_picture() {
    let fixture = RenderFixture::new("full-picture-edit", 1);
    let mut h = loupe(&fixture, None);
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    let (_, old) = h.app.renderer.full.held().expect("a picture");
    let started = h.app.renderer.started;
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.7}}));
    exec(&mut h, "view.navigate", json!({"zoom": {"percent": 100}, "pan": [0.5, 0.5]}));
    // every frame on the way to the new picture: the old one is never what the loupe drew
    let drew_old = std::cell::Cell::new(0);
    let ready = h.step_until(SETTLE, |h| {
        let full = &h.app.renderer.full;
        if full.on_screen && full.held().is_some_and(|(_, k)| k == old) {
            drew_old.set(drew_old.get() + 1);
        }
        full.ready() && full.held().is_some_and(|(_, k)| k != old)
    });
    assert!(ready, "the new picture comes: {}", h.app.renderer.full.to_json());
    assert_eq!(drew_old.get(), 0, "frames drawn from the old picture");
    assert!(h.app.renderer.started > started + 1, "the edit and the zoomed view rendered meanwhile");
    view(&mut h, json!({"percent": 200}), [0.5, 0.5]);
    assert!(h.app.renderer.full.on_screen, "the new picture is drawn");
}

// Given a photo whose finished picture was made and kept on disk, when I go to another photo and back, then it is
// read from disk and nothing renders it again; and once its settings change, the file of the old look is not used
#[test]
fn going_back_to_a_photo_reads_its_picture_from_disk_and_a_new_look_does_not() {
    let fixture = RenderFixture::new("full-picture-disk", 2);
    let dir = std::env::temp_dir().join(format!("lc-full-picture-{}-{}", std::process::id(), lightcraft_preview::next_tick()));
    let mut h = loupe(&fixture, Some(&dir));
    let first = h.app.session.active().expect("a photo");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": 0.4}}));
    assert!(h.settle(SETTLE), "Fit settles");
    until_ready(&mut h);
    let written = |h: &Headless| h.app.session.media.rendered.disk().map_or(0, |d| d.full_writes.load(std::sync::atomic::Ordering::Relaxed));
    assert!(h.step_until(SETTLE, |h| written(h) >= 1), "the picture is written to disk");
    exec(&mut h, "library.next", json!({}));
    assert_ne!(h.app.session.active(), Some(first));
    assert!(h.settle(SETTLE), "the other photo settles");
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.rendered, stats.from_disk), (2, 0), "{stats:?}");
    exec(&mut h, "library.previous", json!({}));
    assert_eq!(h.app.session.active(), Some(first));
    assert!(h.settle(SETTLE), "back on the first photo");
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.rendered, stats.from_disk), (2, 1), "read back, not rendered: {stats:?}");
    exec(&mut h, "develop.set", json!({"values": {"light.exposure": -0.4}}));
    assert!(h.settle(SETTLE), "the edit settles");
    until_ready(&mut h);
    let stats = h.app.renderer.full.stats;
    assert_eq!((stats.rendered, stats.from_disk), (3, 1), "the new look is rendered, not read: {stats:?}");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
