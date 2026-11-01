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
    let moved = job.run_with(&fs);
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
    let moved = job.run_with(&fs);
    let err = s.finish_rename_with(&fs, &job, moved).unwrap_err().to_string();
    assert!(err.contains("/p/b.jpg"), "names the file: {err}");
    assert_eq!(fs.listing(), before, "the first file moved back");
    assert_eq!((path_of(&s, 1), path_of(&s, 2)), ("/p/a.jpg".to_string(), "/p/b.jpg".to_string()));
    assert!(s.undo.is_empty());
}

/// A folder rename on the real disk: the photos follow, in one undo step; a name that is taken is
/// refused by the disk step and the catalog stays.
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

    let job = s.begin_rename("folder.rename", &json!({"path": from, "name": "taken"})).unwrap();
    let moved = job.run();
    let err = s.finish_rename(&job, moved).unwrap_err().to_string();
    assert!(err.contains("already exists"), "{err}");
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
