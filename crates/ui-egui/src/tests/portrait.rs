//! The Edit panel's Portrait section (fork, `fork/retouch-foundation.md`): its feature sections and its face picker.

use std::sync::Arc;
use std::time::Duration;

use lightcraft_develop::retouch::{FEATURES, Face, NormRect};
use lightcraft_engine::retouch_input::FaceProvider;
use serde_json::json;

use crate::headless::Headless;
use crate::panels::edit::portrait::picked;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

struct TwoFaces;

impl FaceProvider for TwoFaces {
    fn name(&self) -> &str {
        "Two Faces"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        Ok(vec![
            Face { person: Some("ann".into()), ..Face::boxed(NormRect::new(0.2, 0.2, 0.35, 0.45)) },
            Face::boxed(NormRect::new(0.6, 0.25, 0.72, 0.45)),
        ])
    }
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "click {id}: {r}");
    h.step();
}

#[test]
fn the_portrait_section_shows_a_section_per_feature_and_a_face_picker() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = crate::state::RightPanel::Edit;
    h.app.ui.open_sections = Vec::new();
    h.step();
    let r = h.request("engine.execute", json!({"command": "section.portrait", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.open_sections, ["portrait"]);
    // opening the section finds the faces, in the background
    let photo = h.app.session.active().unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while h.app.session.retouch_faces_of(photo).found.is_none() {
        assert!(std::time::Instant::now() < until, "the faces were never found");
        h.step();
    }
    h.step();
    h.step();
    for f in FEATURES {
        let r = h.request("ui.clickWidget", json!({"id": format!("flyout:portrait.{}", f.key)}), T);
        assert_eq!(r["ok"], true, "{}: {r}", f.key);
        h.step();
        assert!(h.app.ui.flyout_open(&format!("portrait.{}", f.key)), "{} opens", f.key);
        click(&mut h, &format!("flyout:portrait.{}", f.key));
        assert!(!h.app.ui.flyout_open(&format!("portrait.{}", f.key)), "{} closes", f.key);
    }
    let ctx = h.view.ctx.clone();
    assert_eq!(picked(&ctx, photo), None, "All by default");
    click(&mut h, "portrait:face:1");
    assert_eq!(picked(&ctx, photo), Some(1));
    click(&mut h, "portrait:face:all");
    assert_eq!(picked(&ctx, photo), None);
    // nothing about the photo changed by looking
    assert!(h.app.session.develop_of(photo).unwrap().retouch.is_default());
}
