//! The three steps of a rename (`begin_rename`, `RenameJob::run`, `finish_rename`): the same result as
//! the command, one undo step, and an unchanged catalog when the disk says no.

use std::path::Path;

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use super::*;
use crate::rename::tests::FakeFs;

fn session_with(photos: &[(u64, &str)]) -> Session {
    let mut s = Session::new();
    for (id, path) in photos {
        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let photo = Photo::new(PhotoId(*id), Source::File { path: (*path).into() }, &name, "JPG", 1, 1, "2026-01-01T00:00:00");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    s
}

fn path_of(s: &Session, id: u64) -> String {
    match &s.catalog.photo(PhotoId(id)).unwrap().source {
        Source::File { path } => path.clone(),
        other => panic!("{other:?}"),
    }
}

/// The disk work needs no session: the job can go to a worker thread.
#[test]
fn a_job_can_go_to_a_worker() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<RenameJob>();
    send_sync::<Moved>();
}

/// Given two photos, when their rename runs as begin, run and finish, then the files and the catalog
/// have the new names, the sidecar went along, and one undo step brings the old names back.
#[test]
fn a_rename_in_three_steps_is_one_undo_step() {
    let fs = FakeFs::new(false, &[("/p/a.jpg", "A"), ("/p/a.xmp", "edits"), ("/p/b.jpg", "B")]);
    let mut s = session_with(&[(1, "/p/a.jpg"), (2, "/p/b.jpg")]);
    let params = json!({"template": "trip-{seq}", "ids": [1, 2]});
    let job = s.begin_rename_with(&fs, "photo.rename", &params).unwrap();
    assert_eq!((job.command(), job.total()), ("photo.rename", 2));
    assert_eq!(path_of(&s, 1), "/p/a.jpg", "nothing changes before the files have moved");
    let moved = job.run_on(&fs, &|_, _| {});
    assert_eq!(path_of(&s, 1), "/p/a.jpg", "nor before the catalog is told");
    let r = s.finish_rename_with(&fs, &job, moved).unwrap();
    assert_eq!(r["renamed"], 2);
    assert_eq!((path_of(&s, 1), path_of(&s, 2)), ("/p/trip-1.jpg".to_string(), "/p/trip-2.jpg".to_string()));
    assert_eq!(
        fs.listing(),
        vec![("/p/trip-1.jpg".into(), "A".into()), ("/p/trip-1.xmp".into(), "edits".into()), ("/p/trip-2.jpg".into(), "B".into())]
    );
    assert_eq!(s.undo.len(), 1, "one undo step");
}

/// Given a disk that refuses the second file, when the rename finishes, then it says why, the first file
/// is back, the catalog keeps the old names and nothing was added to the undo history.
#[test]
fn a_move_that_fails_leaves_the_catalog_and_the_files_as_they_were() {
    let fs = FakeFs::new(false, &[("/p/a.jpg", "A"), ("/p/b.jpg", "B")]);
    fs.fail.borrow_mut().push("/p/b.jpg".into());
    let mut s = session_with(&[(1, "/p/a.jpg"), (2, "/p/b.jpg")]);
    let before = fs.listing();
    let job = s.begin_rename_with(&fs, "photo.rename", &json!({"template": "x-{seq}", "ids": [1, 2]})).unwrap();
    let moved = job.run_on(&fs, &|_, _| {});
    let err = s.finish_rename_with(&fs, &job, moved).unwrap_err().to_string();
    assert!(err.contains("/p/b.jpg"), "names the file: {err}");
    assert_eq!(fs.listing(), before, "the first file moved back");
    assert_eq!((path_of(&s, 1), path_of(&s, 2)), ("/p/a.jpg".to_string(), "/p/b.jpg".to_string()));
    assert!(s.undo.is_empty());
}

