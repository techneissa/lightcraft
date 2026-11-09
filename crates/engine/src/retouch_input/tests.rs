//! Portrait retouching in the engine: faces per photo, the settings through sidecar, catalog, copy / paste, sync and
//! presets, and the commands.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lightcraft_develop::retouch::{Face, NormRect};
use lightcraft_raster::Rgba8;
use serde_json::{Value, json};

use super::FaceProvider;
use crate::Session;

/// Gives the faces it holds, whatever the picture.
struct Fake(Mutex<Vec<Face>>);

impl FaceProvider for Fake {
    fn name(&self) -> &str {
        "Fake"
    }
    fn faces(&self, _img: &Rgba8) -> Result<Vec<Face>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
}

fn face(x0: f64, y0: f64, x1: f64, y1: f64, person: Option<&str>) -> Face {
    Face { person: person.map(str::to_string), ..Face::boxed(NormRect::new(x0, y0, x1, y1)) }
}

fn ann() -> Face {
    face(0.1, 0.2, 0.3, 0.5, Some("ann"))
}
fn bob() -> Face {
    face(0.6, 0.2, 0.8, 0.5, Some("bob"))
}

/// A demo session whose faces come from `faces`.
fn demo_with(faces: Vec<Face>) -> (Session, Arc<Fake>) {
    let mut s = Session::with_demo();
    let fake = Arc::new(Fake(Mutex::new(faces)));
    s.retouch_faces.set_provider(fake.clone());
    (s, fake)
}

fn retouch_of(s: &Session) -> lightcraft_develop::retouch::Retouch {
    s.develop_of(s.active().unwrap()).unwrap().retouch.clone()
}

#[test]
fn faces_are_found_by_the_provider_and_listed() {
    let (mut s, _) = demo_with(vec![ann(), face(0.4, 0.6, 0.5, 0.8, None)]);
    let r = s.execute("retouch.faces", &json!({})).unwrap();
    assert_eq!((r["found"].clone(), r["faces"].as_array().unwrap().len()), (json!(false), 0), "nothing starts by itself: {r}");
    let r = s.execute("retouch.detect", &json!({})).unwrap();
    assert_eq!(r["found"], true, "{r}");
    let faces = r["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 2);
    assert_eq!(faces[0]["person"], "ann");
    assert_eq!(faces[0]["bounds"], json!([0.1, 0.2, 0.3, 0.5]));
    assert_eq!(faces[1]["person"], Value::Null);
    let id = s.active().unwrap();
    let st = s.retouch_faces_of(id);
    assert_eq!(st.found.unwrap().thumbs.len(), 2, "a picker thumbnail per face");
}

#[test]
fn a_value_for_one_face_stays_with_its_person_when_faces_come_back_in_another_order() {
    let (mut s, fake) = demo_with(vec![ann(), bob()]);
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.skin.amount", "value": 20})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.skin.amount", "value": 70, "face": 1})).unwrap();
    let r = retouch_of(&s);
    assert_eq!(r.global.get("retouch.skin.amount"), Some(20.0));
    assert_eq!(r.faces.len(), 1);
    assert_eq!(r.faces[0].face.person.as_deref(), Some("bob"));
    // a new detection (of another crop of the scene) finds them the other way round: Ann where Bob was, Bob far left
    *fake.0.lock().unwrap() = vec![face(0.6, 0.2, 0.8, 0.5, Some("ann")), face(0.05, 0.22, 0.26, 0.52, Some("bob"))];
    let listed = s.execute("retouch.detect", &json!({"again": true})).unwrap();
    assert_eq!(listed["faces"][0]["person"], "bob");
    assert_eq!(listed["faces"][0]["own"], 0, "Bob keeps his own settings: {listed}");
    assert_eq!(listed["faces"][1]["own"], Value::Null, "Ann follows every face's: {listed}");
    let faces: Vec<Face> = s.retouch_faces_of(s.active().unwrap()).found.unwrap().input.faces.clone();
    let r = retouch_of(&s);
    assert_eq!(r.settings_for(&faces, 0).get("retouch.skin.amount"), Some(70.0));
    assert_eq!(r.settings_for(&faces, 1).get("retouch.skin.amount"), Some(20.0));
    // and the per-face value is a develop control too
    assert_eq!(s.execute("develop.set", &json!({"control": "retouchFace.0.skin.amount", "value": 15})).unwrap(), Value::Null);
    assert_eq!(retouch_of(&s).faces[0].settings.get("retouch.skin.amount"), Some(15.0));
    // a face index past the list, a bad control, an unknown feature: errors, nothing changes
    let before = retouch_of(&s);
    assert!(s.execute("retouch.set", &json!({"control": "retouch.skin.amount", "value": 1, "face": 9})).is_err());
    assert!(s.execute("retouch.set", &json!({"control": "light.exposure", "value": 1})).is_err());
    assert!(s.execute("retouch.set", &json!({"control": "retouchFace.0.skin.amount", "value": 1})).is_err());
    assert!(s.execute("retouch.set", &json!({"control": "retouch.skin.amount", "value": 1, "face": -1})).is_err());
    assert!(s.execute("retouch.reset", &json!({"feature": "nope"})).is_err());
    assert_eq!(retouch_of(&s), before);
    // back to every face's settings, then all off
    assert_eq!(s.execute("retouch.resetFace", &json!({"face": 0})).unwrap()["removed"], true);
    assert!(retouch_of(&s).faces.is_empty());
    s.execute("retouch.reset", &json!({})).unwrap();
    assert!(retouch_of(&s).is_default());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(retouch_of(&s).global.get("retouch.skin.amount"), Some(20.0), "one undo step each");
}

