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
    /// The next full-size decode panics (a worker dying).
    panic_next: Arc<std::sync::atomic::AtomicBool>,
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
            if d.panic_next.swap(false, Ordering::SeqCst) {
                panic!("a decoder bug");
            }
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

/// The read-ahead's row in the activity stack, if listed.
fn row(h: &Headless) -> Option<lightcraft_engine::activity::TaskInfo> {
    h.app.session.activity.list().into_iter().find(|t| t.kind == "decode")
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
    assert!(row(&h).is_none(), "its activity row went when it was done");

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
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.running_for(first) && decodes.full() == 1), "the first photo's original is being read");
    let listed = row(&h).expect("the read-ahead has an activity row");
    assert_eq!(listed.label, crate::full_decode::LABEL);
    assert!(!listed.cancellable, "no ✕: it never holds up quitting");
    assert!(h.app.session.activity.running_cancellable().is_empty(), "quitting asks about nothing");

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
    assert!(row(&h).is_none(), "and both rows went");
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

// Given a photo opened in the Edit view, when it has been on screen for less than the dwell (stepping through photos),
// then no read-ahead starts; it starts once the photo has stayed on screen
#[test]
fn a_photo_only_glanced_at_is_not_read_ahead() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.renderer.textures.contains_key(&crate::render::Slot::Main)));
    for _ in 0..((crate::full_decode::DWELL_SECS * 30.0) as usize) {
        h.step();
    }
    assert_eq!(h.app.full_decode.started(), 0, "half the dwell: nothing read");
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.started() == 1), "then it is");
    decodes.release();
    assert!(h.settle(SETTLE));
}

// Given a read-ahead decoding, when I leave the Edit view, then it is stopped, its original dropped and its row gone
#[test]
fn leaving_the_edit_view_stops_the_read_ahead() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    let id = h.app.session.active().expect("a photo is open");
    decodes.hold();
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.running_for(id) && decodes.full() == 1));
    assert!(row(&h).is_some());
    assert_eq!(h.request("ui.set", json!({"right": "none"}), T)["ok"], true);
    h.step();
    assert!(!h.app.full_decode.running_for(id), "stopped");
    decodes.release();
    assert!(h.step_until(SETTLE, |h| !h.app.full_decode.busy()));
    assert!(h.settle(SETTLE));
    assert!(!h.app.session.media.has_source(id, SourceLevel::Full), "its original was dropped");
    assert!(row(&h).is_none(), "its row went");
}

// Given the read-ahead queued at the memory gate (AI Denoise holding it), when I zoom to 100 %, then the window does
// not wait behind it: the read-ahead is stopped and the window decodes the original itself
#[test]
fn a_zoom_does_not_wait_for_a_read_ahead_still_queued() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    let id = h.app.session.active().expect("a photo is open");
    let gate = lightcraft_engine::memory::work_gate();
    let held = gate.acquire_urgent(gate.usage().1.max(1));
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.running_for(id)), "the read-ahead started");
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(decodes.full(), 0, "queued at the gate");
    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    h.step();
    assert!(!h.app.full_decode.running_for(id), "the queued read-ahead was stopped");
    drop(held);
    assert!(h.step_until(SETTLE, |h| h.app.region_view.is_some_and(|v| v.photo == id)), "the window arrived");
    assert!(h.settle(SETTLE));
    assert_eq!(decodes.full(), 1, "decoded once, by the window");
    assert!(row(&h).is_none());
}

// Given a read-ahead whose worker dies (a decoder panic), when I zoom to 100 %, then the window does not wait for it
// forever: it decodes the original itself
#[test]
fn a_read_ahead_that_died_does_not_hold_up_the_zoom() {
    let fixture = RenderFixture::new("full-decode", 2);
    let (mut h, decodes) = editing(&fixture);
    let id = h.app.session.active().expect("a photo is open");
    decodes.panic_next.store(true, Ordering::SeqCst);
    open_in_edit(&mut h);
    assert!(h.step_until(SETTLE, |h| h.app.full_decode.started() == 1 && !h.app.full_decode.busy()), "it ran and died");
    assert!(!h.app.session.media.has_source(id, SourceLevel::Full));
    h.request("engine.execute", json!({"command": "view.zoom100"}), T);
    assert!(h.step_until(SETTLE, |h| h.app.region_view.is_some_and(|v| v.photo == id)), "the window arrived");
    assert!(h.settle(SETTLE));
    assert_eq!(decodes.full(), 2, "the window decoded it");
    assert!(row(&h).is_none());
}