/// A folder rename on the real disk: the photos follow, in one undo step; a name that is taken is
/// refused when the rename begins and the catalog stays.
#[test]
fn a_folder_rename_in_three_steps_and_a_taken_name() {
    let dir = std::env::temp_dir().join(format!("lc-rename-job-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (old, taken) = (dir.join("old"), dir.join("taken"));
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&taken).unwrap();
    std::fs::write(old.join("a.jpg"), b"A").unwrap();
    let a = old.join("a.jpg").to_string_lossy().to_string();
    let mut s = session_with(&[(1, &a)]);
    let from = old.to_string_lossy().to_string();

    let err = s.begin_rename("folder.rename", &json!({"path": from, "name": "taken"})).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("already exists"), "refused before anything runs: {err}");
    assert_eq!(path_of(&s, 1), a, "the catalog is unchanged");
    assert!(s.undo.is_empty());

    let job = s.begin_rename("folder.rename", &json!({"path": from, "name": "new"})).unwrap();
    let moved = job.run();
    let r = s.finish_rename(&job, moved).unwrap();
    assert_eq!(r["relinked"], 1);
    let renamed = dir.join("new").join("a.jpg");
    assert_eq!(Path::new(&path_of(&s, 1)), renamed);
    assert!(renamed.exists() && !old.exists());
    assert_eq!(s.undo.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Only the two rename commands run as jobs.
#[test]
fn other_commands_are_not_jobs() {
    let mut s = Session::new();
    assert!(s.begin_rename("album.rename", &json!({})).is_err());
}

/// A folder name with a character a file name can't have is refused with that character named.
#[test]
fn a_folder_name_with_a_forbidden_character_is_refused_naming_it() {
    let dir = std::env::temp_dir().join(format!("lc-rename-chars-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("old")).unwrap();
    let from = dir.join("old").to_string_lossy().to_string();
    let mut s = Session::new();
    for (name, shown) in [("a:b", "“:”"), ("a*b", "“*”"), ("a\u{7}b", "control")] {
        let e = s.execute("folder.rename", &json!({"path": from, "name": name})).unwrap_err().to_string();
        assert!(e.contains(shown), "{name:?}: {e}");
        let e = s.begin_rename("folder.rename", &json!({"path": from, "name": name})).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains(shown), "{name:?}: {e}");
    }
    assert!(dir.join("old").exists(), "nothing was renamed");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A template with a character a file name can't have, or no template, is refused before any file moves
/// (`Trip: {seq}` used to become `Trip- 1.jpg`, an empty template renamed nothing without a word).
#[test]
fn a_template_that_would_be_rewritten_or_is_empty_is_refused() {
    let fs = FakeFs::new(false, &[("/p/a.jpg", "A")]);
    let mut s = session_with(&[(1, "/p/a.jpg")]);
    s.execute("library.select", &json!({"ids": [1]})).unwrap();
    for (template, shown) in [("Trip: {seq}", "“:”"), ("a|b", "“|”"), ("a?b", "“?”"), ("x/y", "“/”"), ("a\u{1}b", "control"), ("  ", "empty")]
    {
        let params = json!({"template": template, "ids": [1]});
        let e = s.begin_rename_with(&fs, "photo.rename", &params).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains(shown), "{template:?}: {e}");
        let e = s.execute("photo.rename", &params).unwrap_err().to_string();
        assert!(e.contains(shown), "{template:?}: {e}");
        assert!(s.execute("photo.renamePreview", &params).is_err(), "the preview says so too");
    }
    // a colon inside a token's braces is the token's own
    s.begin_rename_with(&fs, "photo.rename", &json!({"template": "{date:%Y%m%d}-{seq:3}", "ids": [1]})).unwrap();
    assert_eq!(fs.listing(), vec![("/p/a.jpg".to_string(), "A".to_string())]);
}

/// A job whose worker never answered can tell whether anything moved: every file the library points at is
/// still there, or not.
#[test]
fn a_job_knows_whether_its_files_are_still_in_place() {
    let dir = std::env::temp_dir().join(format!("lc-rename-inplace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.jpg");
    std::fs::write(&a, b"A").unwrap();
    let mut s = session_with(&[(1, &a.to_string_lossy())]);
    let job = s.begin_rename("photo.rename", &json!({"template": "x", "ids": [1]})).unwrap();
    assert!(job.files_in_place());
    std::fs::remove_file(&a).unwrap();
    assert!(!job.files_in_place(), "a file the library points at is gone");
    s.abandon_rename();
    let _ = std::fs::remove_dir_all(&dir);
}