#[test]
fn copy_paste_sync_and_presets_carry_the_block_and_the_faces() {
    let (mut s, _) = demo_with(vec![ann(), bob()]);
    let vis = s.visible_cloned();
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.eyes.amount", "value": 30})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.skin.amount", "value": 55, "face": 0})).unwrap();
    let want = retouch_of(&s);
    assert_eq!(want.faces.len(), 1);
    // copy / paste with Portrait ticked (it is not in the default copy groups)
    s.execute("develop.copy", &json!({"groups": ["portrait"]})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[1].0]})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    assert_eq!(s.develop_of(vis[1]).unwrap().retouch, want);
    // sync from the active photo to the others selected
    s.execute("library.select", &json!({"ids": [vis[0].0, vis[2].0]})).unwrap();
    s.execute("develop.sync", &json!({"groups": ["portrait"]})).unwrap();
    assert_eq!(s.develop_of(vis[2]).unwrap().retouch, want);
    // a preset made of the Portrait group
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    let pid = s.execute("preset.create", &json!({"name": "Portrait Look", "groups": ["portrait"]})).unwrap()["id"].as_str().unwrap().to_string();
    s.execute("library.select", &json!({"ids": [vis[3].0]})).unwrap();
    s.execute("preset.apply", &json!({"id": pid})).unwrap();
    assert_eq!(s.develop_of(vis[3]).unwrap().retouch, want);
    // pasting an unretouched photo's Portrait settings turns retouching off
    s.execute("library.select", &json!({"ids": [vis[4].0]})).unwrap();
    s.execute("develop.copy", &json!({"groups": ["portrait"]})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[3].0]})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    assert!(s.develop_of(vis[3]).unwrap().retouch.is_default());
}

/// Portrait holds one photo's face edits, like masks and spots: the default copy and Auto Sync leave it.
#[test]
fn default_copy_and_auto_sync_leave_the_portrait_settings_alone() {
    let (mut s, _) = demo_with(vec![ann()]);
    let vis = s.visible_cloned();
    s.execute("library.select", &json!({"ids": [vis[0].0, vis[1].0]})).unwrap();
    s.execute("develop.autoSync", &json!({"on": true})).unwrap();
    s.execute("develop.set", &json!({"control": "retouch.skin.amount", "value": 40})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    assert_eq!(s.develop_of(vis[0]).unwrap().retouch.global.get("retouch.skin.amount"), Some(40.0));
    let other = s.develop_of(vis[1]).unwrap();
    assert_eq!(other.light.exposure, 0.5, "Auto Sync is on");
    assert!(other.retouch.is_default(), "Auto Sync does not carry face edits");
    s.execute("develop.autoSync", &json!({"on": false})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[0].0]})).unwrap();
    s.execute("develop.copy", &json!({})).unwrap();
    s.execute("library.select", &json!({"ids": [vis[2].0]})).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    assert_eq!(s.develop_of(vis[2]).unwrap().light.exposure, 0.5);
    assert!(s.develop_of(vis[2]).unwrap().retouch.is_default(), "not in the default copy");
}

