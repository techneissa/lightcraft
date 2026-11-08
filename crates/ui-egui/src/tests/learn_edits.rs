//! Fork feature (`fork/learn-my-edits.md`): the Learn My Edits window opens from the Photo menu, lists the profiles,
//! offers the model download without starting it, and its New Profile form needs a name and photos.

use crate::headless::Headless;
use crate::{LightcraftApp, Services};
use serde_json::json;
use std::time::Duration;

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(60);

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn harness(tag: &str, statistics_only: bool) -> (Headless, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-learn-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let session = lightcraft_engine::Session::with_demo();
    session.learn.set_dirs(Some(dir.join("models")), Some(dir.join("profiles")));
    session.learn.set_statistics_only(statistics_only);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1300.0, 900.0], 1.0);
    h.settle(SETTLE);
    (h, dir)
}

#[test]
fn the_window_opens_from_the_photo_menu_and_asks_before_downloading() {
    let (mut h, dir) = harness("open", false);
    let r = h.request("ui.menu.invoke", json!({"id": "dialog.learnEdits"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let text = h.painted_text();
    assert!(text.iter().any(|t| t == "Learn My Edits"), "{text:?}");
    assert!(text.iter().any(|t| t == "No profiles yet."), "{text:?}");
    assert!(text.iter().any(|t| t.contains("needs an image model (88 MB)")), "{text:?}");
    // Download… only asks; nothing starts until the second click
    let r = h.request("ui.clickWidget", json!({"id": "button:learnDownload"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.painted_text().iter().any(|t| t.contains("from huggingface.co?")));
    assert!(has(&h, "button:learnDownloadConfirm"));
    assert!(matches!(h.app.session.learn.download_state(), lightcraft_engine::learn::Download::Idle));
    let r = h.request("ui.clickWidget", json!({"id": "button:learnDownloadCancel"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(!has(&h, "button:learnDownloadConfirm"));
    // closing it
    h.app.ui.learn_edits.open = false;
    h.step();
    assert!(!h.painted_text().iter().any(|t| t == "No profiles yet."));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_profile_is_listed_with_what_it_learns_and_the_form_needs_a_name() {
    let (mut h, dir) = harness("list", true);
    h.app
        .session
        .execute("learn.create", &json!({"name": "Portraits", "kind": "Skin Tones", "groups": ["tone", "whiteBalance"], "empty": true}))
        .unwrap();
    h.request("ui.menu.invoke", json!({"id": "dialog.learnEdits"}), T);
    h.step();
    h.step();
    let text = h.painted_text();
    assert!(text.iter().any(|t| t.starts_with("Portraits")), "{text:?}");
    assert!(text.iter().any(|t| t == "Not trained yet."), "{text:?}");
    assert!(has(&h, "button:learnApply") && has(&h, "button:learnTrain"));
    // New Profile…: Create and Train waits for a name
    let r = h.request("ui.clickWidget", json!({"id": "button:learnNew"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.learn_edits.new.is_some());
    assert!(has(&h, "button:learnCreate") && has(&h, "field:learnName"));
    let r = h.request("ui.clickWidget", json!({"id": "button:learnCreate"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert_eq!(h.app.session.learn.list().len(), 1, "a nameless profile is not made");
    let _ = std::fs::remove_dir_all(dir);
}
