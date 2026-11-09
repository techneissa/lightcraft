//! Background Swap's picture in the engine (`fork/retouch-background.md`): the command (a file copied into the
//! library by its hash), loading a library photo off the UI thread, renders getting it with the faces, exports
//! waiting for it on their worker and failing when it is missing.

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

/// A small PNG picture file in a folder of its own.
fn picture_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lightcraft-swap-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut img = Rgba8::filled(40, 30, [0, 0, 90, 255]);
    for (i, p) in img.data.iter_mut().enumerate() {
        *p = [(i % 40 * 6) as u8, (i / 40 * 8) as u8, 90, 255];
    }
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn the_command_sets_a_colour_a_file_or_a_photo_and_refuses_anything_else() {
    let mut s = demo();
    let id = s.active().unwrap();
    let r = s.execute("retouch.background", &json!({"with": {"colour": "#FFFFFF"}})).unwrap();
    assert_eq!(r["with"]["colour"], "#FFFFFF", "{r}");
    assert_eq!(r["loaded"], false);
    let other = other(&s);
    let r = s.execute("retouch.background", &json!({"with": {"photo": other}})).unwrap();
    assert_eq!(r["with"]["photo"], other, "{r}");
    assert_eq!(r["picture"], format!("photo:{other}"));
    let junk = picture_file("junk.png");
    std::fs::write(&junk, b"not a picture").unwrap();
    for bad in [
        json!({"with": {"colour": "white"}}),
        json!({"with": {"file": "/no/such/picture.jpg"}}),
        json!({"with": {"file": junk.to_string_lossy()}}),
        json!({"with": {"photo": 999_999}}),
        json!({"with": {"photo": id.0}}),
        json!({"with": {"colour": "#000000", "photo": other}}),
        json!({"with": {}}),
    ] {
        assert!(s.execute("retouch.background", &bad).is_err(), "{bad}");
    }
    let d = s.develop_of(id).unwrap();
    assert_eq!(d.retouch.global.background.with.picture(), Some(format!("photo:{other}")), "a refused value changes nothing");
}

/// A picture file is copied into the library's Backgrounds folder as `<sha256>.<ext>`, and the photo's settings keep
/// its hash only: nothing saved names the file's folder.
#[test]
fn a_picture_file_is_copied_into_the_library_and_kept_by_its_hash() {
    let mut s = demo();
    let id = s.active().unwrap();
    let file = picture_file("studio wall.png");
    let r = s.execute("retouch.background", &json!({"with": {"file": file.to_string_lossy()}})).unwrap();
    let hash = r["with"]["picture"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64, "{r}");
    assert_eq!(r["name"], "studio wall.png");
    let copy = s.backdrop_dir().join(format!("{hash}.png"));
    assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&file).unwrap());
    let saved = serde_json::to_string(&*s.develop_of(id).unwrap()).unwrap();
    let folder = file.parent().unwrap().to_string_lossy().into_owned();
    assert!(saved.contains(&hash) && !saved.contains(&folder), "the settings keep the hash, never the path");
    // the same picture again is the same file
    let again = s.execute("retouch.background", &json!({"with": {"file": file.to_string_lossy()}})).unwrap();
    assert_eq!(again["with"]["picture"], hash);
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
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

/// An export started now waits for the picture on its own worker (the load starts when it is set up, never on this
/// thread), then renders with it.
#[test]
fn an_export_waits_for_the_picture_on_its_worker() {
    let mut s = demo();
    let id = s.active().unwrap();
    s.execute("retouch.detect", &json!({})).unwrap();
    let other = other(&s);
    s.execute("develop.set", &json!({"control": "retouch.background.swap", "value": 100})).unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.retouch.global.background.with = lightcraft_develop::retouch::background::Swap::Photo { id: other };
    s.set_develop(id, d, "test").unwrap();
    let wait = s.retouch_export_backdrop(id).expect("the export waits for a picture");
    let mut job = s.export_job(id, 120, 120, Default::default(), Default::default()).unwrap();
    let worker = std::thread::spawn(move || {
        let r = wait.fill(&mut [&mut job]);
        (r, job)
    });
    let (r, job) = worker.join().unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert!(job.info.retouch.as_ref().is_some_and(|i| i.backdrop.is_some()), "the export renders with the picture");
}