#[test]
fn the_block_round_trips_through_the_sidecar_and_the_catalog() {
    let src = crate::tests::xmp::temp_dir("retouch-src");
    let lib = crate::tests::xmp::temp_dir("retouch-lib");
    let png = src.join("portrait.png");
    let img = Rgba8::from_fn(40, 24, |x, y| [(x * 6) as u8, (y * 9) as u8, 90, 255]);
    std::fs::write(
        &png,
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap(),
    )
    .unwrap();
    let other = Rgba8::from_fn(40, 24, |x, y| [(y * 6) as u8, (x * 5) as u8, 30, 255]);
    std::fs::write(
        src.join("plain.png"),
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&other), &Default::default()).unwrap(),
    )
    .unwrap();
    let mut s = Session::new().with_fs();
    s.retouch_faces.set_provider(Arc::new(Fake(Mutex::new(vec![ann()]))));
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [png.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.teethLips.amount", "value": 40})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.eyes.amount", "value": 65, "face": 0})).unwrap();
    let want = retouch_of(&s);
    assert!(!want.is_off() && want.faces.len() == 1);
    s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let xmp = std::fs::read_to_string(src.join("portrait.xmp")).unwrap();
    assert!(xmp.contains("fork_retouch"), "in lc:settings, under the fork's own key");
    drop(s);

    // the catalog keeps it
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().develop.retouch, want, "catalog round trip");
    drop(s);

    // another app (Lightroom) rewrites the sidecar: its own settings change, ours stay; a fresh library reads them
    let rewritten = xmp.replacen(
        "<rdf:Description ",
        "<rdf:Description xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" crs:Exposure2012=\"+0.50\" ",
        1,
    );
    std::fs::write(src.join("portrait.xmp"), rewritten).unwrap();
    std::fs::remove_dir_all(&lib).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let p = s.catalog.photos().find(|p| p.file_name == "portrait.png").unwrap();
    assert_eq!(p.develop.retouch, want, "sidecar round trip");
    let plain = s.catalog.photos().find(|p| p.file_name == "plain.png").unwrap();
    assert!(plain.develop.retouch.is_default() && plain.develop.is_unedited(), "a photo without the block reads as off");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn an_all_off_block_renders_like_no_block_and_adds_nothing_to_the_keys() {
    let (mut s, _) = demo_with(vec![ann()]);
    let id = s.active().unwrap();
    let plain = s.render_now(id, 96, 96).unwrap().image;
    s.execute("retouch.detect", &json!({})).unwrap();
    // a face with its own settings, all off
    s.execute("retouch.set", &json!({"control": "retouch.skin.amount", "value": 0, "face": 0})).unwrap();
    assert!(!retouch_of(&s).is_default() && retouch_of(&s).is_off());
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!(s.retouch_salt(&p, &p.develop), 0);
    assert!(s.render_job(id, 96, 96, false, true).unwrap().info.retouch.is_none(), "an off block gets no faces");
    assert_eq!(s.render_now(id, 96, 96).unwrap().image, plain, "byte-identical");
}

#[test]
fn a_retouched_photo_gets_its_faces_in_the_background_with_an_activity_row() {
    struct Slow(Arc<AtomicBool>);
    impl FaceProvider for Slow {
        fn name(&self) -> &str {
            "Slow"
        }
        fn faces(&self, _img: &Rgba8) -> Result<Vec<Face>, String> {
            while !self.0.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            Ok(vec![ann()])
        }
    }
    let go = Arc::new(AtomicBool::new(false));
    let mut s = Session::with_demo();
    s.retouch_faces.set_provider(Arc::new(Slow(go.clone())));
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "retouch.skin.amount", "value": 40})).unwrap();
    let first = s.render_job(id, 64, 64, false, true).unwrap();
    assert!(first.info.retouch.is_none(), "renders don't wait");
    assert!(s.retouch_faces_of(id).busy);
    let rows = s.execute("activity.list", &json!({})).unwrap();
    assert!(rows["tasks"].as_array().unwrap().iter().any(|t| t["kind"] == "retouch.faces"), "{rows}");
    // a second render starts no second search
    let _ = s.render_job(id, 64, 64, false, true).unwrap();
    go.store(true, Ordering::SeqCst);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while s.retouch_faces_of(id).busy {
        assert!(std::time::Instant::now() < until, "the search never ended");
        std::thread::yield_now();
    }
    let later = s.render_job(id, 64, 64, false, true).unwrap();
    assert_eq!(later.info.retouch.as_ref().map(|i| i.faces.len()), Some(1));
    assert_ne!(later.key, first.key, "the preview made without faces is redrawn");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while s.execute("activity.list", &json!({})).unwrap()["tasks"].as_array().unwrap().iter().any(|t| t["kind"] == "retouch.faces") {
        assert!(std::time::Instant::now() < until, "the row stayed");
        std::thread::yield_now();
    }
}

