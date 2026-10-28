//! Use Smart Previews for viewing and editing: the Performance row, the setting saved with the user's settings and
//! followed by the engine, the one-time question about building the missing smart previews, and the Info line.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const CHECK: &str = "check:settings.smartPreviewsForEditing";

struct TempFolder(std::path::PathBuf);

impl Drop for TempFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A library on disk with `n` PNG photos (they have originals, so they can have smart previews).
fn library(tag: &str, n: u8, smart: bool) -> (TempFolder, Headless) {
    let base = std::env::temp_dir().join(format!("lc-ui-smartedit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (src, lib) = (base.join("src"), base.join("lib"));
    std::fs::create_dir_all(&src).unwrap();
    let mut paths = Vec::new();
    for seed in 0..n {
        let data: Vec<[u8; 4]> = (0..48 * 32).map(|i| [(i % 48 * 5) as u8, (i / 48 * 7) as u8, seed * 30, 255]).collect();
        let img = lightcraft_raster::Rgba8 { width: 48, height: 32, data };
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
        let path = src.join(format!("p{seed}.png"));
        std::fs::write(&path, bytes).unwrap();
        paths.push(path.to_string_lossy().to_string());
    }
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.open_library(&lib, false).unwrap();
    session.execute("library.import", &json!({"paths": paths})).unwrap();
    if smart {
        // (before the app asks whether the files are there: it keeps the answer for a while)
        session.execute("library.smartPreviews", &json!({})).unwrap();
    }
    let mut h = Headless::new(LightcraftApp::new(session, Services::default()), [1600.0, 1000.0], 1.0);
    h.settle(T);
    (TempFolder(base), h)
}

fn show_performance(h: &mut Headless) {
    assert_eq!(h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "performance"}}), T)["ok"], true);
    h.settle(T);
    let window = h.app.widgets.iter().find(|(w, _)| w == "dialog:window").map(|(_, r)| *r).expect("the Settings window");
    h.request("ui.move", json!({"x": window.center().x, "y": window.center().y}), T);
    for _ in 0..6 {
        h.request("ui.scroll", json!({"dy": -400.0}), T);
        h.step();
    }
    h.settle(T);
}

#[test]
fn the_setting_is_off_by_default_and_the_performance_page_has_its_row() {
    let (_dir, mut h) = library("row", 2, false);
    assert!(!h.app.ui.settings.smart_previews_for_editing && !h.app.session.media.smart_editing());
    show_performance(&mut h);
    assert!(h.app.widgets.iter().any(|(w, _)| w == CHECK), "the row is on the Performance page");
    let text = h.painted_text().join("\n");
    assert!(text.contains("Use Smart Previews for viewing and editing"), "{text}");
    assert!(text.contains("Faster with photos on a network or external drive; the original is read when you zoom to 100 % or export"), "{text}");
}

#[test]
fn switching_it_on_asks_once_about_the_missing_smart_previews_and_building_fills_them() {
    let (_dir, mut h) = library("offer", 3, false);
    show_performance(&mut h);
    assert_eq!(h.request("ui.clickWidget", json!({"id": CHECK}), T)["ok"], true);
    h.settle(T);
    h.step();
    assert!(h.app.ui.settings.smart_previews_for_editing && h.app.session.media.smart_editing(), "saved and in the engine");
    assert!(matches!(h.app.ui.dialog, Some(Dialog::SmartPreviewsForEditing { missing: 3 })), "{:?}", h.app.ui.dialog);
    assert!(h.app.ui.settings.smart_editing_offered);
    // Build Smart Previews
    let r = h.app.run("library.smartPreviewsForEditing", json!({"build": true})).unwrap();
    assert_eq!(r["building"], 3, "{r}");
    for _ in 0..200 {
        if h.app.session.activity.list().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let r = h.app.run("library.smartPreviewsForEditing", json!({})).unwrap();
    assert_eq!(r["smartPreviews"], 3, "{r}");
    // off and on again: not asked twice
    h.app.ui.dialog = None;
    h.app.ui.settings.smart_previews_for_editing = false;
    h.step();
    h.app.ui.settings.smart_previews_for_editing = true;
    h.step();
    assert!(h.app.ui.dialog.is_none(), "asked once: {:?}", h.app.ui.dialog);
}

#[test]
fn a_command_run_by_an_agent_changes_the_saved_setting_and_none_is_asked() {
    let (_dir, mut h) = library("agent", 2, false);
    h.step();
    h.app.run("library.smartPreviewsForEditing", json!({"enabled": true})).unwrap();
    h.step();
    assert!(h.app.ui.settings.smart_previews_for_editing, "the saved setting follows the command");
    assert!(h.app.ui.dialog.is_none(), "an agent is not asked");
    h.app.run("library.smartPreviewsForEditing", json!({"enabled": false})).unwrap();
    h.step();
    assert!(!h.app.ui.settings.smart_previews_for_editing);
}

#[test]
fn the_info_panel_says_the_photo_is_edited_from_its_smart_preview() {
    let (_dir, mut h) = library("info", 1, true);
    h.app.run("library.smartPreviewsForEditing", json!({"enabled": true})).unwrap();
    let id = h.app.session.active().unwrap();
    h.app.session.media.clear_sources();
    // the loupe's render comes from the smart preview
    let r = h.app.session.loupe_job(id, 200, 200, true).unwrap().run();
    h.app.session.accept(&r);
    let p = h.app.session.catalog.photo(id).unwrap().clone();
    assert!(h.app.session.media.editing_from_smart_preview(id, &p));
    // (the app checks files off the UI thread: the smart preview is known to be there a moment later)
    let line = |h: &mut Headless| {
        for _ in 0..200 {
            h.step();
            let p = h.app.session.catalog.photo(id).unwrap().clone();
            if let Some(l) = crate::panels::right::smart_line(&h.app, &p) {
                return Some(l);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    };
    assert_eq!(line(&mut h), Some(("Editing from Smart Preview", true)));
    // with the setting off the panel only says one is there
    h.app.run("library.smartPreviewsForEditing", json!({"enabled": false})).unwrap();
    h.step();
    assert_eq!(line(&mut h), Some(("Smart preview available", false)));
}
