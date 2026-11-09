//! The Portrait tab (fork, `fork/retouch-foundation.md`): its feature sections and its face picker.

use std::sync::Arc;
use std::time::Duration;

use lightcraft_develop::retouch::{FEATURES, Face, NormRect};
use lightcraft_engine::retouch_input::FaceProvider;
use serde_json::json;

use crate::headless::Headless;
use crate::panels::portrait::picked;
use crate::state::RightPanel;
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

/// The Portrait tab: its strip icon and shortcut open it, it finds the photo's faces, has a section per feature and
/// Portrait Presets (closed until opened, kept in `openSections`), says which faces its sliders change, shows the
/// face picker on the photo while it is open, and its eye turns retouching off.
#[test]
fn the_portrait_tab_has_a_section_per_feature_and_shows_the_face_picker() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Edit;
    h.step();
    h.step();
    assert!(rect_of(&mut h, "portrait:card").is_none(), "no face picker in the Edit panel");
    click(&mut h, "icon:portrait");
    assert_eq!(h.app.ui.right, RightPanel::Portrait, "the strip icon opens the tab");
    assert!(crate::menus::UI_COMMANDS.iter().any(|c| c.0 == "panel.portrait" && c.2 == Some("T")), "shortcut T");
    // opening the tab finds the faces, in the background
    let photo = h.app.session.active().unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while h.app.session.retouch_faces_of(photo).found.is_none() {
        assert!(std::time::Instant::now() < until, "the faces were never found");
        h.step();
    }
    h.step();
    h.step();
    let keys: Vec<&str> = FEATURES.iter().map(|f| f.key).chain(["presets"]).collect();
    for key in &keys {
        let id = format!("portrait.{key}");
        assert!(!h.app.ui.section_open(&id), "{key} starts closed");
        click(&mut h, &format!("section:{id}"));
        assert!(h.app.ui.section_open(&id), "{key} opens");
        click(&mut h, &format!("section:{id}"));
        assert!(!h.app.ui.section_open(&id), "{key} closes");
    }
    let r = h.request("engine.execute", json!({"command": "portrait.section", "params": {"section": "nope"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    // the tab says which faces the sliders change; the picker is on the photo
    assert!(rect_of(&mut h, "portrait:editing").is_some());
    assert!(rect_of(&mut h, "portrait:card").is_some(), "the face picker shows while the tab is open");
    let ctx = h.view.ctx.clone();
    assert_eq!(picked(&ctx, photo), None, "All by default");
    click(&mut h, "portrait:face:1");
    assert_eq!(picked(&ctx, photo), Some(1));
    click(&mut h, "portrait:face:all");
    assert_eq!(picked(&ctx, photo), None);
    // nothing about the photo changed by looking
    assert!(h.app.session.develop_of(photo).unwrap().retouch.is_default());
    // the eye turns retouching off for the photo, and on again
    click(&mut h, "portrait:switch");
    assert!(!h.app.session.develop_of(photo).unwrap().section_enabled("portrait"));
    click(&mut h, "portrait:switch");
    assert!(h.app.session.develop_of(photo).unwrap().section_enabled("portrait"));
    // another tab: no picker
    click(&mut h, "icon:edit");
    h.step();
    assert!(rect_of(&mut h, "portrait:card").is_none());
}

/// The Skin Tone section (`portrait/skin_tone.rs`): Even Tone, Redness and Face Light; a click on a track sets the
/// value for every face (Face Light is centred: a click right of the middle lightens).
#[test]
fn the_skin_tone_section_has_its_three_sliders() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    click(&mut h, "icon:portrait");
    click(&mut h, "section:portrait.skinTone");
    h.step();
    let photo = h.app.session.active().unwrap();
    for id in ["retouch.skinTone.evenTone", "retouch.skinTone.redness", "retouch.skinTone.faceLight"] {
        assert!(rect_of(&mut h, &format!("slider:{id}")).is_some(), "{id} is in the Skin Tone section");
    }
    for (id, least) in [("retouch.skinTone.evenTone", 40.0), ("retouch.skinTone.faceLight", 20.0)] {
        let r = h.request("ui.clickWidget", json!({"id": format!("slider:{id}"), "fx": 0.75}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        let v = h.app.session.develop_of(photo).unwrap().retouch.global.get(id).unwrap_or(0.0);
        assert!(v > least, "{id} set by the click: {v}");
    }
}

fn rect_of(h: &mut Headless, id: &str) -> Option<[f64; 4]> {
    let r = h.request("ui.widgets", json!({"filter": id}), T);
    r["result"].as_array().into_iter().flatten().find(|w| w["id"] == id).and_then(|w| serde_json::from_value(w["rect"].clone()).ok())
}

/// The face picker sits at the image view's top-right corner, a fixed margin from its edges: the same place for
/// photos of other shapes and at any zoom. It picks faces, folds into the overflow button there (remembered), and
/// hides with the tool overlay.
#[test]
fn the_face_picker_keeps_to_the_views_corner_picks_folds_and_hides() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Portrait;
    let settle = |h: &mut Headless| {
        for _ in 0..30 {
            h.step();
        }
    };
    let ids = h.app.session.visible_cloned();
    let (wide, tall) = (ids[0], ids[1]);
    // the second photo cropped tall: another shape on screen
    let mut d = (*h.app.session.develop_of(tall).unwrap()).clone();
    d.crop.geometry.rect = lightcraft_geom::Rect { x0: 0.35, y0: 0.0, x1: 0.6, y1: 1.0 };
    h.app.session.set_develop(tall, d, "Crop").unwrap();
    for id in [wide, tall] {
        h.app.session.retouch_detect_now(id, false).unwrap();
    }
    let mut cards = Vec::new();
    for id in [wide, tall] {
        h.app.session.execute("library.select", &json!({"ids": [id.0]})).unwrap();
        settle(&mut h);
        cards.push((rect_of(&mut h, "portrait:card").expect("the picker is on the loupe"), rect_of(&mut h, "canvas:image").unwrap()));
    }
    let r = h.request("engine.execute", json!({"command": "view.zoom100", "params": {}}), T);
    assert_eq!(r["ok"], true, "{r}");
    settle(&mut h);
    let zoomed = rect_of(&mut h, "portrait:card").unwrap();
    let canvas = h.app.canvas_rect.unwrap();
    let corner = [f64::from(canvas.right()) - 12.0, f64::from(canvas.top()) + 12.0];
    assert_ne!(cards[0].1[2] / cards[0].1[3], cards[1].1[2] / cards[1].1[3], "the photos have other shapes");
    for card in [cards[0].0, cards[1].0, zoomed] {
        assert!((card[0] + card[2] - corner[0]).abs() < 0.5 && (card[1] - corner[1]).abs() < 0.5, "{card:?} at {corner:?}");
    }
    let ctx = h.view.ctx.clone();
    click(&mut h, "portrait:face:0");
    assert_eq!(picked(&ctx, tall), Some(0), "picked on the loupe");
    // folds into the overflow button at the same corner, remembered in the saved state; it opens again
    click(&mut h, "portrait:more");
    assert!(h.app.ui.flyout_open(crate::panels::portrait::FOLDED));
    assert!(rect_of(&mut h, "portrait:face:0").is_none(), "folded: no faces");
    let folded = rect_of(&mut h, "portrait:card").unwrap();
    assert!((folded[0] + folded[2] - corner[0]).abs() < 0.5 && (folded[1] - corner[1]).abs() < 0.5 && folded[2] < 50.0, "{folded:?}");
    click(&mut h, "portrait:more");
    assert!(rect_of(&mut h, "portrait:face:0").is_some());
    // the tool overlay off (Shift+H to Never) hides it; the tab closed hides it
    h.app.ui.tool_overlay = crate::state::ToolOverlay::Never;
    h.step();
    assert!(rect_of(&mut h, "portrait:card").is_none());
    h.app.ui.tool_overlay = crate::state::ToolOverlay::Auto;
    h.app.ui.right = RightPanel::Edit;
    h.step();
    assert!(rect_of(&mut h, "portrait:card").is_none());
}

/// The Skin section (`portrait/skin.rs`): Smoothness, Texture and Shine; a click on a track sets the value for every
/// face.
#[test]
fn the_skin_section_has_its_three_sliders() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    click(&mut h, "icon:portrait");
    click(&mut h, "section:portrait.skin");
    h.step();
    let photo = h.app.session.active().unwrap();
    for id in ["retouch.skin.smoothness", "retouch.skin.texture", "retouch.skin.shine"] {
        assert!(rect_of(&mut h, &format!("slider:{id}")).is_some(), "{id} is in the Skin section");
    }
    let r = h.request("ui.clickWidget", json!({"id": "slider:retouch.skin.smoothness", "fx": 0.75}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let v = h.app.session.develop_of(photo).unwrap().retouch.global.get("retouch.skin.smoothness").unwrap_or(0.0);
    assert!(v > 40.0, "Smoothness set by the click: {v}");
}

/// One face whose provider found three spots (two blemishes and a mole) on its skin.
struct SpottedFace;

impl FaceProvider for SpottedFace {
    fn name(&self) -> &str {
        "Spotted Face"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        use lightcraft_develop::retouch::input::{Spot, SpotKind, Spots};
        use lightcraft_develop::retouch::{Part, PartMask};
        let b = NormRect::new(0.3, 0.2, 0.6, 0.6);
        let spot = |x, y, kind| Spot { at: (x, y), radius: 0.01, donor: None, kind, strength: 1.0 };
        let list = vec![spot(0.4, 0.3, SpotKind::Blemish), spot(0.5, 0.4, SpotKind::Blemish), spot(0.45, 0.5, SpotKind::Mole)];
        Ok(vec![Face {
            masks: vec![(Part::FacialSkin, PartMask { plane: lightcraft_raster::Plane::filled(8, 8, 1.0), area: b })],
            spots: Some(Spots { list, skin: 0.5 }),
            ..Face::boxed(b)
        }])
    }
}

/// The Blemishes section (`portrait/blemish.rs`): Amount, the Keep Moles and Freckles switch (on by default; a click
/// turns it off for every face) and how many spots the settings remove and keep.
#[test]
fn the_blemishes_section_has_amount_the_switch_and_the_report() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(SpottedFace));
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    // (what a screenshot needs)
    let services = Services { png: Some(Box::new(png)), write_new: Some(Box::new(lightcraft_engine::export::write_file_new)), ..Default::default() };
    let app = LightcraftApp::new(session, services);
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    click(&mut h, "icon:portrait");
    click(&mut h, "section:portrait.blemish");
    let photo = h.app.session.active().unwrap();
    // (a hang guard, not a timing check)
    let until = std::time::Instant::now() + T;
    while h.app.session.retouch_faces_of(photo).found.is_none() {
        assert!(std::time::Instant::now() < until, "the faces were never found");
        h.step();
    }
    h.step();
    h.step();
    assert!(rect_of(&mut h, "slider:retouch.blemish.amount").is_some());
    assert!(rect_of(&mut h, "portrait:blemish.report").is_some(), "the report shows once the spots are found");
    let r = h.request("ui.clickWidget", json!({"id": "slider:retouch.blemish.amount", "fx": 0.9}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let d = h.app.session.develop_of(photo).unwrap();
    assert!(d.retouch.global.blemish.amount > 60.0 && d.retouch.global.blemish.keep_moles);
    let input = h.app.session.retouch_faces_of(photo).found.unwrap().input;
    let report = d.retouch.global.blemish.report(input.faces[0].spots.as_ref().unwrap());
    assert_eq!((report.removed, report.moles), (2, 1), "the mole is kept");
    if let Some(p) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        h.step();
        let r =
            h.request("ui.screenshot", json!({"path": std::path::Path::new(&p).join("blemish-section.png").to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    click(&mut h, "portrait:blemish.keepMoles");
    h.step();
    assert!(!h.app.session.develop_of(photo).unwrap().retouch.global.blemish.keep_moles, "the click turns the switch off");
}

/// The Face Reshape section (`portrait/reshape.rs`): its seven −100…+100 sliders; a click right of the middle sets
/// Slim Face for every face.
#[test]
fn the_face_reshape_section_has_its_seven_sliders() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(TwoFaces));
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1600.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    click(&mut h, "icon:portrait");
    click(&mut h, "section:portrait.reshape");
    h.step();
    let photo = h.app.session.active().unwrap();
    for f in ["slimFace", "jaw", "chin", "doubleChin", "nose", "forehead", "eyeSize"] {
        assert!(rect_of(&mut h, &format!("slider:retouch.reshape.{f}")).is_some(), "{f} is in the Face Reshape section");
    }
    let r = h.request("ui.clickWidget", json!({"id": "slider:retouch.reshape.slimFace", "fx": 0.75}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let v = h.app.session.develop_of(photo).unwrap().retouch.global.get("retouch.reshape.slimFace").unwrap_or(0.0);
    assert!(v > 30.0 && v < 70.0, "Slim Face set by the click: {v}");
}