/// A Swap picture that is not in the library fails the export, naming the picture, and the section says it is
/// missing; never a silent export without it.
#[test]
fn a_missing_picture_fails_the_export_with_its_name() {
    let mut s = demo();
    let id = s.active().unwrap();
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("develop.set", &json!({"control": "retouch.background.swap", "value": 100})).unwrap();
    let mut d = (*s.develop_of(id).unwrap()).clone();
    d.retouch.global.background.with = lightcraft_develop::retouch::background::Swap::Picture { hash: "ab".repeat(32) };
    s.set_develop(id, d, "test").unwrap();
    let st = s.execute("retouch.background", &json!({})).unwrap();
    assert_eq!(st["missing"], true, "{st}");
    let wait = s.retouch_export_backdrop(id).unwrap();
    let mut job = s.export_job(id, 120, 120, Default::default(), Default::default()).unwrap();
    let e = wait.fill(&mut [&mut job]).unwrap_err();
    assert!(e.contains("abababab") && e.contains("missing"), "{e}");
    let e = s.render_export(id, 120, 120, Default::default(), Default::default()).err().unwrap_or_default();
    assert!(e.contains("missing"), "{e}");
}

/// An iPhone Portrait photo's depth map reaches Blur with the faces (read once, off the UI thread; an export reads
/// it itself). A build without HEIF reads none and renders as before.
#[test]
fn a_heic_photos_depth_map_reaches_the_render_when_blur_is_on() {
    use lightcraft_heif::testdata::{Spec, build};
    let dir = std::env::temp_dir().join(format!("lightcraft-depth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let photo = |x: u32, y: u32| [((x * 7 + y * 13) % 200 + 30) as u16, 110, 150];
    let mut spec = Spec::new(96, 64, &photo);
    spec.ignored_aux = vec!["urn:mpeg:hevc:2015:auxid:2"];
    let bytes = build(&spec);
    let path = dir.join("IMG_0003.HEIC");
    std::fs::write(&path, &bytes).unwrap();
    let readable = lightcraft_codecs::Format::Heif.can_decode();
    assert_eq!(lightcraft_codecs::decode_depth(&bytes).is_some(), readable);
    if !readable {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let mut s = Session::new().with_fs();
    s.retouch_faces.set_provider(Arc::new(OnePerson));
    let r = s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    let id = crate::catalog::PhotoId(r["imported"][0].as_u64().unwrap());
    s.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
    s.execute("retouch.detect", &json!({})).unwrap();
    s.execute("develop.set", &json!({"control": "retouch.background.blur", "value": 60})).unwrap();
    // the export reads it on its worker
    let wait = s.retouch_export_backdrop(id).expect("the export reads the depth map");
    let mut job = s.export_job(id, 96, 96, Default::default(), Default::default()).unwrap();
    wait.fill(&mut [&mut job]).unwrap();
    let input = job.info.retouch.clone().unwrap();
    let depth = input.depth.as_ref().expect("the depth map");
    assert_eq!((depth.plane.width, depth.plane.height), (96, 64));
    // Blur off: no depth map, and the faces are the found ones
    s.execute("develop.set", &json!({"control": "retouch.background.blur", "value": 0})).unwrap();
    s.execute("develop.set", &json!({"control": "retouch.background.clean", "value": 50})).unwrap();
    let job = s.render_job(id, 96, 96, false, true).unwrap();
    assert!(job.info.retouch.as_ref().is_some_and(|i| i.depth.is_none()));
    let _ = std::fs::remove_dir_all(&dir);
}
