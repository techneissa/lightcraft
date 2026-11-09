//! Fork feature (`fork/apple-vision-masks.md`, issue #115): Settings ▸ Faces ▸ Hair Edges offers Apple Vision on a Mac
//! that runs it; one click chooses it for one kind and keeps it, with no download question (headless).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};
use lightcraft_engine::hair_edges::{Edge, EdgeKind, HairEdges};

const T: Duration = Duration::from_secs(20);

fn has(h: &mut Headless, id: &str) -> bool {
    h.request("ui.widgets", json!({}), T).to_string().contains(id)
}

#[test]
fn apple_vision_is_one_click_and_kept() {
    if !HairEdges::AVAILABLE {
        return;
    }
    if !lightcraft_engine::hair_edges::vision::available() {
        lightcraft_testkit::skip!(System, "Apple Vision masks need macOS 14 or later on a Mac");
        return;
    }
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    let dir = std::env::temp_dir().join(format!("lc-ui-vision-edges-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("faces")).unwrap();
    h.app.session.face_models_dir = Some(dir.join("faces"));
    h.app.session.hair_edges.set_paths(Some(dir.join("models")), Some(dir.join("mask-edges.json")));
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "faces"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(30));
    h.request("ui.widgets", json!({}), T);
    h.step();
    assert_eq!(h.request("ui.move", json!({"x": 500.0, "y": 500.0}), T)["ok"], true);
    assert_eq!(h.request("ui.scroll", json!({"dx": 0.0, "dy": -4000.0}), T)["ok"], true);
    h.step();
    h.step();
    for id in ["people", "subject"] {
        assert!(has(&mut h, &format!("button:maskEdges.{id}-vision")), "{id}");
    }
    let r = h.request("ui.clickWidget", json!({"id": "button:maskEdges.subject-vision"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(!has(&mut h, "button:maskEdgesDownload"), "no download question");
    let e = h.app.session.hair_edges.clone();
    assert_eq!(e.chosen(EdgeKind::Subject), Edge::Vision);
    assert_eq!(e.chosen(EdgeKind::People), Edge::Current, "the other kind is untouched");
    let saved = std::fs::read_to_string(dir.join("mask-edges.json")).unwrap();
    assert!(saved.contains("apple-vision"), "{saved}");
    let _ = std::fs::remove_dir_all(&dir);
}
