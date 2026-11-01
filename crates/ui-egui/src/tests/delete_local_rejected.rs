//! Local photos (browsed from a folder) always have their files moved to the Trash when deleted
//! for good, so their dialog is a plain Move to Trash question; library photos get Keep Files; and
//! Photo ▸ Delete Rejected Photos. A fake Trash stands in.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lightcraft_engine::file_trash::Trasher;
use serde_json::json;

use crate::headless::Headless;
use crate::state::{DeleteFiles, Dialog};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Fake(Mutex<Vec<PathBuf>>);

impl Trasher for Fake {
    fn trash(&self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        self.0.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
}

fn folder(tag: &str, names: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-ui-del-local-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (i, name) in names.iter().enumerate() {
        let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[(i * 50) as u8, 120, 160, 255]; 64] };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    dir
}

fn host() -> (Headless, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.set_trasher(fake.clone());
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), [1400.0, 900.0], 1.0);
    h.step();
    (h, fake)
}

fn photos(h: &Headless) -> Vec<u64> {
    let mut v: Vec<_> = h.app.session.catalog.photos().map(|p| p.id.0).collect();
    v.sort();
    v
}

/// Delete every photo, then show Recently Deleted with all of them selected.
fn to_recently_deleted(h: &mut Headless) {
    let ids = photos(h);
    let all: Vec<_> = ids.iter().map(|i| lightcraft_engine::catalog::PhotoId(*i)).collect();
    h.app.session.selection = lightcraft_engine::Selection { active: all.first().copied(), ids: all };
    h.app.run("photo.delete", json!({"ids": ids})).unwrap();
    h.app.run("library.source", json!({"kind": "recentlyDeleted"})).unwrap();
    h.app.run("library.selectAll", json!({})).unwrap();
    h.step();
}

