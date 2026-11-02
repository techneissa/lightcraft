//! Fork feature (issue #48): the model dropdown on the Masking heading's row. Only installed models are listed, it
//! hides with one model (a quiet "Get BiRefNet" asks first), choosing sets the model for new parts, and a part says
//! which model made it.

use std::path::PathBuf;
use std::time::Duration;

use lightcraft_develop::{MaskShape, SegMask};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn masking(name: &str, sam: bool, bire: bool) -> (Headless, PathBuf) {
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let root = std::env::temp_dir().join(format!("lc-ui-selmodel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sam3")).unwrap();
    app.session.segmenter.dir = Some(root.join("sam3"));
    if sam {
        for f in ["model.safetensors", "vocab.json", "merges.txt"] {
            std::fs::write(root.join("sam3").join(f), b"x").unwrap();
        }
    }
    if bire {
        std::fs::create_dir_all(root.join("birefnet")).unwrap();
        std::fs::write(root.join("birefnet").join("model.safetensors"), b"x").unwrap();
    }
    app.session.segmenter.birefnet.root = Some(root.clone());
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    for (method, params) in [("ui.set", json!({"view": "detail"})), ("engine.execute", json!({"command": "panel.masking"}))] {
        let r = h.request(method, params, T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.step();
    (h, root)
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

#[test]
fn the_dropdown_shows_with_two_models_and_sets_the_model_for_new_parts() {
    if !lightcraft_engine::segment::Segmenter::AVAILABLE {
        return;
    }
    let (mut h, root) = masking("two", true, true);
    assert!(has(&h, "dropdown:selectionModel"), "two models installed: the dropdown");
    assert!(!has(&h, "dropdown:getBiRefNet"));
    assert_eq!(h.app.session.segmenter.selection().id(), "birefnet", "never chosen: the finest edges");
    let r = h.request("ui.clickWidget", json!({"id": "dropdown:selectionModel"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    for m in ["sam3", "birefnet", "sam3+birefnet"] {
        assert!(has(&h, &format!("selectionModel:{m}")), "{m} is listed");
    }
    let r = h.request("ui.clickWidget", json!({"id": "selectionModel:sam3"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.session.segmenter.selection().id(), "sam3");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn with_one_model_the_dropdown_hides_and_birefnet_is_offered_with_its_size() {
    if !lightcraft_engine::segment::Segmenter::AVAILABLE {
        return;
    }
    let (mut h, root) = masking("one", true, false);
    assert!(!has(&h, "dropdown:selectionModel"), "one model: nothing to choose");
    assert!(has(&h, "dropdown:getBiRefNet"));
    let r = h.request("ui.clickWidget", json!({"id": "dropdown:getBiRefNet"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(has(&h, "button:getBiRefNetDownload"));
    assert!(!h.app.session.segmenter.birefnet_download_status().running, "nothing downloads before the yes");
    let _ = std::fs::remove_dir_all(root);
    // neither model, or only BiRefNet: no control
    let (h, root) = masking("none", false, false);
    assert!(!has(&h, "dropdown:selectionModel") && !has(&h, "dropdown:getBiRefNet"));
    let _ = std::fs::remove_dir_all(root);
    let (h, root) = masking("bire-only", false, true);
    assert!(!has(&h, "dropdown:selectionModel") && !has(&h, "dropdown:getBiRefNet"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_part_says_which_model_made_it() {
    use crate::panels::selection_model::part_tip;
    let seg = SegMask::from_logits(4, &[1.0; 16]);
    let shape =
        |made: Option<&str>| MaskShape::Subject { seg: Some(made.map_or(seg.clone(), |m| seg.clone().made_by(m))), detail: vec![], edge: 0.0 };
    assert!(part_tip(&shape(None)).unwrap().ends_with("SAM 3"));
    assert!(part_tip(&shape(Some("birefnet"))).unwrap().ends_with("BiRefNet"));
    assert!(part_tip(&shape(Some("sam3+birefnet"))).unwrap().ends_with("SAM 3 + BiRefNet Edges"));
    assert!(part_tip(&shape(Some("newer-model"))).unwrap().ends_with("newer-model"));
    assert_eq!(part_tip(&MaskShape::Subject { seg: None, detail: vec![], edge: 0.0 }), None, "a rough mask has no model");
    assert_eq!(part_tip(&MaskShape::sky()), None);
}
