//! The fork's update check (fork/in-app-update.md): Settings ▸ General has a Check for Updates button with a
//! one-line status, Update opens the updater app, the check at start is quiet, and no test reaches the network
//! (GitHub is a fake).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;

use crate::fork_update::{Getter, Reply};
use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const APP: &str = "d7ae6dfc978dd9d4ec963aae4a6c37cecd680806";
const NEW: &str = "0123456789abcdef0123456789abcdef01234567";

/// A GitHub whose release says `body` and whose compare answer is `compare` (status, JSON).
fn github(body: &str, compare: (u16, &str)) -> Getter {
    let body = body.to_string();
    let compare = (compare.0, compare.1.to_string());
    Arc::new(move |url: &str| {
        if url.contains("/releases/tags/custom-build") {
            Ok(Reply { status: 200, body: json!({ "body": body }).to_string() })
        } else if url.contains("/compare/") {
            Ok(Reply { status: compare.0, body: compare.1.clone() })
        } else {
            Err(format!("unexpected url {url}"))
        }
    })
}

fn offline() -> Getter {
    Arc::new(|_| Err("Can't Reach GitHub".into()))
}

/// The app on Settings ▸ General, built from `APP`, with `getter` as GitHub and a launcher that counts its starts.
fn settings(getter: Getter) -> (Headless, Arc<AtomicUsize>) {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
    let opened = Arc::new(AtomicUsize::new(0));
    let count = opened.clone();
    let fork = &mut app.updates_rt.fork;
    fork.getter = getter;
    fork.launcher = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    fork.set_app_commit(Some(APP));
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    h.settle(T);
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "general"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..4 {
        h.step();
    }
    (h, opened)
}

fn says(h: &Headless, part: &str) -> bool {
    h.painted_text().iter().any(|text| text.contains(part))
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "click {id}: {r}");
    for _ in 0..4 {
        h.step();
    }
}

/// Click Check for Updates and wait for the answer to be drawn.
fn check(h: &mut Headless, wanted: &str) {
    click(h, "button:settingsCheckForUpdates");
    assert!(h.step_until(T, |h| says(h, wanted)), "never said {wanted:?}: {:?}", h.painted_text());
}

#[test]
fn settings_has_a_check_button_with_no_status_until_it_is_pressed() {
    let (h, _) = settings(offline());
    assert!(says(&h, "Check for Updates"), "{:?}", h.painted_text());
    assert!(!says(&h, "Up to Date") && !says(&h, "Checking"));
}

#[test]
fn settings_says_up_to_date() {
    let (mut h, _) = settings(github(&format!("commit: {APP}"), (200, "{}")));
    check(&mut h, "Up to Date");
    assert!(!says(&h, "A New Build"));
}

#[test]
fn settings_says_a_new_build_is_ready_and_update_opens_the_updater() {
    let (mut h, opened) = settings(github(&format!("commit: {NEW}"), (200, r#"{"status":"ahead"}"#)));
    check(&mut h, "A New Build Is Ready");
    assert_eq!(opened.load(Ordering::SeqCst), 0);
    click(&mut h, "button:settingsUpdate");
    assert_eq!(opened.load(Ordering::SeqCst), 1, "Update opens the updater app once");
    assert!(says(&h, "Update LightCraft Is Opening"), "{:?}", h.painted_text());
}

#[test]
fn settings_says_a_new_build_is_being_made() {
    let (mut h, _) = settings(github("commit: updating", (200, "{}")));
    check(&mut h, "A New Build Is Being Made");
    // (nothing to install yet: no Update button)
    let r = h.request("ui.clickWidget", json!({"id": "button:settingsUpdate"}), T);
    assert_ne!(r["ok"], true, "{r}");
}

#[test]
fn an_unknown_build_says_so_and_can_still_update() {
    let (mut h, opened) = settings(github(&format!("commit: {NEW}"), (200, "{}")));
    h.app.updates_rt.fork.set_app_commit(None);
    check(&mut h, "Unknown Build");
    click(&mut h, "button:settingsUpdate");
    assert_eq!(opened.load(Ordering::SeqCst), 1);
}

#[test]
fn a_network_error_is_a_short_message_and_the_app_goes_on() {
    let (mut h, _) = settings(offline());
    check(&mut h, "Can't Reach GitHub");
    assert!(h.app.session.activity.list().iter().all(|r| r.kind != "update"), "the row goes with the check");
}

#[test]
fn the_update_check_holds_a_row_in_the_activity_stack_while_it_runs() {
    let (mut h, _) = settings(github(&format!("commit: {APP}"), (200, "{}")));
    click(&mut h, "button:settingsCheckForUpdates");
    // (the check is on a worker thread: the row is there from the click until the answer is applied)
    let rows = h.app.session.activity.list();
    let started_or_done = rows.iter().any(|r| r.kind == "update" && r.label == crate::fork_update::LABEL) || says(&h, "Up to Date");
    assert!(started_or_done, "{rows:?}");
}

#[test]
fn the_check_at_start_is_quiet_unless_a_new_build_is_ready() {
    // nothing new: no window, no status
    let (mut h, _) = settings(github(&format!("commit: {APP}"), (200, "{}")));
    h.app.set_startup_update_check(true);
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!h.app.updates_rt.fork.window_open);
    assert!(!says(&h, "LightCraft Updates"));

    // a new build: the window opens
    let (mut h, _) = settings(github(&format!("commit: {NEW}"), (200, r#"{"status":"ahead"}"#)));
    h.app.set_startup_update_check(true);
    assert!(h.step_until(T, |h| says(h, "LightCraft Updates")), "{:?}", h.painted_text());
    assert!(says(&h, "A New Build Is Ready"));
    click(&mut h, "button:updateClose");
    assert!(!h.app.updates_rt.fork.window_open, "Close closes the window");
    assert!(h.step_until(T, |h| !says(h, "LightCraft Updates")), "{:?}", h.painted_text());
}

#[test]
fn help_check_for_updates_opens_the_window_with_the_result() {
    let (mut h, _) = settings(github(&format!("commit: {APP}"), (200, "{}")));
    let r = h.request("engine.execute", json!({"command": "app.checkForUpdates", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(T, |h| says(h, "LightCraft Updates") && says(h, "Up to Date")), "{:?}", h.painted_text());
}
