//! The Portrait tab's Background section (fork, `fork/retouch-background.md`): its sliders write the photo's one
//! setting whichever face is picked, and Swap With takes a colour, a picture file or a photo of the view.

use std::sync::Arc;
use std::time::Duration;

use lightcraft_develop::retouch::background::Swap;
use lightcraft_develop::retouch::{Face, NormRect};
use lightcraft_engine::retouch_input::FaceProvider;
use serde_json::json;

use crate::headless::Headless;
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

fn shown(h: &mut Headless, id: &str) -> bool {
    let r = h.request("ui.widgets", json!({"filter": id}), T);
    r["result"].as_array().into_iter().flatten().any(|w| w["id"] == id)
}

#[test]
fn the_background_section_sets_the_photos_sliders_and_what_swap_uses() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1600.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    click(&mut h, "icon:portrait");
    click(&mut h, "section:portrait.background");
    h.step();
    let photo = h.app.session.active().unwrap();
    for id in ["retouch.background.blur", "retouch.background.swap", "retouch.background.clean"] {
        assert!(shown(&mut h, &format!("slider:{id}")), "{id} is in the Background section");
    }
    // a face picked: the slider still writes the photo's one setting
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while h.app.session.retouch_faces_of(photo).found.is_none() {
        assert!(std::time::Instant::now() < until, "the faces were never found");
        h.step();
    }
    crate::panels::portrait::pick(&h.view.ctx.clone(), photo, Some(1));
    h.step();
    let r = h.request("ui.clickWidget", json!({"id": "slider:retouch.background.blur", "fx": 0.75}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let d = h.app.session.develop_of(photo).unwrap();
    assert!(d.retouch.global.background.blur > 40.0, "Blur set for the photo: {}", d.retouch.global.background.blur);
    assert!(d.retouch.faces.is_empty(), "no face got its own Background");
    // a swatch, a picture file, a photo of the view
    click(&mut h, "portrait:background:colour:FFFFFF");
    assert_eq!(h.app.session.develop_of(photo).unwrap().retouch.global.background.with, Swap::Colour { rgb: [1.0; 3] });
    let file = std::env::temp_dir().join(format!("lightcraft-backdrop-ui-{}.png", std::process::id()));
    let img = lightcraft_raster::Rgba8::filled(8, 6, [40, 90, 160, 255]);
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(&file, png).unwrap();
    let path = file.to_string_lossy().into_owned();
    let r = h.request("engine.execute", json!({"command": "portrait.backgroundPicture", "params": {"path": path}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let with = h.app.session.develop_of(photo).unwrap().retouch.global.background.with.clone();
    assert!(matches!(&with, Swap::Picture { hash } if hash.len() == 64), "kept by its hash: {with:?}");
    // a picture gone from the library: the engine says so (the section shows "Background Picture Missing")
    let mut d = (*h.app.session.develop_of(photo).unwrap()).clone();
    d.retouch.global.background.with = Swap::Picture { hash: "cd".repeat(32) };
    assert_eq!(h.app.session.backdrop_status(&d)["missing"], true);
    click(&mut h, "portrait:background:photos");
    h.step();
    let other = h.app.session.visible_cloned().into_iter().find(|id| *id != photo).unwrap();
    click(&mut h, &format!("portrait:background:photo:{}", other.0));
    assert_eq!(h.app.session.develop_of(photo).unwrap().retouch.global.background.with, Swap::Photo { id: other.0 });
    let _ = std::fs::remove_file(&file);
}
