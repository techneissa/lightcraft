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
    // the side panel says which faces the sliders change; the picker is on the photo
    assert!(rect_of(&mut h, "portrait:editing").is_some());
    let ctx = h.view.ctx.clone();
    assert_eq!(picked(&ctx, photo), None, "All by default");
    click(&mut h, "portrait:face:1");
    assert_eq!(picked(&ctx, photo), Some(1));
    click(&mut h, "portrait:face:all");
    assert_eq!(picked(&ctx, photo), None);
    // nothing about the photo changed by looking
    assert!(h.app.session.develop_of(photo).unwrap().retouch.is_default());
}

fn rect_of(h: &mut Headless, id: &str) -> Option<[f64; 4]> {
    let r = h.request("ui.widgets", json!({"filter": id}), T);
    r["result"].as_array().into_iter().flatten().find(|w| w["id"] == id).and_then(|w| serde_json::from_value(w["rect"].clone()).ok())
}

/// The face picker is a card at the top-right of the photo: it picks faces, folds into a button (remembered),
/// stays put while the photo zooms, and hides with the tool overlay.
#[test]
fn the_face_picker_on_the_photo_picks_folds_and_hides() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = crate::state::RightPanel::Edit;
    h.app.ui.open_sections = vec!["portrait".into()];
    let photo = h.app.session.active().unwrap();
    h.app.session.retouch_detect_now(photo, false).unwrap();
    h.step();
    h.step();
    let image = rect_of(&mut h, "canvas:image").unwrap();
    let card = rect_of(&mut h, "portrait:facesCard").expect("the picker is on the photo");
    // top-right corner of the photo, inside it
    assert!(card[0] + card[2] <= image[0] + image[2] && card[1] >= image[1], "{card:?} in {image:?}");
    assert!(card[0] + card[2] > image[0] + image[2] - 40.0 && card[1] < image[1] + 40.0, "near the top-right corner: {card:?} {image:?}");
    let ctx = h.view.ctx.clone();
    click(&mut h, "portrait:face:0");
    assert_eq!(picked(&ctx, photo), Some(0), "picked on the photo");
    // zoomed in, the photo fills the view and the card keeps to the view's top-right corner
    let r = h.request("engine.execute", json!({"command": "view.zoom100", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..30 {
        h.step();
    }
    let after = rect_of(&mut h, "portrait:facesCard").unwrap();
    assert!(after[1] <= card[1] + 0.5 && after[0] + after[2] >= card[0] + card[2] - 0.5, "{after:?} {card:?}");
    let panned = {
        h.app.ui.pan = (0.2, 0.7);
        for _ in 0..30 {
            h.step();
        }
        rect_of(&mut h, "portrait:facesCard").unwrap()
    };
    assert_eq!(panned, after, "panning moves the photo, not the card");
    // folds into one button, remembered in the saved state
    click(&mut h, "portrait:faces");
    assert!(h.app.ui.flyout_open(crate::panels::edit::portrait::FOLDED));
    assert!(rect_of(&mut h, "portrait:face:0").is_none(), "folded: no faces");
    click(&mut h, "portrait:faces");
    assert!(rect_of(&mut h, "portrait:face:0").is_some());
    // the tool overlay off (Shift+H to Never) hides it; the section closed hides it
    h.app.ui.tool_overlay = crate::state::ToolOverlay::Never;
    h.step();
    assert!(rect_of(&mut h, "portrait:facesCard").is_none());
    h.app.ui.tool_overlay = crate::state::ToolOverlay::Auto;
    h.app.ui.open_sections.clear();
    h.step();
    assert!(rect_of(&mut h, "portrait:facesCard").is_none());
}
