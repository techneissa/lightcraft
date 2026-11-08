//! Fork feature (`fork/match-look.md`): Match Look gives photos another picture's look as slider values, one undo
//! step, in the background with an activity row; a bad reference is an error, never a panic.

use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use serde_json::{Value, json};

use crate::Session;

fn demo() -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    let ids: Vec<PhotoId> = s.visible_cloned().into_iter().take(3).collect();
    (s, ids)
}

#[test]
fn a_library_photo_as_reference_sets_sliders_in_one_undo_step() {
    let (mut s, ids) = demo();
    let (a, b, reference) = (ids[0], ids[1], ids[2]);
    let before = [(*s.develop_of(a).unwrap()).clone(), (*s.develop_of(b).unwrap()).clone()];
    // what the reference as it is gives (the demo photos differ: dunes against a sunset has no one right white
    // balance, so this test checks Exposure; the look crate's tests check Temp on alike scenes)
    s.execute("develop.matchLook", &json!({"ids": [a, b], "reference": {"photo": reference}})).unwrap();
    let ex = |s: &Session, id| s.develop_of(id).unwrap().light.exposure;
    let (ea, eb) = (ex(&s, a), ex(&s, b));
    s.execute("edit.undo", &json!({})).unwrap();
    // the reference made much darker
    s.execute("develop.set", &json!({"ids": [reference], "values": {"light.exposure": -1.5}})).unwrap();
    let v = s.execute("develop.matchLook", &json!({"ids": [a, b], "reference": {"photo": reference}})).unwrap();
    assert_eq!(v["photos"].as_array().map(Vec::len), Some(2), "{v}");
    assert!(v["failed"].as_array().is_some_and(Vec::is_empty), "{v}");
    assert!(ex(&s, a) < ea && ex(&s, b) < eb, "darker than to the reference as it was: {} < {ea}, {} < {eb}", ex(&s, a), ex(&s, b));
    // one undo step brings both back
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(*s.develop_of(a).unwrap(), before[0]);
    assert_eq!(*s.develop_of(b).unwrap(), before[1]);
}

#[test]
fn a_background_match_shows_its_row_and_applies_from_the_pump() {
    let (mut s, ids) = demo();
    s.execute("develop.set", &json!({"ids": [ids[1]], "values": {"light.exposure": 1.2}})).unwrap();
    let before = (*s.develop_of(ids[0]).unwrap()).clone();
    let v = s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": {"photo": ids[1]}, "wait": false})).unwrap();
    assert_eq!(v["started"], true);
    assert!(s.activity.list().iter().any(|t| t.kind == "look"), "the match has a row while it runs");
    // a second one waits its turn
    assert!(s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": {"photo": ids[1]}})).is_err());
    let deadline = Instant::now() + Duration::from_secs(300);
    let last: Value = loop {
        let p = s.execute("develop.matchLookPump", &json!({})).unwrap();
        if p["active"] == false {
            break p["last"].clone();
        }
        assert!(Instant::now() < deadline, "the match never finished");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(last["photos"].as_array().map(Vec::len), Some(1), "{last}");
    assert!(last["photos"][0]["changes"].as_array().is_some_and(|c| !c.is_empty()), "{last}");
    assert_ne!(*s.develop_of(ids[0]).unwrap(), before, "applied by the pump");
    assert!(!s.activity.list().iter().any(|t| t.kind == "look"), "the row goes when it ends");
}

#[test]
fn bad_references_are_errors_not_panics() {
    let (mut s, ids) = demo();
    let before = (*s.develop_of(ids[0]).unwrap()).clone();
    for r in [json!({}), json!({"url": "http://example.com/a.jpg"}), json!({"url": "nonsense"}), json!({"photo": 999_999}), json!({"pasted": true})] {
        assert!(s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": r})).is_err(), "{r}");
    }
    assert!(s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": {"photo": ids[1]}, "amount": 150})).is_err());
    // a file that is not an image: reported per photo, nothing changed
    let dir = std::env::temp_dir().join(format!("lightcraft-match-look-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let junk = dir.join("junk.jpg");
    std::fs::write(&junk, b"\xff\xd8\xff garbage that is no JPEG").unwrap();
    for path in [junk.clone(), dir.join("missing.jpg")] {
        let v = s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": {"path": path}})).unwrap();
        assert_eq!(v["failed"].as_array().map(Vec::len), Some(1), "{v}");
    }
    assert_eq!(*s.develop_of(ids[0]).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pasted_picture_is_a_reference() {
    let (mut s, ids) = demo();
    // a dark, warm picture
    s.set_match_look_pasted(lightcraft_raster::Rgba8::from_fn(64, 48, |x, y| [(40 + x) as u8, (20 + y / 2) as u8, 10, 255]));
    assert!(s.match_look_has_pasted());
    let ex = s.develop_of(ids[0]).unwrap().light.exposure;
    let v = s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": {"pasted": true}})).unwrap();
    assert_eq!(v["photos"].as_array().map(Vec::len), Some(1), "{v}");
    assert!(s.develop_of(ids[0]).unwrap().light.exposure < ex);
}

#[test]
fn amount_zero_changes_nothing_and_adds_no_undo_step() {
    let (mut s, ids) = demo();
    s.execute("develop.set", &json!({"ids": [ids[1]], "values": {"wb.temp": 9500, "light.exposure": -1.0}})).unwrap();
    let before = (*s.develop_of(ids[0]).unwrap()).clone();
    let steps = s.undo.len();
    s.execute("develop.matchLook", &json!({"ids": [ids[0]], "reference": {"photo": ids[1]}, "amount": 0})).unwrap();
    assert_eq!(serde_json::to_string(&*s.develop_of(ids[0]).unwrap()).unwrap(), serde_json::to_string(&before).unwrap());
    assert_eq!(s.undo.len(), steps, "no undo step");
}
