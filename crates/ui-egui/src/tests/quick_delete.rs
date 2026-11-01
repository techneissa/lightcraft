//! Quick delete: ⌫ in Recently Deleted deletes for good; Settings ▸ General ▸ "Only for picked or
//! starred photos" skips the delete confirmation when nothing picked or starred is in the set.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{DeleteFiles, Dialog};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// Three imported photos a.png, b.png, c.png.
fn app(tag: &str) -> (Headless, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-ui-quick-delete-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (seed, name) in [(60, "a.png"), (120, "b.png"), (180, "c.png")] {
        let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[seed, 120, 160, 255]; 64] };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let session = lightcraft_engine::Session::new().with_fs();
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), [1400.0, 900.0], 1.0);
    h.step();
    h.app.run("library.import", json!({"paths": [dir.to_string_lossy()]})).unwrap();
    h.app.run("library.selectAll", json!({})).unwrap();
    h.step();
    (h, dir)
}

fn id(h: &Headless, name: &str) -> u64 {
    h.app.session.catalog.photos().find(|p| p.file_name == name).unwrap().id.0
}

fn deleted(h: &Headless, name: &str) -> bool {
    h.app.session.catalog.photos().find(|p| p.file_name == name).unwrap().deleted
}

fn key_delete(h: &mut Headless) {
    h.request("ui.key", json!({"key": "delete"}), T);
    h.step();
}

fn select(h: &mut Headless, names: &[&str]) {
    let ids: Vec<u64> = names.iter().map(|n| id(h, n)).collect();
    h.app.run("library.select", json!({"ids": ids})).unwrap();
    h.step();
}

#[test]
fn delete_key_in_recently_deleted_is_delete_permanently() {
    let (mut h, dir) = app("rd");
    h.app.run("photo.delete", json!({})).unwrap();
    h.app.run("library.source", json!({"kind": "recentlyDeleted"})).unwrap();
    h.app.run("library.selectAll", json!({})).unwrap();
    h.step();
    key_delete(&mut h);
    assert_eq!(h.app.ui.dialog, Some(Dialog::DeletePermanently { count: 3, local: 0, empty: false, files: "trash".into() }));
    h.app.ui.dialog = None;
    h.app.ui.settings.delete_files = DeleteFiles::Keep;
    key_delete(&mut h);
    assert_eq!(h.app.ui.dialog, None);
    assert!(h.app.session.catalog.is_empty(), "the photos are gone for good");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_key_outside_recently_deleted_still_asks_with_the_setting_off() {
    let (mut h, dir) = app("ask");
    assert!(!h.app.ui.settings.confirm_delete_picked_only);
    key_delete(&mut h);
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDelete { count: 3 }));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unflagged_and_rejected_unstarred_photos_skip_the_question() {
    let (mut h, dir) = app("skip");
    h.app.ui.settings.confirm_delete_picked_only = true;
    let b = id(&h, "b.png");
    h.app.run("photo.reject", json!({"ids": [b]})).unwrap();
    key_delete(&mut h);
    assert_eq!(h.app.ui.dialog, None, "no dialog");
    assert!(deleted(&h, "a.png") && deleted(&h, "b.png") && deleted(&h, "c.png"));
    let toast = h.app.ui.toast.clone().map(|t| t.0).unwrap_or_default();
    assert!(toast.starts_with("Moved 3 photos to Recently Deleted"), "{toast}");
    // Photo menu path too
    h.app.run("edit.undo", json!({})).unwrap();
    h.app.run("library.selectAll", json!({})).unwrap();
    crate::menubar::run_item(&mut h.app, "photo.delete", json!({})).unwrap();
    assert_eq!(h.app.ui.dialog, None);
    assert!(deleted(&h, "a.png"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_picked_or_starred_photo_in_the_selection_asks() {
    let (mut h, dir) = app("ask-picked");
    h.app.ui.settings.confirm_delete_picked_only = true;
    let a = id(&h, "a.png");
    h.app.run("photo.pick", json!({"ids": [a]})).unwrap();
    key_delete(&mut h);
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDelete { count: 3 }), "a picked photo asks");
    assert!(!deleted(&h, "a.png"));
    h.app.ui.dialog = None;
    h.app.run("photo.unflag", json!({"ids": [a]})).unwrap();
    // a rejected photo with a star counts as starred
    let c = id(&h, "c.png");
    h.app.run("photo.reject", json!({"ids": [c]})).unwrap();
    h.app.run("photo.rate", json!({"ids": [c], "rating": 2})).unwrap();
    select(&mut h, &["c.png"]);
    key_delete(&mut h);
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDelete { count: 1 }), "a starred photo asks");
    assert!(!deleted(&h, "c.png"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_rejected_photos_follows_the_same_rule() {
    let (mut h, dir) = app("rejected");
    h.app.ui.settings.confirm_delete_picked_only = true;
    let (a, b) = (id(&h, "a.png"), id(&h, "b.png"));
    h.app.run("photo.reject", json!({"ids": [a, b]})).unwrap();
    crate::menubar::run_item(&mut h.app, "photo.deleteRejected", json!({})).unwrap();
    assert_eq!(h.app.ui.dialog, None, "unstarred rejects go at once");
    assert!(deleted(&h, "a.png") && deleted(&h, "b.png") && !deleted(&h, "c.png"));
    h.app.run("edit.undo", json!({})).unwrap();
    h.app.run("photo.rate", json!({"ids": [b], "rating": 1})).unwrap();
    crate::menubar::run_item(&mut h.app, "photo.deleteRejected", json!({})).unwrap();
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDeleteRejected { count: 2 }), "a starred reject asks");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_checkbox_is_disabled_when_the_confirmation_is_off_and_the_setting_saves() {
    let (mut h, dir) = app("setting");
    h.app.ui.dialog = Some(Dialog::Settings { tab: "general".into() });
    h.step();
    h.step();
    h.settle(Duration::from_millis(600));
    let r = h.request("ui.clickWidget", json!({"id": "check:settings.confirmDeletePickedOnly"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.ui.settings.confirm_delete_picked_only, "enabled while the confirmation is on");
    h.app.ui.settings.confirm_delete = false;
    h.step();
    h.step();
    h.request("ui.clickWidget", json!({"id": "check:settings.confirmDeletePickedOnly"}), T);
    h.step();
    assert!(h.app.ui.settings.confirm_delete_picked_only, "a click does nothing while the confirmation is off");
    let ui = h.app.ui.clone();
    let back: crate::state::UiState = serde_json::from_str(&serde_json::to_string(&ui).unwrap()).unwrap();
    assert!(back.settings.confirm_delete_picked_only);
    assert!(!crate::state::UiState::default().settings.confirm_delete_picked_only);
    let _ = std::fs::remove_dir_all(&dir);
}