#[test]
fn no_panic_with_no_faces_fifty_faces_a_failing_provider_or_a_one_pixel_photo() {
    struct Failing;
    impl FaceProvider for Failing {
        fn name(&self) -> &str {
            "Failing"
        }
        fn faces(&self, _img: &Rgba8) -> Result<Vec<Face>, String> {
            Err("the model is damaged".into())
        }
    }
    let fifty: Vec<Face> = (0..50)
        .map(|i| face((i % 10) as f64 / 10.0, (i / 10) as f64 / 5.0, (i % 10) as f64 / 10.0 + 0.09, (i / 10) as f64 / 5.0 + 0.19, None))
        .collect();
    for faces in [Vec::new(), fifty] {
        let (mut s, _) = demo_with(faces.clone());
        let id = s.active().unwrap();
        for spec in lightcraft_develop::retouch::specs() {
            s.execute("develop.set", &json!({"control": spec.id, "value": spec.max})).unwrap();
        }
        let r = s.render_now(id, 64, 64).unwrap();
        assert!(r.image.width > 0);
        assert_eq!(s.execute("retouch.faces", &json!({})).unwrap()["faces"].as_array().unwrap().len(), faces.len());
        if let Some(n) = faces.len().checked_sub(1) {
            s.execute("retouch.set", &json!({"control": "retouch.hair.amount", "value": 10, "face": n})).unwrap();
            assert!(s.render_now(id, 64, 64).is_ok());
        } else {
            assert!(s.execute("retouch.set", &json!({"control": "retouch.hair.amount", "value": 10, "face": 0})).is_err());
        }
    }
    // a provider that fails: the photo renders without faces, the failure is reported
    let mut s = Session::with_demo();
    s.retouch_faces.set_provider(Arc::new(Failing));
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "retouch.skin.amount", "value": 40})).unwrap();
    assert!(s.render_now(id, 32, 32).is_ok());
    assert!(s.execute("retouch.detect", &json!({})).is_err());
    assert_eq!(s.retouch_faces_of(id).error.as_deref(), Some("the model is damaged"));
    // without a face model, detect says how to get one
    let mut s = Session::with_demo();
    let e = s.execute("retouch.detect", &json!({})).unwrap_err();
    assert!(e.to_string().contains("YuNet") || e.to_string().contains("face"), "{e}");
    // a one-pixel photo
    let src = crate::tests::xmp::temp_dir("retouch-tiny");
    let png = src.join("dot.png");
    let dot = Rgba8::from_fn(1, 1, |_, _| [200, 150, 120, 255]);
    std::fs::write(&png, lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&dot), &Default::default()).unwrap()).unwrap();
    let mut s = Session::new().with_fs();
    s.retouch_faces.set_provider(Arc::new(Fake(Mutex::new(vec![face(0.0, 0.0, 1.0, 1.0, None)]))));
    s.execute("library.import", &json!({"paths": [png.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "retouch.skin.amount", "value": 100})).unwrap();
    assert!(s.render_now(id, 16, 16).is_ok());
    assert_eq!(s.execute("retouch.detect", &json!({})).unwrap()["faces"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&src);
}

/// No face model is not a failure of the photo: once a model is there, the next search runs (no restart).
#[test]
fn a_missing_face_model_is_not_kept_as_a_failure() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let _ = s.retouch_search(id, false);
    let st = s.retouch_faces_of(id);
    assert!(st.error.is_none() && !st.busy && st.found.is_none());
    // the model arrives (as installing YuNet does: nothing else is reset)
    s.retouch_faces.provider = Some(Arc::new(Fake(Mutex::new(vec![ann()]))));
    let _ = s.retouch_search(id, false);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while s.retouch_faces_of(id).found.is_none() {
        assert!(std::time::Instant::now() < until, "the search never ran");
        std::thread::yield_now();
    }
}

/// A background export is set up on the UI thread: it must not wait there for the faces. The export finds them
/// when it runs (on its worker, under its activity row), so the result still has them.
#[test]
fn a_background_export_finds_the_faces_when_it_runs_not_when_it_is_set_up() {
    use std::sync::atomic::AtomicUsize;
    struct Gate {
        go: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }
    impl FaceProvider for Gate {
        fn name(&self) -> &str {
            "Gate"
        }
        fn faces(&self, _img: &Rgba8) -> Result<Vec<Face>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // (a hang guard, not a timing check: set-up never gets here once fixed)
            let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while !self.go.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                std::thread::yield_now();
            }
            Ok(vec![ann()])
        }
    }
    let (go, calls) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let mut s = Session::with_demo();
    s.retouch_faces.set_provider(Arc::new(Gate { go: go.clone(), calls: calls.clone() }));
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "retouch.skin.amount", "value": 40})).unwrap();
    let opts = crate::export::ExportOptions { resize: Some(crate::export::Resize::long_edge(64)), ..Default::default() };
    let items = crate::export::prepare_batch(&mut s, &[id], &opts).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0, "setting up the export ran no face search");
    go.store(true, Ordering::SeqCst);
    for item in items {
        item.run().unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "running it did, once");
    assert_eq!(s.retouch_faces_of(id).found.map(|f| f.input.faces.len()), Some(1), "and kept the faces for the previews");
    assert!(!s.retouch_faces_of(id).busy);
}

