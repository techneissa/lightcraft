//! Renaming, one way everywhere: a list row turns into a text field (Return or a click away saves, Esc
//! cancels, an empty or used name is refused with the reason, a keyword that exists asks before it
//! merges), and Rename Photos / Rename Folder move their files on a worker thread without
//! changing the catalog when the disk says no.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::inline_rename::Kind;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn demo() -> Headless {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1400.0, 1300.0], 1.0);
    let r = h.request("ui.set", json!({"view": "photoGrid", "leftPanel": true, "sidebar": ["albums", "keywords"]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    h
}

fn album(h: &mut Headless, name: &str) -> u64 {
    let r = h.request("engine.execute", json!({"command": "album.create", "params": {"name": name}}), T);
    let id = r["result"]["id"].as_u64().unwrap_or_else(|| panic!("{r}"));
    h.settle(T);
    id
}

fn album_name(h: &Headless, id: u64) -> String {
    h.app.session.catalog.album(lightcraft_catalog::AlbumId(id)).unwrap().name.clone()
}

fn has_widget(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn double_click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id, "count": 2}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(T);
}

/// Replace what the focused field holds with `text`.
fn type_name(h: &mut Headless, text: &str) {
    h.request("ui.key", json!({"key": "A", "cmd": true}), T);
    h.request("ui.text", json!({"text": text}), T);
}

fn key(h: &mut Headless, key: &str) {
    let r = h.request("ui.key", json!({"key": key}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

/// Given an album row, when it is double-clicked, then its name is a field on the row (no window opens);
/// typing a name and Return saves it, and the field is gone.
#[test]
fn an_album_is_renamed_in_its_row() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    assert!(matches!(&h.app.ui.rename_row, Some(r) if r.kind == Kind::Album(id)));
    assert!(h.app.ui.dialog.is_none(), "no window");
    assert!(has_widget(&h, &format!("field:rename:album:{id}")), "the field is on the row");
    type_name(&mut h, "  Holiday ");
    key(&mut h, "Enter");
    assert!(h.app.ui.rename_row.is_none());
    assert_eq!(album_name(&h, id), "Holiday", "saved, trimmed");
    assert!(!has_widget(&h, &format!("field:rename:album:{id}")));
}

/// Esc puts the old name back and leaves the album alone.
#[test]
fn escape_cancels_a_rename() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    type_name(&mut h, "Holiday");
    key(&mut h, "Escape");
    assert!(h.app.ui.rename_row.is_none());
    assert_eq!(album_name(&h, id), "Trip");
}

/// A click on another row ends the edit and saves.
#[test]
fn a_click_away_saves() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    type_name(&mut h, "Holiday");
    let r = h.request("ui.clickWidget", json!({"id": "source:all"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(h.app.ui.rename_row.is_none());
    assert_eq!(album_name(&h, id), "Holiday");
}

/// Given a name another album has (the demo has a Garden), when Return is pressed, then the field stays
/// open with the reason under it and the album keeps its name; changing the text clears the reason.
#[test]
fn a_used_name_is_refused_on_the_field_with_the_reason() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    type_name(&mut h, "garden");
    key(&mut h, "Enter");
    assert!(h.app.ui.rename_row.is_some(), "the field stays");
    assert_eq!(album_name(&h, id), "Trip");
    assert!(has_widget(&h, &format!("renameReason:field:rename:album:{id}")), "the reason is shown");
    type_name(&mut h, "Garden 2");
    key(&mut h, "Enter");
    assert!(h.app.ui.rename_row.is_none());
    assert_eq!(album_name(&h, id), "Garden 2");
}

/// An empty name is refused the same way.
#[test]
fn an_empty_name_is_refused_on_the_field() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    type_name(&mut h, "   ");
    key(&mut h, "Enter");
    assert!(h.app.ui.rename_row.is_some());
    assert!(has_widget(&h, &format!("renameReason:field:rename:album:{id}")));
    assert_eq!(album_name(&h, id), "Trip");
}

/// A click away with a name that can't be used puts the old name back and says why.
#[test]
fn a_click_away_with_a_used_name_says_why_and_changes_nothing() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    type_name(&mut h, "Garden");
    let r = h.request("ui.clickWidget", json!({"id": "source:all"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(T);
    assert!(h.app.ui.rename_row.is_none());
    assert_eq!(album_name(&h, id), "Trip");
    assert!(toast(&h).contains("already used"), "{}", toast(&h));
}

/// Typing in the field does not run shortcuts: a letter that is a shortcut stays text.
#[test]
fn keys_typed_in_the_field_are_text_not_shortcuts() {
    let mut h = demo();
    let id = album(&mut h, "Trip");
    double_click(&mut h, &format!("source:album:{id}"));
    let steps = h.app.session.undo.len();
    // P, X and U flag a photo, B adds it to the target album, a digit rates it
    type_name(&mut h, "PXUB5");
    key(&mut h, "Enter");
    assert_eq!(album_name(&h, id), "PXUB5");
    assert_eq!(h.app.session.undo.len(), steps + 1, "the rename is the only step: no shortcut ran");
}

/// A keyword is renamed in its row; one that does not exist yet is just renamed.
#[test]
fn a_keyword_is_renamed_in_its_row() {
    let mut h = demo();
    assert!(h.app.session.catalog.has_keyword("beach"));
    double_click(&mut h, "source:keyword:beach");
    assert!(matches!(&h.app.ui.rename_row, Some(r) if r.kind == Kind::Keyword("beach".into())));
    type_name(&mut h, "seaside");
    key(&mut h, "Enter");
    assert!(h.app.ui.dialog.is_none(), "nothing to ask");
    assert!(h.app.session.catalog.has_keyword("seaside") && !h.app.session.catalog.has_keyword("beach"));
}

/// Given a keyword that exists, when another is renamed to it, then the window asks before merging
/// and nothing changes until it is confirmed.
#[test]
fn renaming_a_keyword_onto_another_asks_before_merging() {
    let mut h = demo();
    assert!(h.app.session.catalog.has_keyword("aurora") && h.app.session.catalog.has_keyword("beach"));
    double_click(&mut h, "source:keyword:beach");
    type_name(&mut h, "aurora");
    key(&mut h, "Enter");
    assert!(matches!(&h.app.ui.dialog, Some(Dialog::RenameKeyword { from, to }) if from == "beach" && to == "aurora"), "{:?}", h.app.ui.dialog);
    assert!(h.app.session.catalog.has_keyword("beach"), "not merged yet");
    h.settle(T);
    assert!(has_widget(&h, "text:keywordMerge"), "the question is on screen");
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!h.app.session.catalog.has_keyword("beach"), "merged once confirmed");
}

/// Presets and versions answer for names the same way as albums.
#[test]
fn presets_and_versions_use_the_same_rule() {
    let mut h = demo();
    let a = h.app.run("preset.create", json!({"name": "Warm", "group": "Mine"})).unwrap()["id"].as_str().unwrap().to_string();
    let b = h.app.run("preset.create", json!({"name": "Cool", "group": "Mine"})).unwrap()["id"].as_str().unwrap().to_string();
    let check = |h: &Headless, kind: &Kind, text: &str| crate::inline_rename::check(&h.app, kind, text);
    assert_eq!(check(&h, &Kind::Preset(b.clone()), " warm ").unwrap_err(), "“Warm” is already used");
    assert_eq!(check(&h, &Kind::Preset(b.clone()), "Cooler"), Ok("Cooler".into()));
    assert!(check(&h, &Kind::Preset(a), "").is_err());
    h.app.run("version.create", json!({"name": "A"})).unwrap();
    h.app.run("version.create", json!({"name": "B"})).unwrap();
    assert_eq!(check(&h, &Kind::Version(1), "a").unwrap_err(), "“A” is already used");
    assert_eq!(check(&h, &Kind::Version(1), "B"), Ok("B".into()), "its own name is fine");
}

// ---- Rename Photos and Rename Folder on a worker

fn files_app(tag: &str) -> (Headless, PathBuf, Vec<u64>) {
    use lightcraft_catalog::{Op, Photo, Source};
    let root = std::env::temp_dir().join(format!("lc-rename-task-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut session = lightcraft_engine::Session::new().with_fs();
    let mut ids = Vec::new();
    for file in ["beach.png", "garden.png"] {
        let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[90, 120, 160, 255]; 64] };
        let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        std::fs::write(root.join(file), png).unwrap();
        let id = session.catalog.alloc_photo_id();
        let p = Photo::new(id, Source::File { path: root.join(file).to_string_lossy().into() }, file, "PNG", 8, 8, "2026-01-01T10:00:00");
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        ids.push(id.0);
    }
    let h = Headless::new(LightcraftApp::new(session, Services { png: None, ..Default::default() }), [1300.0, 900.0], 1.0);
    (h, root, ids)
}

fn finish_task(h: &mut Headless) {
    for _ in 0..400 {
        h.step();
        if h.app.rename_task.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the rename never finished");
}

fn path_of(h: &Headless, id: u64) -> PathBuf {
    match &h.app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).unwrap().source {
        lightcraft_catalog::Source::File { path } => PathBuf::from(path),
        other => panic!("{other:?}"),
    }
}

/// Given two photos, when Rename Photos runs in the background, then the command answers at once with a
/// row in the activity stack, and when the files have moved the catalog follows in one undo step.
#[test]
fn rename_photos_runs_on_a_worker_and_is_one_undo_step() {
    let (mut h, root, ids) = files_app("photos");
    let r = h.app.run("photo.rename", json!({"template": "trip-{seq}", "ids": ids, "background": true})).unwrap();
    assert_eq!(r["background"], true);
    assert!(
        h.app.rename_task.is_some() && h.app.session.activity.list().iter().any(|t| t.kind == "rename" && t.total == 2),
        "a row counting the files"
    );
    assert!(h.app.run("photo.rename", json!({"template": "x", "ids": ids, "background": true})).is_err(), "one rename at a time");
    finish_task(&mut h);
    assert_eq!((path_of(&h, ids[0]), path_of(&h, ids[1])), (root.join("trip-1.png"), root.join("trip-2.png")));
    assert!(root.join("trip-1.png").exists() && !root.join("beach.png").exists());
    assert!(toast(&h).contains("Renamed 2 photos"), "{}", toast(&h));
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(path_of(&h, ids[0]), root.join("beach.png"));
    assert!(root.join("beach.png").exists() && !root.join("trip-1.png").exists(), "undo moved the files back");
    let _ = std::fs::remove_dir_all(&root);
}

/// Given a folder the disk won't let files out of, when Rename Photos runs, then the catalog keeps the old
/// names and the toast says why.
#[cfg(unix)]
#[test]
fn a_rename_the_disk_refuses_changes_nothing_and_says_why() {
    use std::os::unix::fs::PermissionsExt;
    let (mut h, root, ids) = files_app("refused");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
    // (a process that may write anywhere, such as root, can't be refused: nothing to test then)
    let writable = std::fs::write(root.join("probe"), b"x").is_ok();
    if !writable {
        let steps = h.app.session.undo.len();
        h.app.run("photo.rename", json!({"template": "trip-{seq}", "ids": ids, "background": true})).unwrap();
        finish_task(&mut h);
        assert_eq!((path_of(&h, ids[0]), path_of(&h, ids[1])), (root.join("beach.png"), root.join("garden.png")), "the catalog is unchanged");
        assert!(toast(&h).starts_with("Not renamed"), "{}", toast(&h));
        assert_eq!(h.app.session.undo.len(), steps, "no undo step");
    }
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(root.join("beach.png").exists() && root.join("garden.png").exists(), "no file is lost");
    let _ = std::fs::remove_dir_all(&root);
}

/// Rename Folder runs the same way: the photos follow, in one undo step.
#[test]
fn rename_folder_runs_on_a_worker() {
    let (mut h, root, ids) = files_app("folder");
    let r = h.app.run("folder.rename", json!({"path": root.to_string_lossy(), "name": "lc-renamed-folder", "background": true})).unwrap();
    assert_eq!(r["background"], true);
    finish_task(&mut h);
    let renamed = root.with_file_name("lc-renamed-folder");
    assert_eq!(path_of(&h, ids[0]), renamed.join("beach.png"));
    assert!(renamed.join("garden.png").exists() && !root.exists());
    assert!(toast(&h).contains("Folder renamed"), "{}", toast(&h));
    h.app.run("edit.undo", json!({})).unwrap();
    assert!(root.join("garden.png").exists() && !renamed.exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// While files move, an export is refused with the reason (it would read them from under the rename), and
/// when the rename is over the export is allowed again.
#[test]
fn no_export_while_files_move() {
    let (mut h, root, ids) = files_app("export");
    h.app.run("photo.rename", json!({"template": "trip-{seq}", "ids": ids, "background": true})).unwrap();
    let e = h.app.run("app.export", json!({"preset": "none", "background": true})).unwrap_err();
    assert!(e.contains("Wait for the rename to finish"), "{e}");
    finish_task(&mut h);
    let e = h.app.run("app.export", json!({"preset": "none", "background": true})).unwrap_err();
    assert!(!e.contains("Wait for the rename"), "{e}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The Rename Photos window says why a template can't be used, instead of showing a preview of names that
/// would be rewritten, and the preview comes back with a good one.
#[test]
fn the_rename_window_says_why_a_template_is_refused() {
    let mut h = demo();
    h.app.run("library.selectAll", json!({})).unwrap();
    h.app.ui.dialog = Some(Dialog::Rename { template: "Trip: {seq}".into(), start: 1 });
    h.settle(T);
    assert!(has_widget(&h, "text:renameTemplateProblem"));
    h.app.ui.dialog = Some(Dialog::Rename { template: "Trip-{seq}".into(), start: 1 });
    h.settle(T);
    assert!(!has_widget(&h, "text:renameTemplateProblem"));
}
