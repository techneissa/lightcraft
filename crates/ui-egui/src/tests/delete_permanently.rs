//! Delete Permanently asks what to do with the photos' files (Settings ▸ General ▸ Delete
//! Permanently): Move to Trash, Keep Files, or Cancel. A fake Trash stands in.

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

/// Two imported photos in Recently Deleted, both selected.
fn app(tag: &str) -> (Headless, Arc<Fake>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-ui-delete-perm-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (seed, name) in [(60, "a.png"), (180, "b.png")] {
        let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[seed, 120, 160, 255]; 64] };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    let fake = Arc::new(Fake::default());
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.set_trasher(fake.clone());
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), [1400.0, 900.0], 1.0);
    h.step();
    let r = h.app.run("library.import", json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let ids = r["imported"].clone();
    h.app.run("photo.delete", json!({"ids": ids})).unwrap();
    h.app.run("library.source", json!({"kind": "recentlyDeleted"})).unwrap();
    h.app.run("library.selectAll", json!({})).unwrap();
    h.step();
    (h, fake, dir)
}

fn click(h: &mut Headless, id: &str) {
    h.settle(Duration::from_millis(600));
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
}

fn ask(h: &mut Headless, id: &str) {
    crate::menubar::run_item(&mut h.app, id, json!({})).unwrap();
}

#[test]
fn it_asks_by_default_and_keep_files_leaves_the_files() {
    let (mut h, fake, dir) = app("remove");
    assert_eq!(h.app.ui.settings.delete_files, DeleteFiles::Ask);
    ask(&mut h, "photo.deletePermanently");
    assert_eq!(h.app.ui.dialog, Some(Dialog::DeletePermanently { count: 2, local: 0, empty: false, files: "trash".into() }));
    assert_eq!(crate::panels::dialogs::delete_permanently::title(h.app.ui.dialog.as_ref().unwrap()), "Delete 2 Photos Permanently?");
    assert_eq!(h.app.session.catalog.len(), 2, "nothing happens before the answer");
    click(&mut h, "button:dialogKeepFiles");
    assert_eq!(h.app.ui.dialog, None);
    assert!(h.app.session.catalog.is_empty());
    assert!(dir.join("a.png").exists() && dir.join("b.png").exists());
    assert!(fake.0.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn move_to_trash_sends_the_files_and_says_so() {
    let (mut h, fake, dir) = app("trash");
    ask(&mut h, "photo.deletePermanently");
    click(&mut h, "button:dialogOk");
    assert_eq!(h.app.ui.dialog, None);
    assert!(h.app.session.catalog.is_empty());
    assert_eq!(fake.0.lock().unwrap().len(), 2);
    assert!(!dir.join("a.png").exists());
    let toast = h.app.ui.toast.clone().map(|t| t.0).unwrap_or_default();
    assert!(toast.contains("Moved 2 files to the Trash") && toast.contains("Undo"), "{toast}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancel_and_escape_change_nothing() {
    let (mut h, fake, dir) = app("cancel");
    ask(&mut h, "photo.deletePermanently");
    click(&mut h, "button:dialogCancel");
    assert_eq!(h.app.ui.dialog, None);
    ask(&mut h, "photo.deletePermanently");
    h.request("ui.key", json!({"key": "escape"}), T);
    h.step();
    assert_eq!(h.app.ui.dialog, None);
    assert_eq!(h.app.session.catalog.len(), 2);
    assert!(dir.join("a.png").exists() && fake.0.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_recently_deleted_asks_too() {
    let (mut h, fake, dir) = app("empty");
    ask(&mut h, "library.emptyRecentlyDeleted");
    assert_eq!(h.app.ui.dialog, Some(Dialog::DeletePermanently { count: 2, local: 0, empty: true, files: "trash".into() }));
    click(&mut h, "button:dialogOk");
    assert!(h.app.session.catalog.is_empty());
    assert_eq!(fake.0.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fixed_setting_does_not_ask() {
    let (mut h, fake, dir) = app("fixed");
    h.app.ui.settings.delete_files = DeleteFiles::Keep;
    ask(&mut h, "photo.deletePermanently");
    assert_eq!(h.app.ui.dialog, None);
    assert!(h.app.session.catalog.is_empty());
    assert!(dir.join("a.png").exists() && fake.0.lock().unwrap().is_empty());

    let (mut h, fake, dir2) = app("fixed-trash");
    h.app.ui.settings.delete_files = DeleteFiles::Trash;
    ask(&mut h, "photo.deletePermanently");
    assert_eq!(h.app.ui.dialog, None);
    assert_eq!(fake.0.lock().unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn the_setting_is_saved_and_a_bad_value_reads_as_ask() {
    let s: crate::state::AppSettings = serde_json::from_value(json!({"deleteFiles": "trash"})).unwrap();
    assert_eq!(s.delete_files, DeleteFiles::Trash);
    let s: crate::state::AppSettings = serde_json::from_value(json!({"deleteFiles": "burn"})).unwrap();
    assert_eq!(s.delete_files, DeleteFiles::Ask);
    let s: crate::state::AppSettings = serde_json::from_value(json!({})).unwrap();
    assert_eq!(s.delete_files, DeleteFiles::Ask);
    assert_eq!(serde_json::to_value(DeleteFiles::Keep).unwrap(), json!("keep"));
}
