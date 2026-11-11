//! Fork feature (`fork/hair-edges.md`): Settings ▸ Faces ▸ Hair Edges (headless). Both kinds start on Current;
//! BiRefNet Matting without the model asks before it downloads; with the model it is chosen with one click and kept.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};
use lightcraft_engine::hair_edges::{Edge, EdgeKind, HairEdges, Matte};

const T: Duration = Duration::from_secs(20);

fn settings(tag: &str, installed: bool) -> (Headless, std::path::PathBuf) {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default());
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    let dir = std::env::temp_dir().join(format!("lc-ui-hair-edges-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("faces")).unwrap();
    h.app.session.face_models_dir = Some(dir.join("faces"));
    h.app.session.hair_edges.set_paths(Some(dir.join("models")), Some(dir.join("mask-edges.json")));
    if installed {
        h.app.session.hair_edges.set_runner(Some(Arc::new(|_, _, _, _| Ok(Matte { side: 1, logits: vec![0.0] }))));
    }
    let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "faces"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(30));
    h.request("ui.widgets", json!({}), T);
    h.step();
    // the section is at the end of the tab: scroll the page down to it
    assert_eq!(h.request("ui.move", json!({"x": 500.0, "y": 500.0}), T)["ok"], true);
    assert_eq!(h.request("ui.scroll", json!({"dx": 0.0, "dy": -4000.0}), T)["ok"], true);
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
fn without_the_model_matting_asks_first_and_cancel_downloads_nothing() {
    if !HairEdges::AVAILABLE {
        return;
    }
    let (mut h, dir) = settings("ask", false);
    for id in ["people", "subject"] {
        assert!(has(&mut h, &format!("button:maskEdges.{id}-current")) && has(&mut h, &format!("button:maskEdges.{id}-matting")), "{id}");
    }
    assert!(!has(&mut h, "button:maskEdgesDownload"));
    click(&mut h, "button:maskEdges.subject-matting");
    assert!(has(&mut h, "button:maskEdgesDownload"), "no question before the download");
    click(&mut h, "button:maskEdgesCancel");
    assert!(!has(&mut h, "button:maskEdgesDownload"));
    let e = &h.app.session.hair_edges;
    assert!(!e.download_status().running);
    assert_eq!((e.chosen(EdgeKind::People), e.chosen(EdgeKind::Subject)), (Edge::Current, Edge::Current));
    assert!(!dir.join("models").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_the_model_one_click_chooses_matting_for_one_kind() {
    if !HairEdges::AVAILABLE {
        return;
    }
    let (mut h, dir) = settings("choose", true);
    // (installed, both kinds start on Matting: pick Current for Subject so the check below can see a change)
    click(&mut h, "button:maskEdges.subject-current");
    assert_eq!(h.app.session.hair_edges.chosen(EdgeKind::Subject), Edge::Current);
    click(&mut h, "button:maskEdges.people-matting");
    assert!(!has(&mut h, "button:maskEdgesDownload"), "installed: no question");
    let e = h.app.session.hair_edges.clone();
    assert_eq!(e.chosen(EdgeKind::People), Edge::Matting);
    assert_eq!(e.chosen(EdgeKind::Subject), Edge::Current, "the other kind is untouched");
    let saved = std::fs::read_to_string(dir.join("mask-edges.json")).unwrap();
    assert!(saved.contains("birefnet-matting"), "{saved}");
    click(&mut h, "button:maskEdges.people-current");
    assert_eq!(e.chosen(EdgeKind::People), Edge::Current);
    let _ = std::fs::remove_dir_all(&dir);
}
