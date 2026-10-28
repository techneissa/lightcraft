use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(60);

type Opened = Arc<Mutex<Vec<(String, String)>>>;

struct Rig {
    h: Headless,
    opened: Opened,
    dir: std::path::PathBuf,
    /// The first photos of the demo library by id, all selected, the first active.
    ids: Vec<u64>,
}

/// The demo library's first `n` photos, all selected (the first active), an injected opener that
/// records what it is given (PhotoCraft itself is never started), and a PhotoCraft that is
/// installed or not and does or doesn't open layer lists.
fn rig(tag: &str, n: usize, supports: bool, installed: bool) -> Rig {
    let dir = std::env::temp_dir().join(format!("lc-layers-ui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = lightcraft_engine::Session::with_demo();
    let mut ids: Vec<u64> = session.catalog.photos().map(|p| p.id.0).collect();
    ids.sort();
    ids.truncate(n);
    assert_eq!(ids.len(), n, "the demo has {n} photos");
    session.execute("library.select", &json!({"ids": ids, "active": ids[0]})).unwrap();
    let opened: Opened = Default::default();
    let o = opened.clone();
    let services = Services {
        png: None,
        open_with: Some(Box::new(move |path: &str, app: &str| {
            o.lock().unwrap().push((path.to_string(), app.to_string()));
            Ok(())
        })),
        check_editor: Some(Box::new(move |_: &str| if installed { Ok(()) } else { Err("PhotoCraft isn't installed".into()) })),
        layers_support: Some(Box::new(move |_: &str| supports)),
        ..Default::default()
    };
    let h = Headless::new(LightcraftApp::new(session, services), [1200.0, 800.0], 1.0);
    Rig { h, opened, dir, ids }
}

impl Rig {
    fn layers(&mut self, params: Value) -> Value {
        let mut params = params;
        params["dir"] = json!(self.dir.to_string_lossy());
        self.h.request("engine.execute", json!({"command": "photo.openAsLayersInPhotocraft", "params": params}), T)
    }

    /// Photo `id`'s file name without its extension.
    fn stem(&self, id: u64) -> String {
        let p = self.h.app.session.catalog.photo(lightcraft_catalog::PhotoId(id)).map(|p| p.file_name.clone()).unwrap_or_default();
        std::path::Path::new(&p).file_stem().unwrap().to_string_lossy().to_string()
    }

    fn select(&mut self, ids: &[u64], active: u64) {
        self.h.app.session.execute("library.select", &json!({"ids": ids, "active": active})).unwrap();
    }

    fn toast(&self) -> String {
        self.h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// The layer list goes to PhotoCraft as one file: top layer first (the active photo, then the
/// selection in order), absolute paths, each layer file named after its photo.
#[test]
fn a_photocraft_that_knows_layers_gets_one_pcstack_file() {
    let mut r = rig("stack", 3, true, true);
    let id = r.ids.clone();
    r.select(&id, id[1]);
    let doc = format!("{} + 2", r.stem(id[1]));
    let library = r.h.app.session.catalog.len();
    let res = r.layers(json!({"wait": true}));
    assert_eq!(res["ok"], true, "{res}");
    let calls = r.opened.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one document: {calls:?}");
    assert!(calls[0].0.ends_with(&format!("{doc}.pcstack")) && calls[0].1 == "PhotoCraft", "{calls:?}");
    let v: Value = serde_json::from_slice(&std::fs::read(&calls[0].0).unwrap()).unwrap();
    assert_eq!((v["kind"].as_str(), v["version"].as_u64(), v["name"].as_str()), (Some("photocraft-open-as-layers"), Some(1), Some(doc.as_str())));
    let files: Vec<String> = v["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
    let names: Vec<String> = files.iter().map(|f| std::path::Path::new(f).file_name().unwrap().to_string_lossy().to_string()).collect();
    let want: Vec<String> = [id[1], id[0], id[2]].iter().map(|i| format!("{}.psd", r.stem(*i))).collect();
    assert_eq!(names, want, "the PhotoCraft file format; layer names come from these");
    assert!(files.iter().all(|f| std::path::Path::new(f).is_absolute() && std::path::Path::new(f).is_file()));
    assert_eq!(res["result"]["oneByOne"], false);
    assert_eq!(res["result"]["stack"], json!(calls[0].0));
    assert!(r.toast().contains("opened in PhotoCraft with 3 layers"), "{}", r.toast());
    assert_eq!(r.h.app.session.catalog.len(), library, "the rendered copies are not added to the library");
}

/// The official PhotoCraft doesn't know the layer list: its files open one by one, no `.pcstack`
/// is written, and the toast says why.
#[test]
fn a_photocraft_without_layers_gets_the_files_one_by_one() {
    let mut r = rig("fallback", 2, false, true);
    let res = r.layers(json!({"wait": true, "format": "tiff", "bitDepth": 8}));
    assert_eq!(res["ok"], true, "{res}");
    let calls = r.opened.lock().unwrap().clone();
    let names: Vec<String> = calls.iter().map(|c| std::path::Path::new(&c.0).file_name().unwrap().to_string_lossy().to_string()).collect();
    let want: Vec<String> = r.ids.iter().map(|i| format!("{}.tif", r.stem(*i))).collect();
    assert_eq!(names, want, "{calls:?}");
    assert!(calls.iter().all(|c| c.1 == "PhotoCraft"));
    assert_eq!(res["result"]["oneByOne"], true);
    assert!(res["result"]["stack"].is_null());
    let dir = std::path::Path::new(&calls[0].0).parent().unwrap().to_path_buf();
    assert!(files_in(&dir).iter().all(|f| !f.ends_with(".pcstack")), "{:?}", files_in(&dir));
    assert!(r.toast().contains("can't open photos as layers"), "{}", r.toast());
}

#[test]
fn a_missing_photocraft_writes_nothing() {
    let mut r = rig("missing", 2, true, false);
    let res = r.layers(json!({"wait": true}));
    assert_eq!(res["ok"], false, "{res}");
    assert!(files_in(&r.dir).is_empty() && r.opened.lock().unwrap().is_empty());
    assert!(r.toast().contains("isn't installed"), "{}", r.toast());
}

/// An editor that won't start leaves the files (a retry would make another set): `failed` and a
/// toast say why instead of an error.
#[test]
fn an_editor_that_wont_open_the_layers_is_reported_not_failed() {
    let mut r = rig("wontopen", 2, true, true);
    r.h.app.services.open_with = Some(Box::new(|_: &str, _: &str| Err("the editor quit".into())));
    let res = r.layers(json!({"wait": true}));
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["result"]["failed"], "the editor quit");
    assert!(r.toast().contains("the editor quit"), "{}", r.toast());
}

#[test]
fn one_photo_or_257_are_refused_with_a_message_and_nothing_is_written() {
    let mut r = rig("count", 2, true, true);
    let first = r.ids[0];
    r.select(&[first], first);
    assert!(!crate::menus::ui_enabled(&r.h.app, "photo.openAsLayersInPhotocraft"), "needs two photos");
    let res = r.layers(json!({"wait": true}));
    assert_eq!(res["ok"], false, "{res}");
    assert!(r.toast().contains("at least two"), "{}", r.toast());
    let many: Vec<u64> = (1..=257).collect();
    let res = r.layers(json!({"wait": true, "ids": many}));
    assert_eq!(res["ok"], false, "{res}");
    assert!(r.toast().contains("257") && r.toast().contains("256"), "{}", r.toast());
    assert!(files_in(&r.dir).is_empty() && r.opened.lock().unwrap().is_empty());
}

#[test]
fn the_command_is_enabled_with_two_or_more_photos_selected_and_an_opener() {
    let mut r = rig("enabled", 3, true, true);
    let on = |r: &Rig| crate::menus::ui_enabled(&r.h.app, "photo.openAsLayersInPhotocraft");
    assert!(on(&r), "three selected");
    let id = r.ids.clone();
    r.select(&id[..2], id[0]);
    assert!(on(&r), "two selected");
    r.select(&id[..1], id[0]);
    assert!(!on(&r), "one selected");
    r.select(&id[..2], id[0]);
    r.h.app.services.open_with = None;
    assert!(!on(&r), "a host that can't start programs (the web)");
}

/// In the background the render shows a row in the activity stack with ✕; stopping it writes and
/// opens nothing; letting it finish opens the layer list and ends the row.
#[test]
fn the_render_has_an_activity_row_that_can_stop_it() {
    let mut r = rig("task", 3, true, true);
    let res = r.layers(json!({}));
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["result"]["background"], true);
    let rows = r.h.app.session.activity.list();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].kind, rows[0].total, rows[0].cancellable), ("layers", 3, true));
    r.h.app.session.activity.cancel(rows[0].id).unwrap();
    assert!(r.h.app.layers.as_ref().unwrap().cancel.load(std::sync::atomic::Ordering::Relaxed), "the stack's ✕ stops it");
    let ctx = egui::Context::default();
    wait_for_task(&mut r, &ctx);
    assert!(r.opened.lock().unwrap().is_empty(), "a stopped run opens nothing");
    assert!(r.toast().contains("cancelled"), "{}", r.toast());
    assert!(r.h.app.session.activity.list().is_empty(), "the row goes with the task");
    assert!(files_in(&r.dir).is_empty(), "and its files and folder");

    let res = r.layers(json!({}));
    assert_eq!(res["ok"], true, "{res}");
    wait_for_task(&mut r, &ctx);
    assert_eq!(r.opened.lock().unwrap().len(), 1, "{}", r.toast());
    assert!(r.opened.lock().unwrap()[0].0.ends_with(".pcstack"));
    assert!(r.h.app.session.activity.list().is_empty());
}

fn wait_for_task(r: &mut Rig, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(300);
    while r.h.app.layers.is_some() {
        super::poll(&mut r.h.app, ctx);
        assert!(Instant::now() < deadline, "the layers never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The command is in the photo menu's catalogue (under Photo › Edit In, for the person to add),
/// and hidden from a menu opened with one photo selected.
#[test]
fn the_photo_menu_offers_the_command() {
    let r = rig("menu", 3, true, true);
    let commands = crate::panels::photo_menu::menu_commands(&r.h.app);
    let found =
        commands.iter().find(|(n, _)| matches!(n, crate::menus::menubar::MenuNode::Item { id, .. } if id == "photo.openAsLayersInPhotocraft"));
    let (node, path) = found.expect("the menu bar has it, so the catalogue offers it");
    assert_eq!(path, "Photo › Edit In");
    assert!(matches!(node, crate::menus::menubar::MenuNode::Item { label, .. } if label == "Open as Layers in PhotoCraft…"), "{node:?}");
    let slot = crate::panels::photo_menu::Slot::parse("cmd:photo.openAsLayersInPhotocraft").unwrap();
    assert!(slot.several(), "left out of a menu opened with one photo");
}
