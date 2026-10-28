//! Apply macOS Colour to All Photos from This Camera (plan §6.1, T27).

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::CameraColour;
use serde_json::json;

use crate::Session;
use crate::import::starting_colour_tests::{mac, raw};

fn colour(s: &Session, id: u64) -> CameraColour {
    s.catalog.photo(PhotoId(id)).unwrap().develop.profile.camera_colour
}

/// T27: the count comes first and changes nothing; the move is one undo step over the camera's photos, skips the ones
/// that fail a gate (another size), leaves other cameras alone, and reports the counts.
#[test]
fn all_photos_from_this_camera_move_in_one_undo_step() {
    let (_, _g) = mac(true, true, Some((7008, 4672)));
    let mut s = Session::new();
    for i in 1..=3 {
        s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(i, "SONY ILCE-7M5")) }).unwrap();
    }
    let mut small = raw(4, "SONY ILCE-7M5");
    (small.width, small.height) = (3504, 2336);
    s.catalog.apply(Op::AddPhoto { photo: Box::new(small) }).unwrap();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(5, "SONY ILCE-7M4")) }).unwrap();
    let mut on = raw(6, "SONY ILCE-7M5");
    std::sync::Arc::make_mut(&mut on.develop).profile.camera_colour = CameraColour::Macos;
    s.catalog.apply(Op::AddPhoto { photo: Box::new(on) }).unwrap();

    let n = s.execute("profiles.macosForCameraCount", &json!({"photo": 1})).unwrap();
    assert_eq!(
        (n["camera"].as_str(), n["photos"].as_u64(), n["skipped"].as_u64(), n["otherSize"].as_u64()),
        (Some("SONY ILCE-7M5"), Some(3), Some(1), Some(1))
    );
    assert_eq!(colour(&s, 1), CameraColour::Auto, "counting changes nothing");

    let undo = s.undo.len();
    let r = s.execute("profiles.macosForCamera", &json!({"photo": 1})).unwrap();
    assert_eq!((r["changed"].as_u64(), r["skipped"].as_u64()), (Some(3), Some(1)), "{r}");
    assert_eq!(s.undo.len(), undo + 1, "one undo step");
    assert!((1..=3).all(|i| colour(&s, i) == CameraColour::Macos));
    assert_eq!(colour(&s, 4), CameraColour::Auto, "another size is skipped");
    assert_eq!(colour(&s, 5), CameraColour::Auto, "another camera is left alone");
    s.execute("edit.undo", &json!({})).unwrap();
    assert!((1..=3).all(|i| colour(&s, i) == CameraColour::Auto), "undone in one step");
}

/// Off the Mac nothing moves: every photo is counted as skipped because this computer isn't a Mac.
#[test]
fn off_the_mac_no_photo_moves() {
    let (_, _g) = mac(false, true, Some((7008, 4672)));
    let mut s = Session::new();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(1, "SONY ILCE-7M5")) }).unwrap();
    let r = s.execute("profiles.macosForCamera", &json!({"photo": 1})).unwrap();
    assert_eq!((r["changed"].as_u64(), r["notMac"].as_u64()), (Some(0), Some(1)));
    assert_eq!(colour(&s, 1), CameraColour::Auto);
}
