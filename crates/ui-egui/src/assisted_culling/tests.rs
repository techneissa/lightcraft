//! Assisted Culling in the app (fork, `fork/ai-culling.md`): the photos are read on worker threads under an activity
//! row that can stop; the eyes' Reject suggestions come in a dialog, and only what the user ticks is rejected.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lightcraft_catalog::{Flag, Op, Photo, PhotoId, Source};
use lightcraft_raster::Rgb32f;
use serde_json::{Value, json};

use super::{LABEL, chosen, suggestions};
use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(60);
const PHOTOS: u64 = 8;
/// How long a read waits for a drive that isn't opened (only a hang guard).
const GUARD: Duration = Duration::from_secs(20);

/// An app whose photos' files wait for `open` (a slow drive); reads made on the UI thread are counted.
fn app_on_a_slow_drive() -> (Headless, Arc<AtomicBool>, Arc<AtomicUsize>) {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services { png: None, ..Default::default() });
    for i in 0..PHOTOS {
        let path = format!("/drive/IMG_{i:04}.arw");
        let p = Photo::new(PhotoId(i + 1), Source::File { path }, &format!("IMG_{i:04}.arw"), "ARW", 64, 64, "2026-01-01T00:00:00");
        app.session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let (open, on_ui) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let (ui, gate, count) = (std::thread::current().id(), open.clone(), on_ui.clone());
    let deadline = Instant::now() + GUARD;
    app.session.media.file_loader = Some(Arc::new(move |_: &str, _, _, _| {
        if std::thread::current().id() == ui {
            count.fetch_add(1, Ordering::SeqCst);
        }
        // (a hang guard: the read goes on after it, so a regression fails the test instead of hanging it)
        while !gate.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok((Rgb32f::from_fn(64, 64, |x, _| [x as f32 / 64.0; 3]), Default::default()))
    }));
    (Headless::new(app, [1200.0, 800.0], 1.0), open, on_ui)
}

fn rows(h: &Headless) -> Vec<lightcraft_engine::activity::TaskInfo> {
    h.app.session.activity.list().into_iter().filter(|r| r.kind == lightcraft_engine::cmd::cull_run::KIND).collect()
}

fn ids() -> Value {
    json!((1..=PHOTOS).collect::<Vec<_>>())
}

/// From the menu the command answers at once, the row counts the photos and can stop, nothing is read on the UI
/// thread, and the scores are filed when the run ends.
#[test]
fn a_run_from_the_menu_reads_off_the_ui_thread() {
    let (mut h, open, on_ui) = app_on_a_slow_drive();
    let r = h.app.run("photo.analyze", json!({"ids": ids()})).unwrap();
    assert_eq!(r, json!({"background": true, "count": PHOTOS}));
    h.step();
    assert!(h.app.session.catalog.photos().all(|p| p.analysis.is_none()), "nothing filed before the files are read");
    let waited = Instant::now();
    while rows(&h).first().is_some_and(|r| r.total == 0) && waited.elapsed() < T {
        h.step();
        std::thread::sleep(Duration::from_millis(1));
    }
    let row = rows(&h).into_iter().next().expect("a row while it runs");
    assert_eq!((row.label.as_str(), row.total, row.cancellable), (LABEL, PHOTOS, true));
    assert!(h.app.run("photo.analyze", json!({"ids": ids()})).is_err(), "one run at a time");
    open.store(true, Ordering::SeqCst);
    assert!(h.step_until(T, |h| h.app.tasks.is_empty()), "the run never ended");
    assert_eq!(on_ui.load(Ordering::SeqCst), 0, "a file was read on the UI thread");
    assert!(rows(&h).is_empty());
    assert!(h.app.session.catalog.photos().all(|p| p.analysis.is_some_and(|a| a.eyes.is_none())));
    let toast = h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    assert!(toast.starts_with(&format!("Assisted Culling scored {PHOTOS} photos")), "{toast}");
    assert!(h.app.ui.dialog.is_none(), "no eyes, no suggestions");
}

