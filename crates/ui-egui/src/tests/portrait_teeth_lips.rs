//! The Portrait tab's Teeth and Lips section (`panels/portrait/teeth_lips.rs`): its sliders, and the lip colour
//! swatch and shades, for the picked face only.

use std::sync::Arc;
use std::time::Duration;

use lightcraft_develop::retouch::{Face, Feature, NormRect};
use lightcraft_engine::retouch_input::FaceProvider;
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

struct TwoFaces;

impl FaceProvider for TwoFaces {
    fn name(&self) -> &str {
        "Two Faces"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        Ok(vec![Face::boxed(NormRect::new(0.2, 0.2, 0.35, 0.45)), Face::boxed(NormRect::new(0.6, 0.25, 0.72, 0.45))])
    }
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "click {id}: {r}");
    h.step();
}

fn has(h: &mut Headless, id: &str) -> bool {
    let r = h.request("ui.widgets", json!({"filter": id}), T);
    r["result"].as_array().into_iter().flatten().any(|w| w["id"] == id)
}

/// A shade, clicked with the second face picked, gives that face the colour and turns its Lip Color on, as one
/// undo step; the other face and the settings for every face keep theirs.
#[test]
fn a_lip_shade_colours_the_picked_face_only() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    // (an encoder for the screenshot LIGHTCRAFT_TEST_SHOTS asks for)
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let services = Services {
        png: Some(Box::new(png)),
        write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
        ..Default::default()
    };
    let app = LightcraftApp::new(session, services);
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Portrait;
    let photo = h.app.session.active().unwrap();
    h.app.session.retouch_detect_now(photo, false).unwrap();
    h.step();
    h.step();
    click(&mut h, "section:portrait.teethLips");
    assert!(has(&mut h, "lipColorSwatch") && has(&mut h, "lipShade.0"));
    click(&mut h, "portrait:face:1");
    click(&mut h, "lipShade.4");
    let d = h.app.session.develop_of(photo).unwrap();
    let faces = h.app.session.retouch_faces_of(photo).found.map(|f| f.input.faces.clone()).unwrap_or_default();
    let mine = d.retouch.settings_for(&faces, 1);
    assert_eq!(
        ["lipColorR", "lipColorG", "lipColorB", "lipColorAmount"].map(|f| mine.get(&format!("retouch.teethLips.{f}")).unwrap_or(-1.0)),
        [140.0, 30.0, 70.0, 60.0]
    );
    assert!(d.retouch.settings_for(&faces, 0).teeth_lips.is_off() && d.retouch.global.teeth_lips.is_off());
    if let Some(p) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        let path = std::path::Path::new(&p).join("teeth-lips-section.png");
        // (the app writes over a file that is there; a new path needs a create-new writer the test has not)
        let _ = std::fs::write(&path, b"");
        let r = h.request("ui.screenshot", json!({"path": path.to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    // one undo step takes it all back
    let r = h.request("engine.execute", json!({"command": "edit.undo", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(h.app.session.develop_of(photo).unwrap().retouch.is_off());
}
