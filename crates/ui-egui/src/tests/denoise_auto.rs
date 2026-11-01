//! Auto AI Denoise through the interface: opening a photo in Detail asks the engine, once per photo.

use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_denoise::manifest::{DenoiserManifest, Domain, Gain};
use lightcraft_engine::Session;
use serde_json::json;

use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

/// A library with two raw photos at ISO 12800 and 400, the first one open in Detail.
fn setup(name: &str) -> (LightcraftApp, PathBuf, PhotoId, PhotoId) {
    let dir = std::env::temp_dir().join(format!("lc-auto-denoise-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new().with_fs();
    s.open_library(dir.join("library"), true).unwrap();
    s.set_denoise_models_dir(Some(dir.join("denoise")));
    s.execute("denoise.settings", &json!({"runOn": "cpu"})).unwrap();
    let first = s.catalog.photos().map(|p| p.id.0).max().unwrap_or(0) + 1;
    let mut ids = Vec::new();
    for (i, iso) in [12_800u32, 400].into_iter().enumerate() {
        let id = PhotoId(first + i as u64);
        let file = format!("iso{iso}.dng");
        let mut raw = Photo::new(id, Source::File { path: dir.join(&file).to_string_lossy().into_owned() }, &file, "DNG", 128, 128, "2026-10-07");
        raw.kind = MediaKind::Raw;
        raw.meta.iso = Some(iso);
        s.commit("fixture", Op::AddPhoto { photo: Box::new(raw) }).unwrap();
        ids.push(id);
    }
    let mut app = LightcraftApp::new(s, Services::default());
    app.ui.view = ViewMode::Detail;
    (app, dir, ids[0], ids[1])
}

fn install_model(app: &mut LightcraftApp, dir: &Path) {
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let bytes = lightcraft_denoise::synthetic::smoothing_onnx();
    std::fs::write(source.join("model.onnx"), &bytes).unwrap();
    let m = DenoiserManifest {
        id: "test-smoothing".into(),
        name: "Synthetic smoothing".into(),
        version: "1".into(),
        licence: Default::default(),
        source: None,
        sha256: None,
        size_bytes: Some(bytes.len() as u64),
        provenance: "Synthetic tests".into(),
        domain: Domain::BayerToRgb,
        tile: 64,
        overlap: 16,
        gain: Gain::None,
    };
    std::fs::write(source.join("denoise-model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    app.run("denoise.models.install", json!({"path": source.join("model.onnx"), "acknowledged": true})).unwrap();
}

fn open(app: &mut LightcraftApp, id: PhotoId) {
    app.run("library.select", json!({"ids": [id.0]})).unwrap();
    crate::panels::denoise_auto::pump(app, &egui::Context::default());
}

fn on(app: &LightcraftApp, id: PhotoId) -> bool {
    app.session.develop_of(id).unwrap().enhance.denoise_enabled()
}

#[test]
fn opening_a_high_iso_photo_switches_it_on_once_and_it_stays() {
    let (mut app, dir, high, low) = setup("open");
    install_model(&mut app, &dir);
    app.run("denoise.settings", json!({"autoOpen": true})).unwrap();
    open(&mut app, high);
    assert!(on(&app, high));
    assert_eq!(app.session.undo.last().map(|u| u.label.as_str()), Some("Auto AI Denoise"));
    let steps = app.session.undo.len();
    // another photo (below the ISO) and back: still on, no second step
    open(&mut app, low);
    assert!(!on(&app, low));
    open(&mut app, high);
    assert!(on(&app, high));
    assert_eq!(app.session.undo.len(), steps);
    // the user switches it off: coming back does not bring it back
    app.run("denoise.toggle", json!({"id": high.0, "enabled": false})).unwrap();
    open(&mut app, low);
    open(&mut app, high);
    assert!(!on(&app, high));
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn nothing_happens_with_the_setting_off_or_outside_the_detail_view() {
    let (mut app, dir, high, _) = setup("off");
    install_model(&mut app, &dir);
    open(&mut app, high);
    assert!(!on(&app, high), "off by default");
    app.run("denoise.settings", json!({"autoOpen": true})).unwrap();
    app.ui.view = ViewMode::PhotoGrid;
    open(&mut app, high);
    assert!(!on(&app, high), "only the Detail view opens a photo");
    app.ui.view = ViewMode::Detail;
    crate::panels::denoise_auto::pump(&mut app, &egui::Context::default());
    assert!(on(&app, high), "in Detail it is opened");
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_model_it_says_so_once_and_changes_nothing() {
    let (mut app, dir, high, low) = setup("nomodel");
    app.run("denoise.settings", json!({"autoOpen": true})).unwrap();
    let steps = app.session.undo.len();
    open(&mut app, high);
    assert!(!on(&app, high));
    assert_eq!(app.session.undo.len(), steps);
    let first = app.ui.toast.take().map(|t| t.0).expect("the missing model is mentioned once");
    assert!(first.contains("denoise model"), "{first}");
    open(&mut app, low);
    open(&mut app, high);
    assert!(app.ui.toast.is_none(), "not again on the next photo");
    // once a model is installed, the same photo gets it the next time it is opened
    install_model(&mut app, &dir);
    open(&mut app, low);
    open(&mut app, high);
    assert!(on(&app, high));
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_settings_tab_shows_a_switch_and_an_iso_field_that_the_commands_change() {
    let (mut app, dir, _, _) = setup("settings");
    app.ui.dialog = Some(crate::state::Dialog::Settings { tab: "denoise".into() });
    let mut h = crate::headless::Headless::new(app, [1100.0, 800.0], 1.0);
    h.settle(std::time::Duration::from_secs(5));
    let text = h.painted_text().join("\n");
    assert!(text.contains("Auto AI Denoise") && text.contains("ISO Threshold") && text.contains("Denoise High-ISO Photos When Opened"), "{text}");
    let r = h.request(
        "engine.execute",
        json!({"command": "denoise.settings", "params": {"autoOpen": true, "autoIso": 3200}}),
        std::time::Duration::from_secs(5),
    );
    assert_eq!((r["result"]["autoOpen"].clone(), r["result"]["autoIso"].clone()), (json!(true), json!(3200)), "{r}");
    drop(h);
    let _ = std::fs::remove_dir_all(dir);
}
