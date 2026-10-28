//! Presets that carry macOS Colour (plan §6.8, T37–T38): a preset made on macOS Colour with the Profile group stores
//! `macos` and puts the photos it is applied to on it in one undo step; where this computer can't make macOS Colour the
//! rest of the preset applies, the stored value stays `macos`, and the picture falls back at render time.

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::CameraColour;
use lightcraft_pipeline::MacosUnavailable;
use serde_json::json;

use super::tests::{Answer, D50, Fake, arw, load};
use crate::Session;
use crate::import::starting_colour_tests::{mac, raw};

fn colour(s: &Session, id: u64) -> CameraColour {
    s.catalog.photo(PhotoId(id)).unwrap().develop.profile.camera_colour
}

fn exposure(s: &Session, id: u64) -> f64 {
    s.catalog.photo(PhotoId(id)).unwrap().develop.light.exposure
}

fn pick(s: &mut Session, id: u64) {
    s.execute("library.select", &json!({"ids": [id]})).unwrap();
}

/// Photo 1 on macOS Colour with a bright look; a preset of everything ("Bright", Profile group included) and one of
/// Light only ("Bright Light"); photos 2.. on Camera Matching. Returns the two preset ids.
fn library(s: &mut Session, cameras: &[&str]) -> (String, String) {
    for (i, camera) in cameras.iter().enumerate() {
        s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(i as u64 + 1, camera)) }).unwrap();
    }
    pick(s, 1);
    s.execute("develop.profile", &json!({"camera_colour": "macos"})).unwrap();
    s.execute("develop.set", &json!({"values": {"light.exposure": 0.4, "color.saturation": 20}})).unwrap();
    let all = s.execute("preset.create", &json!({"name": "Bright"})).unwrap()["id"].as_str().unwrap().to_string();
    let light = s.execute("preset.create", &json!({"name": "Bright Light", "groups": ["light"]})).unwrap()["id"].as_str().unwrap().to_string();
    (all, light)
}

/// T37: the preset made with the Profile group stores `macos`; applied to a Camera Matching photo it sets macOS Colour
/// and the look in one undo step, which undo takes back to Camera Matching. A Light-only preset leaves the colour source.
#[test]
fn a_macos_colour_preset_round_trips() {
    let (_, _g) = mac(true, true, Some((7008, 4672)));
    let mut s = Session::new();
    let (all, light) = library(&mut s, &["SONY ILCE-7M5", "SONY ILCE-7M5"]);
    let stored = |id: &str| s.presets.iter().find(|p| p.id == id).unwrap().settings.clone();
    assert_eq!(stored(&all)["profile"]["camera_colour"], "macos");
    assert!(stored(&light).pointer("/profile/camera_colour").is_none(), "no Profile group, no colour source");

    pick(&mut s, 2);
    let undo = s.undo.len();
    let r = s.execute("preset.apply", &json!({"id": all})).unwrap();
    assert_eq!(r["macosFallback"], 0, "this Mac makes it");
    assert_eq!((colour(&s, 2), exposure(&s, 2)), (CameraColour::Macos, 0.4));
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((colour(&s, 2), exposure(&s, 2)), (CameraColour::Auto, 0.0));

    s.execute("preset.apply", &json!({"id": light})).unwrap();
    assert_eq!((colour(&s, 2), exposure(&s, 2)), (CameraColour::Auto, 0.4), "the look only");
}

/// T38: off the Mac, and for a camera macOS doesn't read, the rest of the preset applies, the stored value is `macos`,
/// and the summary counts those photos; the picture is Camera Matching with the note, and macOS Colour where the
/// decoder is there.
#[test]
fn a_macos_colour_preset_falls_back_where_it_cant_be_made() {
    let mut s = Session::new();
    let (all, _) = library(&mut s, &["SONY ILCE-7M5", "SONY ILCE-7M5", "SONY ILCE-7M5", "SONY ILCE-0X"]);
    {
        // (a) not a Mac: both photos it lands on count
        let (_, _g) = mac(false, true, Some((7008, 4672)));
        let r = s.execute("preset.apply", &json!({"id": all, "ids": [2, 3]})).unwrap();
        assert_eq!(r["macosFallback"], 2, "{r}");
        assert!((2..=3).all(|i| colour(&s, i) == CameraColour::Macos && exposure(&s, i) == 0.4), "stored and applied");
    }
    {
        // (b) a Mac that doesn't read the camera: only that photo counts
        let (_, _g) = mac(true, true, Some((7008, 4672)));
        let r = s.execute("preset.apply", &json!({"id": all, "ids": [3, 4]})).unwrap();
        assert_eq!(r["macosFallback"], 1, "{r}");
        assert_eq!((colour(&s, 4), exposure(&s, 4)), (CameraColour::Macos, 0.4));
    }
    // the picture: Camera Matching with the note off the Mac; macOS Colour where the decoder is there
    let bytes = arw("ILCE-7M5", 1);
    let off = Fake { available: false, ..Fake::new(Answer::Grey(D50)) };
    let (_, _, info, _) = load(&bytes, 4096, CameraColour::Macos, &off);
    assert_eq!(info.macos_note, Some(MacosUnavailable::System));
    assert!(info.relative_wb, "edited on Camera Matching's scale there");
    let on = Fake::new(Answer::Grey(D50));
    let (_, _, info, _) = load(&bytes, 4096, CameraColour::Macos, &on);
    assert_eq!(info.macos_note, None);
    assert!(!info.relative_wb && info.camera_color.is_none(), "macOS Colour");
}
