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

fn temp(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-learn-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn harness(tag: &str, statistics_only: bool) -> (Headless, std::path::PathBuf) {
    let dir = temp(tag);
    with_services(&dir, statistics_only, Services { png: None, ..Default::default() })
}

fn with_services(dir: &std::path::Path, statistics_only: bool, services: Services) -> (Headless, std::path::PathBuf) {
    let dir = dir.to_path_buf();
    let session = lightcraft_engine::Session::with_demo();
    session.learn.set_dirs(Some(dir.join("models")), Some(dir.join("profiles")));
    session.learn.set_statistics_only(statistics_only);
    let app = LightcraftApp::new(session, services);
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

fn click(h: &mut Headless, id: &str) {
    h.step();
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

/// No error under the window: the last button worked.
fn no_error(h: &Headless, what: &str) {
    if let Some((text, true)) = &h.app.ui.learn_edits.note {
        panic!("{what}: {text}");
    }
}

/// Steps until no Learn My Edits job runs (the frame loop takes it in).
fn settle_job(h: &mut Headless) {
    assert!(h.step_until(SETTLE, |h| h.app.session.learn.running().is_none()), "the job never ended");
    h.step();
}

#[test]
fn every_button_of_the_window_works() {
    let dir = temp("buttons");
    let export = dir.join("out.lclearn");
    let answer = export.to_string_lossy().into_owned();
    let services = Services {
        png: None,
        // the file dialogs: Export… saves to `out.lclearn`, Import… opens it
        picker: Some(Box::new(move |_req| {
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(vec![answer.clone()]).unwrap();
            Ok(rx)
        })),
        ..Default::default()
    };
    let (mut h, dir) = with_services(&dir, true, services);
    // twelve photos edited alike: a fixed edit is a pattern too
    let ids: Vec<u64> = h.app.session.catalog.photos().map(|p| p.id.0).take(14).collect();
    let (examples, new) = ids.split_at(12);
    // (the demo photos come edited: start them from their defaults)
    h.app.session.execute("develop.reset", &json!({"ids": ids})).unwrap();
    h.app.session.execute("develop.set", &json!({"ids": examples, "values": {"light.exposure": 0.6, "light.shadows": 30}})).unwrap();
    h.app.session.execute("library.select", &json!({"ids": examples})).unwrap();
    h.request("ui.menu.invoke", json!({"id": "dialog.learnEdits"}), T);
    h.step();

    // New Profile… → name → Create and Train
    click(&mut h, "button:learnNew");
    click(&mut h, "field:learnName");
    let r = h.request("ui.text", json!({"text": "Clicks"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    click(&mut h, "button:learnCreate");
    no_error(&h, "Create and Train");
    settle_job(&mut h);
    let listed = h.app.session.learn.list();
    assert_eq!(listed.len(), 1);
    let p = listed[0].profile.as_ref().unwrap();
    assert!(p.model.is_some(), "trained: {:?} {:?}", p.problem, h.app.ui.learn_edits.note);

    // Apply to Selected Photos, on two new photos
    h.app.session.execute("library.select", &json!({"ids": new})).unwrap();
    click(&mut h, "button:learnApply");
    no_error(&h, "Apply");
    settle_job(&mut h);
    for id in new {
        let d = h.app.session.develop_of(lightcraft_engine::catalog::PhotoId(*id)).unwrap();
        assert!((d.light.exposure - 0.6).abs() < 0.05, "{}", d.light.exposure);
    }

    // Add Selected Photos as Examples, then Train Again
    click(&mut h, "button:learnAddExamples");
    no_error(&h, "Add Examples");
    settle_job(&mut h);
    assert_eq!(h.app.session.learn.list()[0].profile.as_ref().unwrap().examples.len(), 14);
    click(&mut h, "button:learnTrain");
    no_error(&h, "Train Again");
    settle_job(&mut h);

    // Export… and Import… through the file dialog
    click(&mut h, "button:learnExport");
    no_error(&h, "Export");
    assert!(export.is_file(), "exported");
    click(&mut h, "button:learnImport");
    no_error(&h, "Import");
    assert_eq!(h.app.session.learn.list().len(), 2, "imported a copy");

    // Delete… asks, then deletes
    click(&mut h, "button:learnDelete");
    click(&mut h, "button:learnDeleteConfirm");
    no_error(&h, "Delete");
    assert_eq!(h.app.session.learn.list().len(), 1);

    // Stop, while a job runs
    h.app.session.learn.hold_jobs(true);
    click(&mut h, "button:learnTrain");
    assert!(h.app.session.learn.running().is_some());
    click(&mut h, "button:learnStop");
    h.app.session.learn.hold_jobs(false);
    settle_job(&mut h);
    no_error(&h, "Stop");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_group_lists_its_learned_and_its_skipped_sliders() {
    let group = json!({"group": "tone", "kept": true, "sliders": [
        {"id": "light.exposure", "error": 0.18, "defaultError": 1.0, "learned": true},
        {"id": "light.contrast", "error": 70.0, "defaultError": 40.0, "learned": false},
        {"id": "light.shadows", "error": 0.0, "defaultError": 20.0, "learned": true},
    ]});
    let (learned, skipped) = crate::learn_edits::group_lines(&group);
    assert_eq!(learned, vec!["Exposure ±0.18 EV".to_string(), "Shadows ±0".to_string()]);
    assert_eq!(skipped, vec!["Contrast".to_string()]);
    // a report saved before sliders were decided one by one: the group decides for all of them
    let old = json!({"group": "tone", "kept": true, "sliders": [{"id": "light.contrast", "error": 4.0, "defaultError": 12.0}]});
    assert_eq!(crate::learn_edits::group_lines(&old), (vec!["Contrast ±4".to_string()], Vec::new()));
    let dropped = json!({"group": "tone", "kept": false, "sliders": [{"id": "light.contrast", "error": 40.0, "defaultError": 12.0}]});
    assert_eq!(crate::learn_edits::group_lines(&dropped), (Vec::new(), vec!["Contrast".to_string()]));
}
