use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::{LightcraftApp, Services};

type Opened = Arc<Mutex<Vec<(String, String)>>>;

/// A desktop-like app: a thread-safe writer that holds every write until `release` is set, and an
/// opener that records what it is given (no editor is started).
fn rig(tag: &str) -> (LightcraftApp, Opened, Arc<AtomicBool>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("lc-edit-in-task-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let opened: Opened = Default::default();
    let release = Arc::new(AtomicBool::new(false));
    let (o, rel) = (opened.clone(), release.clone());
    let services = Services {
        png: None,
        write_shared: Some(Arc::new(move |path: &str, bytes: &[u8]| {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !rel.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::fs::write(path, bytes).map_err(|e| e.to_string())
        })),
        open_with: Some(Box::new(move |path: &str, app: &str| {
            o.lock().unwrap().push((path.to_string(), app.to_string()));
            Ok(())
        })),
        ..Default::default()
    };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo().with_fs(), services);
    app.ui.settings.external_editor = "Editor".into();
    (app, opened, release, dir)
}

fn wait_for_task(app: &mut LightcraftApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while app.edit_in.is_some() {
        super::poll(app, ctx);
        assert!(Instant::now() < deadline, "Edit In never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Edit In from the window returns while the copy is still being written (the frame isn't held
/// up), with a row in the activity stack; when the copy is written it joins the library and
/// opens in the editor, and the row goes.
#[test]
fn edit_in_renders_in_the_background_with_an_activity_row() {
    let (mut app, opened, release, dir) = rig("bg");
    let n = app.session.catalog.len();
    let r = app.run("photo.editInExternal", json!({"dir": dir.to_string_lossy()})).unwrap();
    assert_eq!(r["background"], true, "{r}");
    assert!(app.edit_in.is_some());
    let rows = app.session.activity.list();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].kind, rows[0].total, rows[0].cancellable), ("editIn", 1, true));
    assert!(opened.lock().unwrap().is_empty(), "nothing opens before the copy is written");
    assert!(!crate::menus::ui_enabled(&app, "photo.editInExternal"), "one Edit In at a time");
    release.store(true, Ordering::SeqCst);
    let ctx = egui::Context::default();
    wait_for_task(&mut app, &ctx);
    let calls = opened.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(calls[0].0.ends_with("-Edit.tif") && std::path::Path::new(&calls[0].0).exists(), "{calls:?}");
    assert_eq!(calls[0].1, "Editor");
    assert_eq!(app.session.catalog.len(), n + 1, "the copy is in the library");
    assert!(app.session.activity.list().is_empty(), "the row goes with the task");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stack's ✕ stops Edit In: no copy stays on disk or in the library, nothing opens.
#[test]
fn a_cancelled_edit_in_leaves_no_copy() {
    let (mut app, opened, release, dir) = rig("cancel");
    let n = app.session.catalog.len();
    app.run("photo.editInExternal", json!({"dir": dir.to_string_lossy()})).unwrap();
    let rows = app.session.activity.list();
    app.session.activity.cancel(rows[0].id).unwrap();
    release.store(true, Ordering::SeqCst);
    let ctx = egui::Context::default();
    wait_for_task(&mut app, &ctx);
    assert!(opened.lock().unwrap().is_empty());
    assert_eq!(app.session.catalog.len(), n);
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `wait: true` (an agent, a script) still does it all in place and reports the copy.
#[test]
fn edit_in_with_wait_runs_in_place() {
    let (mut app, opened, release, dir) = rig("wait");
    release.store(true, Ordering::SeqCst);
    let r = app.run("photo.editInExternal", json!({"dir": dir.to_string_lossy(), "wait": true})).unwrap();
    assert!(r["path"].as_str().is_some_and(|p| p.ends_with("-Edit.tif")), "{r}");
    assert!(app.edit_in.is_none());
    assert_eq!(opened.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
