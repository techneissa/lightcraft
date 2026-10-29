//! Local photos (browsed from a folder, never in the library) always have their files moved to the
//! Trash when they are deleted for good, and Photo ▸ Delete Rejected Photos (`photo.deleteRejected`).
//! A fake [`Trasher`] stands in, so no test touches the real Trash.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;
use crate::file_trash::Trasher;

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
    let d = std::env::temp_dir().join(format!("lc-del-local-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for (i, n) in names.iter().enumerate() {
        let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, (i * 40) as u8, 255]);
        let b = crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() })
            .unwrap();
        std::fs::write(d.join(n), b).unwrap();
    }
    d
}

fn session(fake: &Arc<Fake>) -> Session {
    let mut s = Session::new().with_fs();
    s.set_trasher(fake.clone());
    s
}

fn browse(s: &mut Session, dir: &Path) {
    s.execute("library.browse", &json!({"path": dir.to_string_lossy()})).unwrap();
}

fn import(s: &mut Session, dir: &Path) {
    s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
}

fn id_of(s: &Session, name: &str) -> u64 {
    s.catalog.photos().find(|p| p.file_name == name).unwrap().id.0
}

fn deleted(s: &Session, name: &str) -> bool {
    s.catalog.photos().find(|p| p.file_name == name).unwrap().deleted
}

fn reject(s: &mut Session, names: &[&str]) {
    let ids: Vec<u64> = names.iter().map(|n| id_of(s, n)).collect();
    s.execute("photo.reject", &json!({"ids": ids})).unwrap();
}

#[test]
fn local_photos_go_to_the_trash_even_with_files_keep() {
    let dir = folder("local-keep", &["a.png"]);
    std::fs::write(dir.join("a.xmp"), "<x/>").unwrap();
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    browse(&mut s, &dir);
    assert!(s.catalog.photos().all(|p| p.local));
    let a = id_of(&s, "a.png");
    let r = s.execute("photo.deletePermanently", &json!({"ids": [a], "files": "keep"})).unwrap();
    assert_eq!(r["deleted"], 1, "{r}");
    assert_eq!(r["trashed"].as_array().unwrap().len(), 2, "the file and its sidecar: {r}");
    assert!(!dir.join("a.png").exists() && !dir.join("a.xmp").exists());
    assert!(s.catalog.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_photos_keep_their_files_with_files_keep() {
    let dir = folder("lib-keep", &["a.png"]);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    import(&mut s, &dir);
    assert!(s.catalog.photos().all(|p| !p.local));
    let a = id_of(&s, "a.png");
    let r = s.execute("photo.deletePermanently", &json!({"ids": [a], "files": "keep"})).unwrap();
    assert_eq!(r["deleted"], 1);
    assert!(r["trashed"].as_array().unwrap().is_empty());
    assert!(dir.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mixed_set_trashes_only_the_local_files_with_files_keep() {
    let lib = folder("mix-lib", &["lib.png"]);
    let loc = folder("mix-local", &["loc.png"]);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    import(&mut s, &lib);
    browse(&mut s, &loc);
    let ids = [id_of(&s, "lib.png"), id_of(&s, "loc.png")];
    s.execute("photo.delete", &json!({"ids": ids})).unwrap();
    s.execute("library.source", &json!({"kind": "recentlyDeleted"})).unwrap();
    let r = s.execute("library.emptyRecentlyDeleted", &json!({"files": "keep"})).unwrap();
    assert_eq!(r["deleted"], 2, "{r}");
    assert_eq!(r["trashed"].as_array().unwrap().len(), 1, "{r}");
    assert!(lib.join("lib.png").exists(), "the library photo's file stays");
    assert!(!loc.join("loc.png").exists(), "the Local photo's file went to the Trash");
    assert!(s.catalog.is_empty());
    let _ = std::fs::remove_dir_all(&lib);
    let _ = std::fs::remove_dir_all(&loc);
}

#[test]
fn a_local_file_that_cannot_be_trashed_keeps_its_photo() {
    struct Refuse;
    impl Trasher for Refuse {
        fn trash(&self, _: &Path) -> Result<(), String> {
            Err("the Trash is full".into())
        }
    }
    let dir = folder("local-fail", &["a.png"]);
    let mut s = Session::new().with_fs();
    s.set_trasher(Arc::new(Refuse));
    browse(&mut s, &dir);
    let a = id_of(&s, "a.png");
    let r = s.execute("photo.deletePermanently", &json!({"ids": [a], "files": "keep"})).unwrap();
    assert_eq!(r["deleted"], 0, "{r}");
    assert_eq!(r["failed"].as_array().unwrap().len(), 1, "{r}");
    assert!(s.catalog.photo(PhotoId(a)).is_some() && dir.join("a.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_rejected_takes_only_the_rejected_photos_of_the_view() {
    let a = folder("rej-a", &["a.png", "b.png", "c.png"]);
    let other = folder("rej-other", &["d.png"]);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    import(&mut s, &other);
    browse(&mut s, &a);
    reject(&mut s, &["a.png", "b.png", "d.png"]);
    // the view is the browsed folder: the rejected photo of the other folder is not in it
    let r = s.execute("photo.deleteRejected", &json!({})).unwrap();
    assert_eq!(r["changed"], 2, "{r}");
    assert!(deleted(&s, "a.png") && deleted(&s, "b.png"));
    assert!(!deleted(&s, "c.png") && !deleted(&s, "d.png"));
    // one undo step brings both back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(!deleted(&s, "a.png") && !deleted(&s, "b.png"));
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&other);
}

#[test]
fn delete_rejected_leaves_the_photos_the_filter_hides() {
    let a = folder("rej-filter", &["a.png", "b.png"]);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    browse(&mut s, &a);
    reject(&mut s, &["a.png", "b.png"]);
    s.execute("library.filter", &json!({"text": "a.png"})).unwrap();
    let r = s.execute("photo.deleteRejected", &json!({})).unwrap();
    assert_eq!(r["changed"], 1, "{r}");
    assert!(deleted(&s, "a.png") && !deleted(&s, "b.png"));
    let _ = std::fs::remove_dir_all(&a);
}

#[test]
fn delete_rejected_with_nothing_rejected_changes_nothing() {
    let a = folder("rej-none", &["a.png"]);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    browse(&mut s, &a);
    let r: Value = s.execute("photo.deleteRejected", &json!({})).unwrap();
    assert_eq!(r["changed"], 0, "{r}");
    assert!(!deleted(&s, "a.png"));
    let _ = std::fs::remove_dir_all(&a);
}

#[test]
fn delete_rejected_is_refused_in_recently_deleted() {
    let a = folder("rej-trash", &["a.png"]);
    let fake = Arc::new(Fake::default());
    let mut s = session(&fake);
    browse(&mut s, &a);
    reject(&mut s, &["a.png"]);
    s.execute("library.source", &json!({"kind": "recentlyDeleted"})).unwrap();
    assert!(s.execute("photo.deleteRejected", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&a);
}
