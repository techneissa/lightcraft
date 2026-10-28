//! White balance of a macOS Colour photo in the session (plan §4, T9, T10, T10b): which scale it is edited on, and
//! what `develop.profile` does to its white balance when it leaves macOS Colour.
use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::{CameraColour, DevelopSettings, WbMode, WbScale};
use lightcraft_pipeline::local::{kelvin_to_legacy, legacy_to_kelvin};
use serde_json::json;

use crate::Session;

/// macOS's as-shot white of the photo, as the catalog learns it from a macOS Colour decode.
const APPLE: (f64, f64) = (3970.0, 4.0);

/// An A7 V ARW (a camera on the relative scale under Camera Matching) whose file is not on disk, active.
fn a7v(s: &mut Session) -> PhotoId {
    let id = s.catalog.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: "/photos/A7V00918.ARW".into() }, "A7V00918.ARW", "ARW", 64, 64, "2026-10-16T00:00:00");
    p.kind = MediaKind::Raw;
    p.meta.camera = "SONY ILCE-7M5".into();
    p.as_shot_wb = Some(APPLE);
    p.develop = std::sync::Arc::new(p.camera_defaults());
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
    id
}

fn dev(s: &Session, id: PhotoId) -> DevelopSettings {
    (*s.develop_of(id).unwrap()).clone()
}

/// Whether this computer's decoder lists the A7 V (a Mac with macOS 27).
fn mac(s: &Session, id: PhotoId) -> bool {
    crate::files::macos_lists(s.catalog.photo(id).unwrap())
}

/// T9: the photo is on the relative scale under Camera Matching everywhere, and in Kelvin on macOS Colour where the
/// decoder lists its camera; the catalog's guess before a decode agrees.
#[test]
fn macos_colour_edits_in_kelvin_where_the_mac_lists_the_camera() {
    let mut s = Session::new();
    let id = a7v(&mut s);
    let p = s.catalog.photo(id).unwrap().clone();
    assert!(p.relative_wb() && p.relative_wb_on(true) && crate::files::relative_wb(&p), "Camera Matching: relative");
    s.execute("develop.profile", &json!({"camera_colour": "macos"})).unwrap();
    let p = s.catalog.photo(id).unwrap().clone();
    assert!(!p.relative_wb_on(true) && p.relative_wb_on(false) && p.relative_wb(), "the catalog's own scale is unchanged");
    let kelvin = mac(&s, id);
    assert_eq!(crate::files::relative_wb(&p), !kelvin);
    let header = s.source_info(id);
    assert_eq!(header.relative_wb, !kelvin);
    if kelvin {
        assert!(header.legacy_relative_wb && (header.as_shot_temp, header.as_shot_tint) == APPLE, "{header:?}");
    }
}

/// T10b: As Shot round trip Auto → macOS Colour → Auto leaves the white balance exactly as it was.
#[test]
fn as_shot_round_trips_through_macos_colour() {
    let mut s = Session::new();
    let id = a7v(&mut s);
    let before = dev(&s, id);
    assert_eq!(before.wb.mode, WbMode::AsShot);
    s.execute("develop.profile", &json!({"camera_colour": "macos"})).unwrap();
    s.execute("develop.profile", &json!({"camera_colour": "auto"})).unwrap();
    let after = dev(&s, id);
    assert_eq!(after.wb, before.wb);
    assert_eq!(after.hash64(), before.hash64());
}

/// T10: a Custom white set on Camera Matching, edited on macOS Colour (now Kelvin), comes back to Camera Matching as the
/// relative white that renders the same: within 1 K of where it started. Without macOS's as-shot white (not a Mac) the
/// Kelvin values are kept (they render as shot there, `pipeline::local::effective_wb`).
#[test]
fn a_custom_white_set_in_kelvin_comes_back_to_the_relative_scale() {
    let mut s = Session::new();
    let id = a7v(&mut s);
    s.execute("develop.profile", &json!({"camera_colour": "macos"})).unwrap();
    let k = legacy_to_kelvin(7000.0, 5.0, APPLE);
    let mut d = dev(&s, id);
    (d.wb.mode, d.wb.temp, d.wb.tint, d.wb.scale) = (WbMode::Custom, k.0, k.1, WbScale::Kelvin);
    s.set_develop(id, d, "White Balance").unwrap();
    s.execute("develop.profile", &json!({"camera_colour": "auto"})).unwrap();
    let back = dev(&s, id);
    assert_eq!(back.profile.camera_colour, CameraColour::Auto);
    if mac(&s, id) {
        assert_eq!((back.wb.mode, back.wb.scale), (WbMode::Custom, WbScale::Legacy));
        assert!((back.wb.temp - 7000.0).abs() < 1.0 && (back.wb.tint - 5.0).abs() < 0.1, "{:?}", back.wb);
        assert_eq!((back.wb.temp, back.wb.tint), kelvin_to_legacy(k.0, k.1, APPLE));
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!((dev(&s, id).wb.scale, dev(&s, id).profile.camera_colour), (WbScale::Kelvin, CameraColour::Macos), "one undo step");
    } else {
        assert_eq!((back.wb.temp, back.wb.tint, back.wb.scale), (k.0, k.1, WbScale::Kelvin));
    }
}
