//! Settings ▸ Import ▸ Lens profiles: the folder is saved with the app settings, set and read by
//! `lensDatabase.*`, and reported with a status line (profiles found, or why none).

use std::sync::Mutex;

use lightcraft_engine::lens_profiles::{self, FolderSource, Problem, Status};
use serde_json::json;

use crate::dialogs::settings::lens_database::status_line;
use crate::state::AppSettings;
use crate::{LightcraftApp, Services};

/// The engine's folder is process-wide: the tests that set it take turns.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const XML: &str = r#"<lensdatabase>
  <lens><maker>Testcam</maker><model>Testcam 18-55mm f/3.5-5.6</model><mount>T</mount><cropfactor>1.5</cropfactor>
    <calibration><distortion model="poly3" focal="18" k1="-0.03" /></calibration>
  </lens></lensdatabase>"#;

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() })
}

fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-ui-lens-db-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_folder_is_saved_and_old_settings_load_without_it() {
    let s = AppSettings { lens_database_dir: "/lenses".into(), ..Default::default() };
    let back: AppSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(back.lens_database_dir, "/lenses");
    let old: AppSettings = serde_json::from_value(json!({"gpu": false})).unwrap();
    assert_eq!(old.lens_database_dir, "");
}

#[test]
fn the_command_sets_reads_and_clears_the_folder() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if std::env::var_os(lens_profiles::ENV).is_some_and(|v| !v.is_empty()) {
        lightcraft_testkit::skip!(Input, "LIGHTCRAFT_LENSFUN_DIR is set and wins over the setting");
        return;
    }
    let mut app = app();
    // a missing folder
    let gone = std::env::temp_dir().join(format!("lc-ui-lens-db-gone-{}", std::process::id()));
    let r = app.run("lensDatabase.set", json!({"dir": gone.to_string_lossy()})).unwrap();
    assert_eq!((r["source"].as_str(), r["problem"].as_str(), r["profiles"].as_u64()), (Some("setting"), Some("missing"), Some(0)), "{r}");
    assert!(r["message"].as_str().unwrap().contains("doesn't exist"), "{r}");
    assert_eq!(app.ui.settings.lens_database_dir, gone.to_string_lossy());
    // a folder with a tiny synthetic file
    let dir = folder("set");
    std::fs::write(dir.join("t.xml"), XML).unwrap();
    let r = app.run("lensDatabase.set", json!({"dir": dir.to_string_lossy()})).unwrap();
    assert_eq!((r["profiles"].as_u64(), r["problem"].is_null()), (Some(1), true), "{r}");
    assert_eq!(r["message"], "1 lens profile found");
    assert!(lens_profiles::database().is_some(), "the engine reads it at once");
    // files added later are found by reload
    std::fs::write(dir.join("u.xml"), XML.replace("18-55", "24-70")).unwrap();
    assert_eq!(app.run("lensDatabase.get", json!({})).unwrap()["profiles"], 1);
    assert_eq!(app.run("lensDatabase.reload", json!({})).unwrap()["profiles"], 2);
    // bad input is refused, the folder stays
    assert!(app.run("lensDatabase.set", json!({"dir": 3})).is_err());
    assert!(app.run("lensDatabase.set", json!({})).is_err());
    assert_eq!(app.run("lensDatabase.get", json!({})).unwrap()["setting"], dir.to_string_lossy().as_ref());
    // cleared
    let r = app.run("lensDatabase.set", json!({"dir": null})).unwrap();
    assert_eq!((r["source"].as_str(), r["profiles"].as_u64()), (Some("none"), Some(0)), "{r}");
    assert_eq!(app.ui.settings.lens_database_dir, "");
    assert!(lens_profiles::database().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_setting_reaches_the_engine_on_the_next_frame_only_when_it_changed() {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if std::env::var_os(lens_profiles::ENV).is_some_and(|v| !v.is_empty()) {
        lightcraft_testkit::skip!(Input, "LIGHTCRAFT_LENSFUN_DIR is set and wins over the setting");
        return;
    }
    let dir = folder("sync");
    std::fs::write(dir.join("t.xml"), XML).unwrap();
    let ctx = egui::Context::default();
    let mut app = app();
    crate::dialogs::settings::lens_database::sync(&ctx, &app);
    assert!(lens_profiles::database().is_none(), "an app that never had a folder leaves the engine alone");
    app.ui.settings.lens_database_dir = dir.to_string_lossy().into_owned();
    crate::dialogs::settings::lens_database::sync(&ctx, &app);
    assert!(lens_profiles::database().is_some());
    app.ui.settings.lens_database_dir.clear();
    crate::dialogs::settings::lens_database::sync(&ctx, &app);
    assert!(lens_profiles::database().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_status_line_says_how_many_or_why_none() {
    let at = |source, problem: Option<Problem>, profiles, skipped| Status {
        source,
        dir: (source != FolderSource::None).then(|| "/lenses".into()),
        setting: None,
        profiles,
        skipped,
        problem,
    };
    assert_eq!(status_line(&at(FolderSource::None, None, 0, 0)), "No folder chosen");
    assert_eq!(status_line(&at(FolderSource::Setting, None, 1, 0)), "1 lens profile found");
    assert_eq!(status_line(&at(FolderSource::Setting, None, 1234, 0)), "1234 lens profiles found");
    assert_eq!(status_line(&at(FolderSource::Setting, None, 3, 2)), "3 lens profiles found, 2 files skipped");
    assert!(status_line(&at(FolderSource::Setting, Some(Problem::NoXml), 0, 0)).contains("no XML files"));
    assert!(status_line(&at(FolderSource::Setting, Some(Problem::NoLenses), 0, 1)).contains("no lens profiles"));
    assert!(status_line(&at(FolderSource::Setting, Some(Problem::Unreadable("denied".into())), 0, 0)).contains("denied"));
    // the variable wins, and the line says so
    let env = status_line(&at(FolderSource::Env, None, 5, 0));
    assert!(env.starts_with("5 lens profiles found") && env.contains("LIGHTCRAFT_LENSFUN_DIR") && env.contains("wins"), "{env}");
}

#[test]
fn settings_import_tab_offers_the_folder_row() {
    let mut h = crate::headless::Headless::new(app(), [900.0, 900.0], 1.0);
    let t = std::time::Duration::from_secs(10);
    h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "import"}}), t);
    h.settle(std::time::Duration::from_secs(2));
    let shown = h.request("ui.widgets", json!({"filter": "settingsLensDatabase"}), t)["result"].as_array().map_or(0, Vec::len);
    assert!(shown >= 2, "Choose… and Clear are on screen ({shown})");
}