/// A face's own settings are kept by its box on the photo before the user's rotation: Rotate Right moves every
/// face, and the settings stay with their face, never with another face that lands where it was.
#[test]
fn a_faces_own_settings_survive_a_rotation() {
    let rect = |b: NormRect| lightcraft_geom::Rect { x0: b.x0, y0: b.y0, x1: b.x1, y1: b.y1 };
    let back = |r: lightcraft_geom::Rect| NormRect::new(r.x0, r.y0, r.x1, r.y1);
    let a = NormRect::new(0.1, 0.1, 0.3, 0.4);
    let (mut s, fake) = demo_with(vec![face(a.x0, a.y0, a.x1, a.y1, None)]);
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("retouch.set", &json!({"control": "retouch.eyes.amount", "value": 60, "face": 0})).unwrap();
    s.execute("photo.rotateRight", &json!({})).unwrap();
    let o = s.develop_of(s.active().unwrap()).unwrap().orientation;
    assert_ne!(o, lightcraft_geom::Orientation::Normal);
    // the detector now sees face A turned, and face B (unturned somewhere else) exactly where A was
    let a_now = back(o.map_norm_rect(rect(a)));
    let b_before = back(o.inverse().map_norm_rect(rect(a)));
    let b_now = back(o.map_norm_rect(rect(b_before)));
    assert!((b_now.x0 - a.x0).abs() < 1e-9 && (b_now.y1 - a.y1).abs() < 1e-9, "B lands on A's old box: {b_now:?}");
    *fake.0.lock().unwrap() = vec![face(a_now.x0, a_now.y0, a_now.x1, a_now.y1, None), face(b_now.x0, b_now.y0, b_now.x1, b_now.y1, None)];
    let listed = s.execute("retouch.detect", &json!({"again": true})).unwrap();
    let own: Vec<(Value, Value)> = listed["faces"].as_array().unwrap().iter().map(|f| (f["bounds"][0].clone(), f["own"].clone())).collect();
    let a_own = own.iter().find(|(x0, _)| (x0.as_f64().unwrap() - a_now.x0).abs() < 1e-6).map(|(_, o)| o.clone());
    let b_own = own.iter().find(|(x0, _)| (x0.as_f64().unwrap() - b_now.x0).abs() < 1e-6).map(|(_, o)| o.clone());
    assert_eq!(a_own, Some(json!(0)), "A keeps its settings: {listed}");
    assert_eq!(b_own, Some(Value::Null), "B never takes them: {listed}");
}

/// Browsing past [`super::KEEP`] retouched photos must not drop the faces of the ones in use: the cache forgets the
/// photo used least recently.
#[test]
fn the_face_cache_forgets_the_photo_used_least_recently() {
    let store = super::FaceStore::default();
    let found = || super::Found { input: Arc::new(lightcraft_develop::retouch::Input::default()), thumbs: Arc::new(Vec::new()) };
    for i in 0..super::KEEP {
        store.put(&format!("photo {i}"), Ok(found()));
    }
    assert!(store.get("photo 0").is_some(), "the oldest, used again");
    store.put("one more", Ok(found()));
    assert!(store.get("photo 0").is_some(), "kept: it was used last but one");
    assert!(store.get("photo 1").is_none(), "the one used least recently goes");
    assert!(store.get("one more").is_some());
}

#[test]
fn faces_take_the_names_of_the_regions_they_overlap() {
    let mut faces = vec![face(0.1, 0.1, 0.3, 0.4, None), face(0.6, 0.1, 0.8, 0.4, None), face(0.4, 0.6, 0.45, 0.65, None)];
    let regions = vec![(NormRect::new(0.62, 0.12, 0.81, 0.42), "bob".to_string()), (NormRect::new(0.09, 0.1, 0.3, 0.38), "ann".to_string())];
    super::name_faces(&mut faces, &regions);
    let names: Vec<Option<&str>> = faces.iter().map(|f| f.person.as_deref()).collect();
    assert_eq!(names, [Some("ann"), Some("bob"), None]);
    super::name_faces(&mut [], &regions);
    super::name_faces(&mut faces, &[(NormRect::new(f64::NAN, 0.0, 1.0, 1.0), "x".into())]);
}
