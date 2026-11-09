//! Background Swap's picture in the engine (`fork/retouch-background.md`): the command, loading a library photo
//! off the UI thread, renders getting it with the faces, and exports waiting for it.

use std::sync::Arc;

use lightcraft_develop::retouch::{Face, NormRect, Part, PartMask};
use lightcraft_raster::{Plane, Rgba8};
use serde_json::json;

use crate::Session;
use crate::retouch_input::FaceProvider;

/// One person in the middle of the photo, with an Entire Person mask.
struct OnePerson;

impl FaceProvider for OnePerson {
    fn name(&self) -> &str {
        "One Person"
    }
    fn faces(&self, _img: &Rgba8) -> Result<Vec<Face>, String> {
        let plane = Plane::from_fn(32, 32, |x, y| if (8..24).contains(&x) && y >= 6 { 1.0 } else { 0.0 });
        let mask = PartMask { plane, area: NormRect::new(0.0, 0.0, 1.0, 1.0) };
        Ok(vec![Face { masks: vec![(Part::EntirePerson, mask)], ..Face::boxed(NormRect::new(0.35, 0.2, 0.65, 0.45)) }])
    }
}

fn demo() -> Session {
    let mut s = Session::with_demo();
    s.retouch_faces.set_provider(Arc::new(OnePerson));
    s
}

/// Another photo of the demo library than the active one.
fn other(s: &Session) -> u64 {
    let id = s.active().unwrap();
    s.catalog.photos().map(|p| p.id).find(|p| *p != id).unwrap().0
}

#[test]
fn the_command_sets_a_colour_a_file_or_a_photo_and_refuses_anything_else() {
    let mut s = demo();
    let r = s.execute("retouch.background", &json!({"with": {"colour": "#FFFFFF"}})).unwrap();
    assert_eq!(r["with"]["colour"], "#FFFFFF", "{r}");
    assert_eq!(r["loaded"], false);
    let other = other(&s);
    let r = s.execute("retouch.background", &json!({"with": {"photo": other}})).unwrap();
    assert_eq!(r["with"]["photo"], other, "{r}");
    assert_eq!(r["picture"], format!("photo:{other}"));
    for bad in [
        json!({"with": {"colour": "white"}}),
        json!({"with": {"file": "/no/such/picture.jpg"}}),
        json!({"with": {"photo": 999_999}}),
        json!({"with": {"colour": "#000000", "photo": other}}),
        json!({"with": {}}),
    ] {
        assert!(s.execute("retouch.background", &bad).is_err(), "{bad}");
    }
    let d = s.develop_of(s.active().unwrap()).unwrap();
    assert_eq!(d.retouch.global.background.with.picture(), Some(format!("photo:{other}")), "a refused value changes nothing");
}

/// A library photo as Swap's picture: loaded on a worker (with an activity row), then every render of the photo
/// gets it with its faces, and the render changes.
#[test]
fn a_library_photo_loads_in_the_background_and_reaches_the_render() {
    let mut s = demo();
    let id = s.active().unwrap();
    s.execute("retouch.detect", &json!({})).unwrap();
    let other = other(&s);
    s.execute("develop.set", &json!({"control": "retouch.background.swap", "value": 100})).unwrap();
    let plain = s.render_now(id, 160, 160).unwrap().image;
    s.execute("retouch.background", &json!({"with": {"photo": other}})).unwrap();
    // wait for the worker (the timeout only guards against a hang)
    let start = std::time::Instant::now();
    while s.execute("retouch.background", &json!({})).unwrap()["loaded"] != true {
        assert!(start.elapsed().as_secs() < 120, "the picture never loaded");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let job = s.render_job(id, 160, 160, false, true).unwrap();
    let input = job.info.retouch.clone().unwrap();
    assert!(input.backdrop.is_some(), "the render gets the picture with the faces");
    let swapped = s.render_now(id, 160, 160).unwrap().image;
    assert_ne!(plain.data, swapped.data);
}

/// An export loads the picture itself when it is not loaded yet.
#[test]
fn an_export_loads_the_picture_first() {
    let mut s = demo();
    let id = s.active().unwrap();
    let other = other(&s);
    s.execute("develop.set", &json!({"control": "retouch.background.swap", "value": 100})).unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.retouch.global.background.with = lightcraft_develop::retouch::background::Swap::Photo { id: other };
    s.set_develop(id, d, "test").unwrap();
    assert_eq!(s.execute("retouch.background", &json!({})).unwrap()["loaded"], false);
    let job = s.export_job(id, 120, 120, Default::default(), Default::default()).unwrap();
    assert!(job.info.retouch.as_ref().is_some_and(|i| i.backdrop.is_some()), "the export renders with the picture");
}