/// A stopped run changes nothing.
#[test]
fn a_stopped_run_changes_nothing() {
    let (mut h, open, _) = app_on_a_slow_drive();
    h.app.run("photo.analyze", json!({"ids": ids()})).unwrap();
    let waited = Instant::now();
    while rows(&h).is_empty() && waited.elapsed() < T {
        h.step();
    }
    h.app.session.activity.cancel(rows(&h)[0].id).unwrap();
    open.store(true, Ordering::SeqCst);
    assert!(h.step_until(T, |h| h.app.tasks.is_empty()), "the run never ended");
    assert!(h.app.session.catalog.photos().all(|p| p.analysis.is_none()));
    assert_eq!(h.app.session.commits(), 0);
}

/// A control request is answered with the command's answer when the run ends.
#[test]
fn a_control_request_is_answered_when_the_run_ends() {
    let (mut h, open, _) = app_on_a_slow_drive();
    let rx = h.send("engine.execute", json!({"command": "photo.analyze", "params": {"ids": ids()}})).unwrap();
    open.store(true, Ordering::SeqCst);
    let t0 = Instant::now();
    let answer = loop {
        h.step();
        if let Ok(v) = rx.try_recv() {
            break v;
        }
        assert!(t0.elapsed() < T, "never answered");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(answer["result"]["photos"].as_array().map(Vec::len), Some(PHOTOS as usize), "{answer}");
    assert!(answer["result"]["eyes"]["unavailable"].is_string(), "{answer}");
}

/// The suggestions dialog: each kind is offered while it has photos, and only the ticked kinds are rejected, each
/// photo once, in one undo step; photos already rejected stay as they are.
#[test]
fn only_the_ticked_suggestions_are_rejected() {
    let answer = json!({"suggestReject": [
        {"id": 1, "why": ["eyesClosed"]},
        {"id": 2, "why": ["eyesClosed", "eyesOutOfFocus"]},
        {"id": 3, "why": ["eyesOutOfFocus"]},
    ]});
    let Some(Dialog::CullSuggest { closed, soft, reject_closed, reject_soft }) = suggestions(&answer) else { panic!("a dialog") };
    assert_eq!((closed.clone(), soft.clone(), reject_closed, reject_soft), (vec![1, 2], vec![2, 3], true, true));
    assert_eq!(chosen(&closed, &soft, true, true), vec![PhotoId(1), PhotoId(2), PhotoId(3)]);
    assert_eq!(chosen(&closed, &soft, false, true), vec![PhotoId(2), PhotoId(3)]);
    assert!(suggestions(&json!({"suggestReject": []})).is_none());

    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    // (what a screenshot needs)
    let services = Services { png: Some(Box::new(png)), write_new: Some(Box::new(lightcraft_engine::export::write_file_new)), ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1300.0, 1000.0], 1.0);
    h.settle(T);
    let shot = |h: &mut Headless, name: &str| {
        if let Some(p) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
            h.step();
            let path = std::path::Path::new(&p).join(name);
            // (a screenshot writes a new file only)
            let _ = std::fs::remove_file(&path);
            let r = h.request("ui.screenshot", json!({"path": path.to_string_lossy(), "headless": true}), T);
            assert_eq!(r["ok"], true, "{r}");
        }
    };
    h.app.run("dialog.cull", json!({})).unwrap();
    h.step();
    h.step();
    shot(&mut h, "cull-dialog.png");
    h.app.ui.dialog = None;
    let demo: Vec<PhotoId> = h.app.session.visible_cloned().into_iter().take(3).collect();
    h.app.session.commit("t", Op::SetFlag { id: demo[2], flag: Flag::Reject }).unwrap();
    let before = h.app.session.commits();
    // (as a run offers it: every kind that has photos ticked)
    let offered = json!({"suggestReject": [
        {"id": demo[0].0, "why": ["eyesClosed"]},
        {"id": demo[1].0, "why": ["eyesOutOfFocus"]},
        {"id": demo[2].0, "why": ["eyesOutOfFocus"]},
    ]});
    h.app.ui.dialog = suggestions(&offered);
    h.step();
    h.step();
    shot(&mut h, "cull-suggest.png");
    // the user unticks Eyes Closed
    if let Some(Dialog::CullSuggest { reject_closed, .. }) = &mut h.app.ui.dialog {
        *reject_closed = false;
    }
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    let flag = |h: &Headless, k: usize| h.app.session.catalog.photo(demo[k]).unwrap().flag;
    assert_eq!((flag(&h, 0), flag(&h, 1), flag(&h, 2)), (Flag::None, Flag::Reject, Flag::Reject));
    assert_eq!(h.app.session.commits(), before + 1, "one undo step");
}
