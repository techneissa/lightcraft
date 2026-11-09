//! The Portrait tab's Portrait Presets section and the People view's menu (fork, `fork/portrait-presets.md`).

use std::sync::Arc;
use std::time::Duration;

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::retouch::{Face, NormRect};
use lightcraft_engine::retouch_input::FaceProvider;
use lightcraft_geom::Rect;
use lightcraft_meta::{Region, RegionKind};
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const ANN: (f64, f64, f64, f64) = (0.2, 0.2, 0.35, 0.45);
const BOB: (f64, f64, f64, f64) = (0.6, 0.25, 0.72, 0.45);

struct AnnAndBob;

impl FaceProvider for AnnAndBob {
    fn name(&self) -> &str {
        "Ann and Bob"
    }
    fn faces(&self, _img: &lightcraft_raster::Rgba8) -> Result<Vec<Face>, String> {
        let f = |b: (f64, f64, f64, f64), n: &str| Face { person: Some(n.into()), ..Face::boxed(NormRect::new(b.0, b.1, b.2, b.3)) };
        Ok(vec![f(ANN, "ann"), f(BOB, "bob")])
    }
}

fn named(s: &mut lightcraft_engine::Session, id: PhotoId, faces: &[((f64, f64, f64, f64), &str)]) {
    let mut meta = s.catalog.photo(id).unwrap().meta.clone();
    meta.regions = faces
        .iter()
        .map(|(b, n)| Region {
            rect: Rect { x0: b.0, y0: b.1, x1: b.2, y1: b.3 },
            kind: RegionKind::Face,
            name: Some((*n).into()),
            description: None,
        })
        .collect();
    s.catalog.apply(Op::SetMeta { id, meta: Box::new(meta) }).unwrap();
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "click {id}: {r}");
    h.step();
}

/// Pick Ann's face, type a name, save: a portrait preset that follows Ann. Her settings then go to every other
/// photo of her from the button under the presets, and a preset from the People view's menu.
#[test]
fn save_a_portrait_preset_from_a_face_and_apply_it_to_every_photo_of_the_person() {
    let mut session = lightcraft_engine::Session::with_demo();
    session.retouch_faces.set_provider(Arc::new(AnnAndBob));
    let ids = session.visible_cloned();
    named(&mut session, ids[0], &[(ANN, "Ann"), (BOB, "Bob")]);
    named(&mut session, ids[1], &[(BOB, "Ann")]);
    named(&mut session, ids[2], &[(ANN, "Bob")]);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    assert_eq!(h.request("ui.set", json!({"view": "detail"}), T)["ok"], true);
    h.app.ui.right = RightPanel::Portrait;
    let photo = h.app.session.active().unwrap();
    h.app.session.retouch_detect_now(photo, false).unwrap();
    h.app.session.execute("retouch.set", &json!({"control": "retouch.skin.smoothness", "value": 65, "face": 0})).unwrap();
    for _ in 0..3 {
        h.step();
    }
    click(&mut h, "portrait:face:0");
    click(&mut h, "section:portrait.presets");
    click(&mut h, "portrait:presetName");
    assert_eq!(h.request("ui.text", json!({"text": "Ann Look"}), T)["ok"], true);
    h.step();
    click(&mut h, "portrait:savePreset");
    let saved: Vec<_> = h.app.session.presets.iter().filter(|p| lightcraft_engine::portrait_presets::is_portrait(p)).cloned().collect();
    assert_eq!(saved.len(), 1, "one portrait preset saved");
    assert_eq!((saved[0].name.as_str(), saved[0].group.as_str()), ("Ann Look", "Portrait"));
    assert_eq!(saved[0].settings["fork_portrait"]["person"], "ann");
    click(&mut h, "portrait:applyToPerson");
    let own = |h: &Headless, id: PhotoId| h.app.session.develop_of(id).unwrap().retouch.faces.clone();
    assert_eq!(own(&h, ids[1]).len(), 1, "Ann's other photo got her settings");
    assert_eq!(own(&h, ids[1])[0].settings.get("retouch.skin.smoothness"), Some(65.0));
    assert!(own(&h, ids[2]).is_empty(), "a photo of Bob alone is left alone");
    // the People view: the preset to every photo of Ann, one undo step per photo
    named(&mut h.app.session, ids[3], &[(ANN, "Ann")]);
    let r = h.request("engine.execute", json!({"command": "view.person", "params": {"name": "Ann"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    let menu = h.request("ui.widgets", json!({"filter": "person:portraitPreset"}), T);
    assert!(menu["result"].as_array().is_some_and(|a| !a.is_empty()), "the person's page has the menu: {menu}");
    let r = h.app.session.execute("retouch.applyToPerson", &json!({"name": "Ann", "preset": saved[0].id})).unwrap();
    assert!(r["applied"].as_array().unwrap().contains(&json!(ids[3].0)), "{r}");
}
