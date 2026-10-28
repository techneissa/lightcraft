//! LT11 (engine half): the one-time offer to move the photos a local profile file used to change inside Camera
//! Matching to its own Profile choice: counted, never applied by itself, one undo step, asked once.

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;
use serde_json::json;

use crate::Session;
use crate::camera_profiles::{self, CameraProfile};

fn raw(id: u64, format: &str, camera: &str) -> Photo {
    let mut p =
        Photo::new(PhotoId(id), Source::File { path: format!("/p/{id}.{format}") }, &format!("{id}.{format}"), format, 6000, 4000, "2026-10-28");
    p.kind = MediaKind::Raw;
    p.meta.camera = camera.into();
    p
}

fn colour(s: &Session, id: u64) -> (CameraColour, Option<String>) {
    let p = &s.catalog.photo(PhotoId(id)).unwrap().develop.profile;
    (p.camera_colour, p.camera_profile.clone())
}

#[test]
fn the_offer_counts_the_photos_the_file_changed_and_moves_them_only_when_asked() {
    let dir = std::env::temp_dir().join(format!("lc-local-offer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = CameraProfile::new("ILCE-7M5", 40, 9000, lightcraft_color::Mat3::IDENTITY, None);
    camera_profiles::save(&p, &dir).unwrap(); // ILCE-7M5.json: the name Camera Matching used to read
    camera_profiles::save_named(&p, &dir, "A7 V Chart").unwrap(); // never changed Camera Matching: no offer
    std::fs::write(dir.join("broken.json"), b"[]").unwrap();
    let _dir = camera_profiles::use_dir_for_tests(&dir);

    let mut s = Session::new();
    let photos = [
        raw(1, "ARW", "SONY ILCE-7M5"),
        raw(2, "ARW", "SONY ILCE-7M5"),
        raw(3, "ARW", "SONY ILCE-7M5"), // already chose No Camera Matching
        raw(4, "ARW", "SONY ILCE-7M4"), // another camera
        raw(5, "DNG", "SONY ILCE-7M5"), // not a fitted look
    ];
    for photo in photos {
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    s.execute("library.select", &json!({"ids": [3], "active": 3})).unwrap();
    s.execute("develop.profile", &json!({"camera_colour": "colorimetric"})).unwrap();

    let offer = s.execute("profiles.localOffer", &json!({})).unwrap();
    assert_eq!(offer["offer"], json!({"id": "ILCE-7M5", "name": "ILCE-7M5", "photos": 2}), "{offer}");
    // asking changes nothing
    let before: Vec<_> = (1..=5).map(|i| colour(&s, i)).collect();
    s.execute("profiles.localOffer", &json!({})).unwrap();
    assert_eq!((1..=5).map(|i| colour(&s, i)).collect::<Vec<_>>(), before, "an offer never moves a photo");

    // Switch: both photos, one undo step
    let r = s.execute("profiles.localSwitch", &json!({"id": "ILCE-7M5"})).unwrap();
    assert_eq!(r["changed"], 2);
    let chosen = (CameraColour::Local, Some("ILCE-7M5".to_string()));
    assert_eq!((colour(&s, 1), colour(&s, 2)), (chosen.clone(), chosen.clone()));
    assert_eq!(colour(&s, 3).0, CameraColour::Colorimetric, "a photo that chose is left alone");
    assert_eq!((colour(&s, 4).0, colour(&s, 5).0), (CameraColour::Auto, CameraColour::Auto));
    assert_eq!(s.undo.last().map(|e| e.label.as_str()), Some("Profile: ILCE-7M5"), "the step is named for the profile");
    assert_eq!(s.local_profile_asked, ["ILCE-7M5"], "Switch answers the question");
    assert_eq!(s.execute("profiles.localOffer", &json!({})).unwrap()["offer"], json!(null));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!((colour(&s, 1), colour(&s, 2)), ((CameraColour::Auto, None), (CameraColour::Auto, None)), "one undo step restores both");
    // undoing the Switch puts the question back; redoing it answers it again
    assert!(s.local_profile_asked.is_empty(), "the undone Switch is not an answer");
    assert!(s.execute("profiles.localOffer", &json!({})).unwrap()["offer"].is_object(), "the offer is back");
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(colour(&s, 1), chosen);
    assert_eq!(s.local_profile_asked, ["ILCE-7M5"]);
    s.execute("edit.undo", &json!({})).unwrap();
    // Leave on Camera Matching is an answer too: asked once, not offered again
    s.execute("profiles.localAsked", &json!({"id": "ILCE-7M5"})).unwrap();
    s.execute("profiles.localAsked", &json!({"id": "ILCE-7M5"})).unwrap();
    assert_eq!(s.execute("profiles.localOffer", &json!({})).unwrap()["offer"], json!(null));
    assert_eq!(s.local_profile_asked, ["ILCE-7M5"], "once");

    // errors, never panics
    for bad in [json!({}), json!({"id": "nothing"}), json!({"id": "broken"}), json!({"id": "../ILCE-7M5"})] {
        assert!(s.execute("profiles.localSwitch", &bad).is_err(), "{bad}");
    }
    // Leave on Camera Matching: only the answer is recorded
    let mut t = Session::new();
    t.catalog.apply(Op::AddPhoto { photo: Box::new(raw(1, "ARW", "SONY ILCE-7M5")) }).unwrap();
    assert!(t.execute("profiles.localOffer", &json!({})).unwrap()["offer"].is_object());
    t.execute("profiles.localAsked", &json!({"id": "ILCE-7M5"})).unwrap();
    assert_eq!(t.execute("profiles.localOffer", &json!({})).unwrap()["offer"], json!(null));
    assert_eq!(colour(&t, 1), (CameraColour::Auto, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// LT12 (settings half): a DNG written from a photo on a local profile (or macOS Colour) carries the colour in its own
/// matrices, so the settings in its XMP say Camera Matching; a photo on neither is written as it is.
#[test]
fn a_dng_describes_a_local_profile_photo_as_camera_matching() {
    use std::borrow::Cow;
    let mut s = Session::new();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(1, "ARW", "SONY ILCE-7M5")) }).unwrap();
    s.execute("library.select", &json!({"ids": [1], "active": 1})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    s.execute("develop.profile", &json!({"camera_colour": "local", "camera_profile": "A7 V Chart"})).unwrap();
    let photo = s.catalog.photo(PhotoId(1)).unwrap().clone();
    let packet = crate::sidecar::sidecar_packet(&photo, &s.catalog);
    assert!(packet.contains("A7 V Chart"), "the photo's own packet names the profile");
    let described = crate::export::for_dng(&photo);
    assert!(matches!(described, Cow::Owned(_)));
    assert_eq!((described.develop.profile.camera_colour, described.develop.profile.camera_profile.clone()), (CameraColour::Auto, None));
    assert_eq!(described.develop.light.exposure, 0.5, "the rest of the settings are kept");
    let packet = crate::sidecar::sidecar_packet(&described, &s.catalog);
    assert!(!packet.contains("A7 V Chart") && !packet.contains("\"local\""), "the DNG's settings name no colour source");
    s.execute("develop.profile", &json!({"camera_colour": "macos"})).unwrap();
    let macos = s.catalog.photo(PhotoId(1)).unwrap().clone();
    assert_eq!(crate::export::for_dng(&macos).develop.profile.camera_colour, CameraColour::Auto);
    s.execute("develop.profile", &json!({"camera_colour": "colorimetric"})).unwrap();
    let plain = s.catalog.photo(PhotoId(1)).unwrap().clone();
    assert!(matches!(crate::export::for_dng(&plain), Cow::Borrowed(_)), "a photo on neither is written as it is");
}

/// Choosing a local profile for a photo is named in the history, like the Switch step; Camera Matching keeps its name.
#[test]
fn the_history_names_the_local_profile_a_photo_is_set_to() {
    let dir = std::env::temp_dir().join(format!("lc-local-label-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = CameraProfile::new("ILCE-7M5", 40, 9000, lightcraft_color::Mat3::IDENTITY, None);
    camera_profiles::save_named(&p, &dir, "A7 V Chart").unwrap();
    let _dir = camera_profiles::use_dir_for_tests(&dir);
    let mut s = Session::new();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(1, "ARW", "SONY ILCE-7M5")) }).unwrap();
    s.execute("library.select", &json!({"ids": [1], "active": 1})).unwrap();
    let label = |s: &Session| s.undo.last().map(|e| e.label.clone());
    s.execute("develop.profile", &json!({"camera_colour": "local", "camera_profile": "A7 V Chart"})).unwrap();
    assert_eq!(label(&s).as_deref(), Some("Profile: A7 V Chart"));
    s.execute("develop.profile", &json!({"camera_colour": "auto"})).unwrap();
    assert_eq!(label(&s).as_deref(), Some("Camera Matching"));
    s.execute("develop.profile", &json!({"camera_colour": "local", "camera_profile": "gone"})).unwrap();
    assert_eq!(label(&s).as_deref(), Some("Profile: gone"), "a file that is not there is named by its id");
    let _ = std::fs::remove_dir_all(&dir);
}
