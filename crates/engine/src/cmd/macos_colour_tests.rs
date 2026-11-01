//! Apply macOS Colour to All Photos from This Camera (plan §6.1, T27).

use lightcraft_catalog::{Op, PhotoId};
use lightcraft_develop::CameraColour;
use serde_json::json;

use crate::Session;
use crate::import::starting_colour_tests::{checked, mac, raw, slow_mac};

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
    assert_eq!(n["checking"].as_u64(), Some(4), "the sizes are checked in the background first: {n}");
    checked(&s);
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

/// Review fix 6: the count and the move run on the UI thread, so they never wait for macOS's probe (seconds on a big
/// raw): an unchecked camera and size is checked on a thread with an activity row, the count says how many photos wait
/// for it, the move refuses until it is known, and the next count knows it. A preset's summary doesn't wait either.
#[test]
fn the_count_never_waits_for_macos() {
    let (probes, _g) = slow_mac(Some((7008, 4672)), std::time::Duration::from_millis(400), true, true);
    let mut s = Session::new();
    for i in 1..=2 {
        s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(i, "SONY ILCE-7M5")) }).unwrap();
    }
    let t0 = std::time::Instant::now();
    let n = s.execute("profiles.macosForCameraCount", &json!({"photo": 1})).unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_millis(200), "returned in {:?}", t0.elapsed());
    assert_eq!((n["photos"].as_u64(), n["checking"].as_u64()), (Some(0), Some(2)), "{n}");
    let e = s.execute("profiles.macosForCamera", &json!({"photo": 1})).unwrap_err();
    assert!(e.to_string().contains("Try again"), "{e}");
    assert!(checked(&s), "an activity row checks the camera");
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1, "one probe for the camera and size, not two");
    let n = s.execute("profiles.macosForCameraCount", &json!({"photo": 1})).unwrap();
    assert_eq!((n["photos"].as_u64(), n["checking"].as_u64()), (Some(2), Some(0)), "{n}");
    assert!(!checked(&s), "nothing left to check");

    // a preset putting another size on macOS Colour: the summary doesn't wait, and the check runs behind it
    let mut other = raw(3, "SONY ILCE-7M5");
    (other.width, other.height) = (3504, 2336);
    s.catalog.apply(Op::AddPhoto { photo: Box::new(other) }).unwrap();
    s.execute("library.select", &json!({"ids": [1]})).unwrap();
    s.execute("develop.profile", &json!({"camera_colour": "macos"})).unwrap();
    let preset = s.execute("preset.create", &json!({"name": "Mac"})).unwrap()["id"].as_str().unwrap().to_string();
    let t0 = std::time::Instant::now();
    let r = s.execute("preset.apply", &json!({"id": preset, "ids": [3]})).unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_millis(200), "returned in {:?}", t0.elapsed());
    assert_eq!(r["macosFallback"], 0, "not known yet: {r}");
    assert!(checked(&s));
    let r = s.execute("preset.apply", &json!({"id": preset, "ids": [3]})).unwrap();
    assert_eq!(r["macosFallback"], 1, "known now: another size: {r}");
}

/// A raw the background check can't read is counted as another size by the next count, not checked forever (an
/// import still probes it again: `a_failed_probe_is_not_remembered`).
#[test]
fn an_unreadable_raw_is_not_checked_forever() {
    let (probes, _g) = mac(true, true, None);
    let mut s = Session::new();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(1, "SONY ILCE-7M5")) }).unwrap();
    let n = s.execute("profiles.macosForCameraCount", &json!({"photo": 1})).unwrap();
    assert_eq!(n["checking"].as_u64(), Some(1));
    checked(&s);
    let n = s.execute("profiles.macosForCameraCount", &json!({"photo": 1})).unwrap();
    assert_eq!((n["checking"].as_u64(), n["otherSize"].as_u64()), (Some(0), Some(1)), "{n}");
    assert!(!checked(&s));
    assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
}
