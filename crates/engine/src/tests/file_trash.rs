//! Delete Permanently with `files: "trash"`: which files go to the Trash, and which stay. A fake
//! [`Trasher`] stands in, so no test touches the real Trash.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::file_trash::Trasher;

/// Moves files into a folder of its own instead of the Trash; `fail` names files that refuse.
#[derive(Default)]
struct Fake {
    moved: Mutex<Vec<PathBuf>>,
    fail: Mutex<Vec<PathBuf>>,
}

impl Trasher for Fake {
    fn trash(&self, path: &Path) -> Result<(), String> {
        if self.fail.lock().unwrap().iter().any(|p| p == path) {
            return Err("the Trash is full".to_string());
        }
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        self.moved.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-file-trash-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn image(p: &Path, format: crate::export::ExportFormat, seed: u8) {
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
    let b = crate::export::encode_image(&img, &crate::export::ExportOptions { format, ..Default::default() }).unwrap();
    std::fs::write(p, b).unwrap();
}

fn png(p: &Path, seed: u8) {
    image(p, crate::export::ExportFormat::Png, seed);
}

fn jpeg(p: &Path, seed: u8) {
    image(p, crate::export::ExportFormat::Jpeg, seed);
}

fn session(fake: &Arc<Fake>) -> Session {
    let mut s = Session::new().with_fs();
    s.set_trasher(fake.clone());
    s
}

fn import(s: &mut Session, dir: &Path) -> Vec<u64> {
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    r["imported"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect()
}

fn id_of(s: &Session, name: &str) -> u64 {
    s.catalog.photos().find(|p| p.file_name == name).unwrap().id.0
}

fn paths(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect()
}

fn has(s: &Session, id: u64) -> bool {
    s.catalog.photo(PhotoId(id)).is_some()
}

#[test]
fn trash_moves_the_file_and_its_sidecar() {
    let dir = temp_dir("moves");
    png(&dir.join("a.png"), 1);
    std::fs::write(dir.join("a.xmp"), "<x/>").unwrap();
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    let ids = import(&mut s, &dir);
    let r = s.execute("photo.deletePermanently", &json!({"ids": ids, "files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 1, "{r}");
    assert_eq!(paths(&r["trashed"]).len(), 2, "{r}");
    assert!(r["failed"].as_array().unwrap().is_empty());
    assert!(r["note"].as_str().unwrap().contains("Trash"), "{r}");
    assert!(!dir.join("a.png").exists() && !dir.join("a.xmp").exists());
    assert!(s.catalog.is_empty());
    // undo brings back the photo, not the files
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), 1);
    assert!(!dir.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keep_is_the_default_and_leaves_the_files() {
    let dir = temp_dir("keep");
    png(&dir.join("a.png"), 1);
    png(&dir.join("b.png"), 2);
    std::fs::write(dir.join("a.xmp"), "<x/>").unwrap();
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    let ids = import(&mut s, &dir);
    let a = id_of(&s, "a.png");
    let r = s.execute("photo.deletePermanently", &json!({"ids": [a]})).unwrap();
    assert_eq!(r["deleted"], 1);
    assert!(r["trashed"].as_array().unwrap().is_empty());
    let r = s.execute("photo.deletePermanently", &json!({"ids": ids.into_iter().filter(|i| *i != a).collect::<Vec<_>>(), "files": "keep"})).unwrap();
    assert_eq!(r["deleted"], 1);
    assert!(dir.join("a.png").exists() && dir.join("a.xmp").exists() && dir.join("b.png").exists());
    assert!(fake.moved.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_files_value_is_refused() {
    let dir = temp_dir("bad");
    png(&dir.join("a.png"), 1);
    let mut s = session(&Arc::new(Fake::default()));
    let ids = import(&mut s, &dir);
    for bad in [json!("everything"), json!(3), json!(true)] {
        let e = s.execute("photo.deletePermanently", &json!({"ids": ids, "files": bad})).unwrap_err().to_string();
        assert!(e.contains("files"), "{e}");
    }
    assert_eq!(s.catalog.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_virtual_copys_shared_file_stays_while_the_master_stays() {
    let dir = temp_dir("virtual");
    png(&dir.join("a.png"), 1);
    std::fs::write(dir.join("a.xmp"), "<x/>").unwrap();
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    let master = import(&mut s, &dir)[0];
    let copy = s.execute("photo.virtualCopy", &json!({"ids": [master]})).unwrap()["ids"][0].as_u64().unwrap();
    // the copy goes, the master stays: its file and sidecar are not moved
    let r = s.execute("photo.deletePermanently", &json!({"ids": [copy], "files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 1, "{r}");
    assert!(r["trashed"].as_array().unwrap().is_empty(), "{r}");
    assert!(dir.join("a.png").exists() && dir.join("a.xmp").exists());
    assert!(has(&s, master) && !has(&s, copy));
    // with the master going too, the file goes
    let copy = s.execute("photo.virtualCopy", &json!({"ids": [master]})).unwrap()["ids"][0].as_u64().unwrap();
    let r = s.execute("photo.deletePermanently", &json!({"ids": [master, copy], "files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 2, "{r}");
    assert_eq!(paths(&r["trashed"]).len(), 2, "the file once, the sidecar once: {r}");
    assert!(!dir.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shared_stem_sidecar_stays_while_the_other_half_of_the_pair_stays() {
    let dir = temp_dir("pair");
    png(&dir.join("IMG_1.png"), 1);
    jpeg(&dir.join("IMG_1.jpg"), 2);
    std::fs::write(dir.join("IMG_1.xmp"), "<x/>").unwrap();
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    import(&mut s, &dir);
    let (png_id, jpg_id) = (id_of(&s, "IMG_1.png"), id_of(&s, "IMG_1.jpg"));
    let r = s.execute("photo.deletePermanently", &json!({"ids": [png_id], "files": "trash"})).unwrap();
    assert_eq!(paths(&r["trashed"]), vec![dir.join("IMG_1.png").to_string_lossy().to_string()], "{r}");
    assert!(dir.join("IMG_1.xmp").exists(), "the JPEG still shares it");
    assert!(dir.join("IMG_1.jpg").exists());
    let r = s.execute("photo.deletePermanently", &json!({"ids": [jpg_id], "files": "trash"})).unwrap();
    assert_eq!(paths(&r["trashed"]).len(), 2, "the last of the pair takes the sidecar: {r}");
    assert!(!dir.join("IMG_1.xmp").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_is_skipped_and_its_record_still_goes() {
    let dir = temp_dir("missing");
    png(&dir.join("a.png"), 1);
    png(&dir.join("b.png"), 2);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    let ids = import(&mut s, &dir);
    std::fs::remove_file(dir.join("a.png")).unwrap();
    let r = s.execute("photo.deletePermanently", &json!({"ids": ids, "files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 2, "{r}");
    assert_eq!(paths(&r["trashed"]), vec![dir.join("b.png").to_string_lossy().to_string()], "{r}");
    assert!(r["failed"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_cannot_be_trashed_is_reported_and_its_record_kept() {
    let dir = temp_dir("fails");
    png(&dir.join("a.png"), 1);
    png(&dir.join("b.png"), 2);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    let ids = import(&mut s, &dir);
    let (a, b) = (id_of(&s, "a.png"), id_of(&s, "b.png"));
    fake.fail.lock().unwrap().push(dir.join("a.png"));
    let r = s.execute("photo.deletePermanently", &json!({"ids": ids, "files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 1, "{r}");
    assert_eq!(r["failed"][0][0], dir.join("a.png").to_string_lossy().as_ref(), "{r}");
    assert_eq!(r["failed"][0][1], "the Trash is full");
    assert!(has(&s, a) && !has(&s, b), "the photo whose file stayed is still in the library");
    assert!(dir.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_recently_deleted_takes_files_too() {
    let dir = temp_dir("empty");
    png(&dir.join("a.png"), 1);
    png(&dir.join("b.png"), 2);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    import(&mut s, &dir);
    let a = id_of(&s, "a.png");
    s.execute("photo.delete", &json!({"ids": [a]})).unwrap();
    let r = s.execute("library.emptyRecentlyDeleted", &json!({"files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 1, "{r}");
    assert_eq!(paths(&r["trashed"]), vec![dir.join("a.png").to_string_lossy().to_string()]);
    assert!(!dir.join("a.png").exists() && dir.join("b.png").exists());
    assert_eq!(s.catalog.len(), 1);
    // with nothing in Recently Deleted: nothing happens, however it is asked
    let r = s.execute("library.emptyRecentlyDeleted", &json!({"files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 0);
    assert!(s.execute("library.emptyRecentlyDeleted", &json!({"files": "x"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_in_place_of_a_file_is_not_trashed() {
    let dir = temp_dir("folder");
    png(&dir.join("a.png"), 1);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    let ids = import(&mut s, &dir);
    std::fs::remove_file(dir.join("a.png")).unwrap();
    std::fs::create_dir(dir.join("a.png")).unwrap();
    let r = s.execute("photo.deletePermanently", &json!({"ids": ids, "files": "trash"})).unwrap();
    assert_eq!(r["deleted"], 0, "{r}");
    assert_eq!(r["failed"].as_array().unwrap().len(), 1, "{r}");
    assert!(dir.join("a.png").is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}
