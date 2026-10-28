//! Starting Colour for Raw Photos (plan §6.7, T28–T35): each test fails before the setting exists.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;
use serde_json::json;

use super::{GATE_FOR_TESTS, Gate, StartingColour, Verdicts};
use crate::Session;
use crate::import::{ImportDefaults, apply_import_defaults};

/// The A7 V's raw size as LightCraft reads it.
const SIZE: (u32, u32) = (7008, 4672);

/// A stand-in for the Mac's raw decoder: whether it is there, lists the camera, and the size it reads (counted).
pub(crate) struct FakeGate {
    pub available: bool,
    pub listed: bool,
    pub size: Option<(u32, u32)>,
    pub probes: Arc<AtomicUsize>,
}

impl Gate for FakeGate {
    fn available(&self) -> bool {
        self.available
    }
    fn supports(&self, exif_model: &str) -> bool {
        self.listed && exif_model == "ILCE-7M5"
    }
    fn native_size(&self, _: &Path) -> Option<(u32, u32)> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        self.size
    }
}

/// A Mac whose decoder reads the A7 V at LightCraft's size, upright (portrait-agnostic), on this thread; returns the
/// probe counter. Cleared when the returned guard drops.
pub(crate) fn mac(available: bool, listed: bool, size: Option<(u32, u32)>) -> (Arc<AtomicUsize>, impl Drop) {
    let probes = Arc::new(AtomicUsize::new(0));
    let gate = FakeGate { available, listed, size, probes: probes.clone() };
    GATE_FOR_TESTS.with(|g| *g.borrow_mut() = Some(Rc::new((Box::new(gate) as Box<dyn Gate>, Verdicts::default()))));
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            GATE_FOR_TESTS.with(|g| *g.borrow_mut() = None);
        }
    }
    (probes, Clear)
}

pub(crate) fn raw(id: u64, camera: &str) -> Photo {
    let mut p = Photo::new(PhotoId(id), Source::File { path: format!("/scratch/{id}.arw") }, "a.arw", "ARW", SIZE.0, SIZE.1, "2026-10-28");
    p.kind = MediaKind::Raw;
    p.meta.camera = camera.into();
    p
}

fn imported(s: &Session, camera: &str) -> Photo {
    let mut p = raw(1, camera);
    apply_import_defaults(s, &mut p);
    p
}

/// T28: the default is macOS Colour; `library.preferences` sets and reports it; an old `prefs.json` without it reads
/// the default; it round-trips through the saved JSON.
#[test]
fn starting_colour_defaults_to_macos_and_round_trips() {
    assert_eq!(ImportDefaults::default().starting_colour, StartingColour::Macos);
    let old: ImportDefaults = serde_json::from_value(json!({"perCamera": true})).unwrap();
    assert_eq!(old.starting_colour, StartingColour::Macos, "an old prefs.json reads the default");
    let mut s = Session::new();
    let r = s.execute("library.preferences", &json!({"import": {"startingColour": "noCameraMatching"}})).unwrap();
    assert_eq!(r["import"]["startingColour"], "noCameraMatching");
    assert_eq!(s.import_defaults.starting_colour, StartingColour::NoCameraMatching);
    let saved = serde_json::to_value(&s.import_defaults).unwrap();
    assert_eq!(serde_json::from_value::<ImportDefaults>(saved).unwrap().starting_colour, StartingColour::NoCameraMatching);
    assert!(s.execute("library.preferences", &json!({"import": {"startingColour": "sepia"}})).is_err());
    assert_eq!(s.import_defaults.starting_colour, StartingColour::NoCameraMatching, "a bad value changes nothing");
}

/// T29: on a Mac that reads the camera, a new A7 V raw starts on macOS Colour, as its import look (not "edited").
#[test]
fn a_new_raw_starts_on_macos_colour_on_a_mac() {
    let (_, _g) = mac(true, true, Some((SIZE.1, SIZE.0)));
    let p = imported(&Session::new(), "SONY ILCE-7M5");
    assert_eq!(p.develop.profile.camera_colour, CameraColour::Macos);
    assert_eq!(p.import_look.as_ref().map(|l| l.profile.camera_colour), Some(CameraColour::Macos));
    assert!(!p.is_edited(), "the starting colour is not an edit");
}

/// T30: a camera macOS doesn't list, and one it reads at another size, start on Camera Matching.
#[test]
fn a_camera_that_fails_a_gate_starts_on_camera_matching() {
    let s = Session::new();
    {
        let (_, _g) = mac(true, false, Some(SIZE));
        assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Auto, "name gate");
    }
    let (_, _g) = mac(true, true, Some((6000, 4000)));
    let p = imported(&s, "SONY ILCE-7M5");
    assert_eq!(p.develop.profile.camera_colour, CameraColour::Auto, "size gate");
    assert!(p.import_look.is_none() && !p.is_edited());
}

