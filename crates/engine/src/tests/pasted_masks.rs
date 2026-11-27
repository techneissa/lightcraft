//! A mask pasted from another photo is an ordinary mask on the photo, and Reset after a paste leaves the photo as
//! unedited (issue #355).

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape, SegMask, SettingsGroup};
use lightcraft_geom::Point;
use serde_json::json;

use crate::Session;

fn part(shape: MaskShape) -> MaskComponent {
    MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }
}

fn found() -> Option<SegMask> {
    Some(SegMask::from_logits(2, &[5.0, -5.0, -5.0, 5.0]))
}

/// One mask of each kind, each with an effect.
fn every_kind() -> Vec<Mask> {
    let shapes = [
        MaskShape::Brush { strokes: vec![] },
        MaskShape::Linear { start: Point::new(0.2, 0.2), end: Point::new(0.8, 0.8) },
        MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 0.3, ry: 0.2, angle: 0.0, feather: 50.0, invert: false },
        MaskShape::Subject { seg: found(), detail: vec![], edge: 0.0 },
        MaskShape::Sky { seg: found(), detail: vec![], edge: 0.0 },
        MaskShape::Background { seg: found(), detail: vec![], edge: 0.0 },
    ];
    shapes
        .into_iter()
        .zip(1..)
        .map(|(shape, id)| {
            let mut m = Mask { id, name: format!("Mask {id}"), components: vec![part(shape)], ..Default::default() };
            m.adjust.exposure = 1.0;
            m
        })
        .collect()
}

/// Photo A with a look and masks of every kind, B and C untouched; B is the active photo.
fn session() -> (Session, [PhotoId; 3]) {
    let mut s = Session::with_demo();
    let mut ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    ids.sort();
    let ids = [ids[0], ids[1], ids[2]];
    let mut d = (*s.develop_of(ids[0]).unwrap()).clone();
    d.light.exposure = 1.0;
    d.masks = every_kind();
    s.set_develop(ids[0], d, "Look").unwrap();
    s.execute("library.select", &json!({"ids": [ids[0].0], "active": ids[0].0})).unwrap();
    s.execute("develop.copy", &json!({"groups": SettingsGroup::ALL})).unwrap();
    s.execute("library.select", &json!({"ids": [ids[1].0], "active": ids[1].0})).unwrap();
    (s, ids)
}

/// Pasted masks of every kind can be hidden, deleted and undone like masks made on the photo.
#[test]
fn a_pasted_mask_is_an_ordinary_mask() {
    let (mut s, [_, b, _]) = session();
    let before = s.develop_of(b).unwrap();
    s.execute("develop.paste", &json!({})).unwrap();
    let pasted = s.develop_of(b).unwrap().masks.clone();
    assert_eq!(pasted.len(), 6);
    for m in &pasted {
        s.execute("mask.select", &json!({"id": m.id})).unwrap();
        s.execute("mask.visible", &json!({"id": m.id})).unwrap();
        assert!(!s.develop_of(b).unwrap().masks.iter().find(|x| x.id == m.id).unwrap().visible, "{} hides", m.name);
        s.execute("edit.undo", &json!({})).unwrap();
        s.execute("mask.delete", &json!({"id": m.id})).unwrap();
        assert!(s.develop_of(b).unwrap().masks.iter().all(|x| x.id != m.id), "{} deleted", m.name);
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.develop_of(b).unwrap().masks.iter().any(|x| x.id == m.id), "{} back after undo", m.name);
    }
    s.execute("mask.deleteAll", &json!({})).unwrap();
    assert!(s.develop_of(b).unwrap().masks.is_empty());
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.develop_of(b).unwrap(), before, "undoing the paste removes its masks");
}

/// Paste then Reset: the photo's settings and its render are what they were after a Reset before the paste.
#[test]
fn reset_after_a_paste_leaves_the_photo_unedited() {
    let (mut s, [_, b, _]) = session();
    s.execute("develop.reset", &json!({})).unwrap();
    let (plain, plain_look) = (s.develop_of(b).unwrap(), s.render_now(b, 96, 96).unwrap().image);
    s.execute("develop.paste", &json!({})).unwrap();
    let pasted = s.render_now(b, 96, 96).unwrap().image;
    assert_ne!(pasted.as_bytes(), plain_look.as_bytes(), "the paste changed the picture");
    s.execute("develop.reset", &json!({})).unwrap();
    let reset = s.develop_of(b).unwrap();
    assert_eq!(reset.to_json(), plain.to_json(), "the settings are an unedited photo's");
    assert_eq!(reset.hash64(), plain.hash64());
    assert_eq!(s.render_now(b, 96, 96).unwrap().image.as_bytes(), plain_look.as_bytes(), "and Reset took the picture back");
}