/// Browse a folder in Local (a background job in the app: wait for it).
fn browse(h: &mut Headless, dir: &Path) {
    let r = h.request("engine.execute", json!({"command": "library.browse", "params": {"path": dir.to_string_lossy()}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(2));
    h.step();
}

fn click(h: &mut Headless, id: &str) -> serde_json::Value {
    h.settle(Duration::from_millis(600));
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    h.step();
    r
}

fn ask(h: &mut Headless, id: &str) {
    crate::menubar::run_item(&mut h.app, id, json!({})).unwrap();
}

fn title(h: &Headless) -> String {
    crate::panels::dialogs::delete_permanently::title(h.app.ui.dialog.as_ref().unwrap())
}

#[test]
fn only_local_photos_get_a_plain_move_to_trash_question() {
    let (mut h, fake) = host();
    let dir = folder("local", &["a.png", "b.png"]);
    browse(&mut h, &dir);
    to_recently_deleted(&mut h);
    ask(&mut h, "photo.deletePermanently");
    assert_eq!(h.app.ui.dialog, Some(Dialog::DeletePermanently { count: 2, local: 2, empty: false, files: "trash".into() }));
    assert_eq!(title(&h), "Move 2 Photos to the Trash?");
    assert_ne!(click(&mut h, "button:dialogKeepFiles")["ok"], true, "no Keep Files button for Local photos");
    assert!(h.app.ui.dialog.is_some());
    assert_eq!(click(&mut h, "button:dialogOk")["ok"], true);
    assert_eq!(h.app.ui.dialog, None);
    assert!(h.app.session.catalog.is_empty());
    assert_eq!(fake.0.lock().unwrap().len(), 2);
    assert!(!dir.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_photos_get_keep_files() {
    let (mut h, fake) = host();
    let dir = folder("lib", &["a.png", "b.png"]);
    h.app.run("library.import", json!({"paths": [dir.to_string_lossy()]})).unwrap();
    to_recently_deleted(&mut h);
    ask(&mut h, "library.emptyRecentlyDeleted");
    assert_eq!(h.app.ui.dialog, Some(Dialog::DeletePermanently { count: 2, local: 0, empty: true, files: "trash".into() }));
    assert_eq!(title(&h), "Delete 2 Photos Permanently?");
    assert_eq!(click(&mut h, "button:dialogKeepFiles")["ok"], true);
    assert!(h.app.session.catalog.is_empty());
    assert!(dir.join("a.png").exists() && fake.0.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mixed_set_keeps_the_library_files_and_trashes_the_local_ones() {
    let (mut h, fake) = host();
    let lib = folder("mix-lib", &["lib.png"]);
    let loc = folder("mix-loc", &["loc.png"]);
    h.app.run("library.import", json!({"paths": [lib.to_string_lossy()]})).unwrap();
    browse(&mut h, &loc);
    to_recently_deleted(&mut h);
    ask(&mut h, "photo.deletePermanently");
    assert_eq!(h.app.ui.dialog, Some(Dialog::DeletePermanently { count: 2, local: 1, empty: false, files: "trash".into() }));
    assert_eq!(click(&mut h, "button:dialogKeepFiles")["ok"], true);
    assert!(h.app.session.catalog.is_empty());
    assert!(lib.join("lib.png").exists());
    assert!(!loc.join("loc.png").exists());
    assert_eq!(fake.0.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&lib);
    let _ = std::fs::remove_dir_all(&loc);
}

#[test]
fn a_fixed_keep_files_setting_still_trashes_local_files() {
    let (mut h, fake) = host();
    let dir = folder("fixed", &["a.png"]);
    browse(&mut h, &dir);
    to_recently_deleted(&mut h);
    h.app.ui.settings.delete_files = DeleteFiles::Keep;
    ask(&mut h, "photo.deletePermanently");
    assert_eq!(h.app.ui.dialog, None);
    assert_eq!(fake.0.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_setting_names_its_third_choice_keep_files() {
    let (mut h, _) = host();
    h.app.ui.dialog = Some(Dialog::Settings { tab: "general".into() });
    h.step();
    h.step();
    let r = click(&mut h, "button:settingsDeleteFiles-2");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.settings.delete_files, DeleteFiles::Keep);
    // (saved settings still use "keep")
    assert_eq!(serde_json::to_value(DeleteFiles::Keep).unwrap(), json!("keep"));
}

fn menu_item(h: &Headless) -> (bool, Option<String>) {
    for (name, nodes) in crate::menubar::menu_bar(&h.app) {
        if name != "Photo" {
            continue;
        }
        for n in nodes {
            if let crate::menubar::MenuNode::Item { id, enabled, shortcut, label, .. } = n
                && id == "photo.deleteRejected"
            {
                assert_eq!(label, "Delete Rejected Photos");
                return (enabled, shortcut);
            }
        }
    }
    panic!("Photo ▸ Delete Rejected Photos is not in the menu");
}

#[test]
fn delete_rejected_photos_menu_item_follows_the_view() {
    let (mut h, _) = host();
    let dir = folder("menu", &["a.png", "b.png"]);
    browse(&mut h, &dir);
    h.step();
    let (enabled, shortcut) = menu_item(&h);
    assert!(!enabled, "nothing is rejected");
    assert_eq!(shortcut.as_deref(), Some("Cmd+Delete"));
    let a = h.app.session.catalog.photos().find(|p| p.file_name == "a.png").unwrap().id.0;
    h.app.run("photo.reject", json!({"ids": [a]})).unwrap();
    h.step();
    assert!(menu_item(&h).0, "a rejected photo is in view");
    // with the confirmation on (the default) it asks, with the count
    assert!(h.app.ui.settings.confirm_delete);
    ask(&mut h, "photo.deleteRejected");
    assert_eq!(h.app.ui.dialog, Some(Dialog::ConfirmDeleteRejected { count: 1 }));
    assert!(h.app.session.catalog.photos().all(|p| !p.deleted), "nothing happens before the answer");
    assert_eq!(click(&mut h, "button:dialogOk")["ok"], true);
    assert!(h.app.session.catalog.photos().find(|p| p.id.0 == a).unwrap().deleted);
    // with the confirmation off it just runs
    h.app.run("edit.undo", json!({})).unwrap();
    h.app.ui.settings.confirm_delete = false;
    ask(&mut h, "photo.deleteRejected");
    assert_eq!(h.app.ui.dialog, None);
    assert!(h.app.session.catalog.photos().find(|p| p.id.0 == a).unwrap().deleted);
    let _ = std::fs::remove_dir_all(&dir);
}