/// T31: off the Mac the photo starts on Camera Matching while the setting stays macOS Colour.
#[test]
fn off_the_mac_a_new_raw_starts_on_camera_matching() {
    let (probes, _g) = mac(false, true, Some(SIZE));
    let s = Session::new();
    assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Auto);
    assert_eq!(s.import_defaults.starting_colour, StartingColour::Macos);
    assert_eq!(probes.load(Ordering::SeqCst), 0, "nothing probed off the Mac");
}

/// T32: Camera Matching and No Camera Matching give `auto` and `colorimetric`, whatever the Mac can do.
#[test]
fn the_other_choices_give_auto_and_colorimetric() {
    let (_, _g) = mac(true, true, Some(SIZE));
    let mut s = Session::new();
    s.import_defaults.starting_colour = StartingColour::CameraMatching;
    assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Auto);
    s.import_defaults.starting_colour = StartingColour::NoCameraMatching;
    let p = imported(&s, "SONY ILCE-7M5");
    assert_eq!(p.develop.profile.camera_colour, CameraColour::Colorimetric);
    assert!(!p.is_edited());
}

/// T33: a raw default preset that names a colour source wins over the setting; one without the Profile group doesn't.
#[test]
fn a_default_preset_with_a_colour_source_wins() {
    let (_, _g) = mac(true, true, Some(SIZE));
    let mut s = Session::new();
    let with = lightcraft_develop::Preset {
        id: "p.auto".into(),
        name: "Auto Colour".into(),
        group: "User".into(),
        settings: json!({"profile": {"camera_colour": "auto"}, "light": {"exposure": 0.5}}),
        favorite: false,
        builtin: false,
    };
    let without = lightcraft_develop::Preset { id: "p.look".into(), settings: json!({"light": {"exposure": 0.5}}), ..with.clone() };
    s.presets.push(with);
    s.presets.push(without);
    s.import_defaults.raw_preset = Some("p.auto".into());
    assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Auto, "the preset's colour wins");
    s.import_defaults.raw_preset = Some("p.look".into());
    let p = imported(&s, "SONY ILCE-7M5");
    assert_eq!(p.develop.profile.camera_colour, CameraColour::Macos);
    assert!((p.develop.light.exposure - 0.5).abs() < 1e-9 && !p.is_edited());
}

/// T34: changing the setting leaves photos already in the library byte for byte as they were.
#[test]
fn changing_the_setting_leaves_existing_photos_alone() {
    let mut s = Session::new();
    for i in 1..=3 {
        s.catalog.apply(Op::AddPhoto { photo: Box::new(raw(i, "SONY ILCE-7M5")) }).unwrap();
    }
    let before: Vec<String> = s.catalog.photos().map(|p| serde_json::to_string(p).unwrap()).collect();
    s.execute("library.preferences", &json!({"import": {"startingColour": "cameraMatching"}})).unwrap();
    s.execute("library.preferences", &json!({"import": {"startingColour": "macos"}})).unwrap();
    let after: Vec<String> = s.catalog.photos().map(|p| serde_json::to_string(p).unwrap()).collect();
    assert_eq!(before, after);
}

/// T35: the size gate probes once per camera and size, not once per photo.
#[test]
fn the_size_gate_probes_once_per_camera_and_size() {
    let (probes, _g) = mac(true, true, Some(SIZE));
    let s = Session::new();
    for _ in 0..50 {
        assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Macos);
    }
    assert_eq!(probes.load(Ordering::SeqCst), 1);
}

/// A probe that could not read the file gives no verdict: the next photo of the same camera and size is probed again
/// and starts on macOS Colour when its file reads.
#[test]
fn a_failed_probe_is_not_remembered() {
    let (probes, _g) = mac(true, true, None);
    let s = Session::new();
    assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Auto);
    assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Auto);
    assert_eq!(probes.load(Ordering::SeqCst), 2, "probed again");
    let (_, _g) = mac(true, true, Some(SIZE));
    let verdicts_kept = GATE_FOR_TESTS.with(|g| g.borrow().clone()).unwrap();
    assert!(verdicts_kept.1.0.lock().unwrap().is_empty());
    assert_eq!(imported(&s, "SONY ILCE-7M5").develop.profile.camera_colour, CameraColour::Macos);
}
