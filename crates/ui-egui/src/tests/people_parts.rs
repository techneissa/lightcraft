//! Fork feature (`fork/people-parsing.md`): Settings ▸ Faces ▸ People Parts (headless). A set without its models
//! asks before it downloads; an installed set is chosen with one click.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn settings(tag: &str) -> (Headless, std::path::PathBuf) {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    let dir = std::env::temp_dir().join(format!("lc-ui-people-parts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("faces")).unwrap();
    h.app.session.face_models_dir = Some(dir.join("faces"));
    h.app.session.parsing.set_paths(Some(dir.join("models")), Some(dir.join("cache")), Some(dir.join("choice.json")));
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "faces"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(30));
    h.request("ui.widgets", json!({}), T);
    h.step();
    // the section is at the end of the tab: scroll the page down to it
    assert_eq!(h.request("ui.move", json!({"x": 500.0, "y": 500.0}), T)["ok"], true);
    assert_eq!(h.request("ui.scroll", json!({"dx": 0.0, "dy": -3000.0}), T)["ok"], true);
    h.step();
    h.step();
    (h, dir)
}

fn has(h: &mut Headless, id: &str) -> bool {
    h.request("ui.widgets", json!({}), T).to_string().contains(id)
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

#[test]
fn a_set_without_models_asks_first_and_cancel_downloads_nothing() {
    let (mut h, dir) = settings("ask");
    assert!(has(&mut h, "button:parsing.set-segformer") && has(&mut h, "button:parsing.set-bisenet"));
    assert!(!has(&mut h, "button:parsingDownload"));
    click(&mut h, "button:parsing.set-bisenet");
    assert!(has(&mut h, "button:parsingDownload"), "no question before the download");
    click(&mut h, "button:parsingCancel");
    assert!(!has(&mut h, "button:parsingDownload"));
    let status = h.app.session.execute("parsing.status", &json!({})).unwrap();
    assert!(status["sets"].as_array().unwrap().iter().all(|s| s["download"]["state"] == "idle"), "{status}");
    assert!(!dir.join("models").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_installed_set_is_chosen_with_one_click() {
    let (mut h, dir) = settings("choose");
    let status = h.app.session.execute("parsing.status", &json!({})).unwrap();
    let bisenet = status["sets"].as_array().unwrap().iter().find(|s| s["id"] == "bisenet").unwrap().clone();
    for f in bisenet["files"].as_array().unwrap() {
        let folder = dir.join("models").join(f["folder"].as_str().unwrap());
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::File::create(folder.join(f["name"].as_str().unwrap())).unwrap().set_len(f["bytes"].as_u64().unwrap()).unwrap();
    }
    h.step();
    click(&mut h, "button:parsing.set-bisenet");
    assert!(!has(&mut h, "button:parsingDownload"));
    assert_eq!(h.app.session.execute("parsing.status", &json!({})).unwrap()["modelSet"], "bisenet");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_download_leaves_the_choice_as_it_was() {
    let (mut h, dir) = settings("failed");
    h.app.session.parsing.set_test_fetch(Some(std::sync::Arc::new(|_, _| lightcraft_engine::people_parsing::Download::Failed("no network".into()))));
    click(&mut h, "button:parsing.set-bisenet");
    click(&mut h, "button:parsingDownload");
    for _ in 0..200 {
        let st = h.app.session.execute("parsing.status", &json!({})).unwrap();
        if st["sets"].as_array().unwrap().iter().any(|s| s["download"]["state"] == "failed") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    h.step();
    let status = h.app.session.execute("parsing.status", &json!({})).unwrap();
    assert_eq!(status["modelSet"], "segformer", "a download that failed chose its set: {status}");
    let _ = std::fs::remove_dir_all(&dir);
}
