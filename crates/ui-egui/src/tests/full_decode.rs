//! The open photo's original read ahead in the Edit view (#182, `crate::full_decode`): one decode per photo, stopped
//! when another photo opens, and the first zoom to 100 % uses it instead of decoding the file again.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lightcraft_engine::SourceLevel;
use serde_json::json;

use crate::headless::Headless;
use crate::tests_render_fixtures::RenderFixture;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// Counts the full-size decodes of the fixture's files and, while `held`, keeps them waiting.
#[derive(Clone, Default)]
struct Decodes {
    full: Arc<AtomicUsize>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Decodes {
    fn hold(&self) {
        *self.gate.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.gate.1.notify_all();
    }

    fn full(&self) -> usize {
        self.full.load(Ordering::SeqCst)
    }
}

fn editing(fixture: &RenderFixture) -> (Headless, Decodes) {
    let decodes = Decodes::default();
    let mut session = fixture.session();
    let plain = session.media.file_loader.clone().expect("the fixture session reads files");
    let d = decodes.clone();
    session.media.file_loader = Some(Arc::new(move |path: &str, max_edge, options, with| {
        if max_edge == usize::MAX {
            d.full.fetch_add(1, Ordering::SeqCst);
            let (held, wake) = &*d.gate;
            let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
            // (a guard against a hung test, not a timing: released long before)
            let until = std::time::Instant::now() + SETTLE;
            while *held && std::time::Instant::now() < until {
                held = wake.wait_timeout(held, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
        plain(path, max_edge, options, with)
    }));
    // room for a 24 MP original in the source caches' share, whatever this machine's default
    session.media.set_budget(2 << 30);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    (Headless::new(app, [1400.0, 900.0], 1.0), decodes)
}

fn open_in_edit(h: &mut Headless) {
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "filmstrip": false}), T);
    assert_eq!(r["ok"], true, "{r}");
}

// Given a 24 MP photo, when I open it in the Edit view, then its original is decoded once in the background, and
// the first zoom to 100 % draws its window from that decode without reading the file again
#[test]
fn the_edit_view_reads_the_original_once_and_the_first_zoom_uses_it() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    let id = h.app.session.active().expect("a photo is open");
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.started() == 1 && !h.app.full_decode.busy()), "the read-ahead ran");
    assert!(h.settle(SETTLE));
    assert_eq!(h.app.full_decode.started(), 1, "one read-ahead");
    assert_eq!(decodes.full(), 1, "one full-size decode");
    assert!(h.app.session.media.has_source(id, SourceLevel::Full), "the original is in the source cache");

    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    assert!(h.settle(SETTLE));
    assert!(h.app.region_view.is_some_and(|v| v.photo == id), "the window at 100 % arrived");
    assert_eq!(decodes.full(), 1, "the zoom decoded nothing again");
    assert_eq!(h.app.full_decode.started(), 1);
}

// Given the read-ahead of one photo still decoding, when I open another, then the first is stopped and its original
// dropped, and the second photo's starts only once the first has left its worker
#[test]
fn opening_another_photo_stops_the_read_ahead() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    let first = h.app.session.active().expect("a photo is open");
    let second = h.app.session.visible_cloned().into_iter().find(|id| *id != first).expect("a second photo");
    decodes.hold();
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.running_for(first)), "the first photo's original is being read");

    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [second.0]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(T, |h| h.app.session.active() == Some(second)));
    h.step();
    assert!(!h.app.full_decode.running_for(first), "stopped");
    assert!(h.app.full_decode.busy(), "its decode is still on the worker");
    assert_eq!(h.app.full_decode.started(), 1, "no second decode beside it");

    decodes.release();
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.started() == 2 && !h.app.full_decode.busy()), "the second photo's ran");
    assert!(h.settle(SETTLE));
    assert!(!h.app.session.media.has_source(first, SourceLevel::Full), "the stopped decode was dropped");
    assert_eq!(h.app.full_decode.started(), 2, "then the second photo's");
    assert!(h.app.session.media.has_source(second, SourceLevel::Full));
    assert_eq!(decodes.full(), 2);
}

// Given the read-ahead still decoding, when I zoom to 100 %, then the window waits for it instead of decoding the
// file a second time, and arrives once it is in
#[test]
fn a_zoom_during_the_read_ahead_waits_for_it() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    let id = h.app.session.active().expect("a photo is open");
    decodes.hold();
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.running_for(id) && decodes.full() == 1), "the original is being read");

    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    for _ in 0..20 {
        h.step();
    }
    assert!(!h.app.renderer.is_pending(crate::render::Slot::Region), "the window waits");
    assert_eq!(decodes.full(), 1, "no second decode");

    decodes.release();
    assert!(h.step_until(SETTLE, |h| h.app.region_view.is_some_and(|v| v.photo == id)), "the window arrived");
    assert!(h.settle(SETTLE));
    assert_eq!(decodes.full(), 1, "from the one decode");
}