/// The A7 V raw of the corpus (`cargo xtask corpus --download`), when it is there.
fn a7v_raw() -> Option<std::path::PathBuf> {
    let root = std::env::var_os("LIGHTCRAFT_CORPUS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
    Some(root.join("raw/arw-sony-ilce-7m5.arw")).filter(|p| p.exists())
}

/// A new Sony raw as the import makes it on this Mac: on macOS Colour, as its import look.
fn sony(id: u64, path: &std::path::Path) -> lightcraft_catalog::Photo {
    use lightcraft_catalog::{MediaKind, Photo, Source};
    let mut p = Photo::new(PhotoId(id), Source::File { path: path.display().to_string() }, "a.arw", "ARW", 7008, 4672, "2026-11-20");
    p.kind = MediaKind::Raw;
    p.meta.camera = "SONY ILCE-7M5".into();
    let mut d = p.camera_defaults();
    d.profile.camera_colour = lightcraft_develop::CameraColour::Macos;
    p.develop = std::sync::Arc::new(d);
    p.import_look = Some(p.develop.clone());
    p
}

/// #355 as reported: Sony raws on macOS Colour, masks of every kind with Portrait and Relight, one photo selected, Paste.
/// The pasted masks are in the list, deleting them takes their look away, and Reset renders the photo unedited.
#[test]
fn corpus_paste_onto_a_macos_colour_raw_then_reset() {
    let Some(path) = a7v_raw() else {
        lightcraft_testkit::skip!(Corpus, "the A7 V raw is absent (cargo xtask corpus --download)");
        return;
    };
    let mut s = Session::new().with_fs();
    for id in [1, 2] {
        s.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(sony(id, &path)) }).unwrap();
    }
    let (a, b) = (PhotoId(1), PhotoId(2));
    if crate::files::macos_available() {
        assert!(crate::files::macos_shows(s.catalog.photo(b).unwrap()), "the photo renders through macOS Colour");
    }
    let mut d = (*s.develop_of(a).unwrap()).clone();
    d.masks = every_kind();
    lightcraft_develop::controls::set(&mut d, "retouch.skin.smoothness", 60.0);
    lightcraft_develop::controls::set(&mut d, "relight.amount", 60.0);
    s.set_develop(a, d, "Look").unwrap();
    s.execute("library.select", &json!({"ids": [a.0], "active": a.0})).unwrap();
    s.execute("develop.copy", &json!({"groups": SettingsGroup::ALL})).unwrap();
    s.execute("library.select", &json!({"ids": [b.0], "active": b.0})).unwrap();
    let plain = s.develop_of(b).unwrap();
    let key = |s: &mut Session| s.render_job(b, 96, 96, false, true).unwrap().key;
    let (plain_key, plain_look) = (key(&mut s), s.render_now(b, 96, 96).unwrap().image);

    s.execute("develop.paste", &json!({})).unwrap();
    let pasted = s.develop_of(b).unwrap();
    assert_eq!(pasted.masks.len(), 6, "every pasted mask is in the list");
    assert_ne!(key(&mut s), plain_key, "the pasted look renders under its own key");
    s.execute("mask.deleteAll", &json!({})).unwrap();
    assert!(s.develop_of(b).unwrap().masks.is_empty());
    s.execute("edit.undo", &json!({})).unwrap();

    s.execute("develop.reset", &json!({})).unwrap();
    let reset = s.develop_of(b).unwrap();
    assert_eq!(reset.to_json(), plain.to_json(), "Reset gives an unedited photo's settings");
    assert_eq!(key(&mut s), plain_key, "and its render key");
    assert_eq!(s.render_now(b, 96, 96).unwrap().image.as_bytes(), plain_look.as_bytes(), "and its picture");
}
