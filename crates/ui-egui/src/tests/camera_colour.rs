//! The Profile menu's Camera Matching choice: "Camera Matching" (the default) and "No Camera Matching" are listed for a
//! raw whose starting colour is fitted to its JPEG, one click sets the photo and its next decode, and one undo goes
//! back. A photo that has no such choice (a JPEG, a DNG with its own matrices) is not offered it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;
use lightcraft_engine::media::{FileLoader, LoadOptions};
use lightcraft_pipeline::SourceInfo;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The Edit panel on one raw whose decoder says whether its colour is fitted to the JPEG; the decodes' options.
fn edit_panel(offered: bool) -> (Headless, Arc<Mutex<Vec<LoadOptions>>>) {
    panel(offered, None)
}

/// [`edit_panel`] with a note the decoder gives a photo on a local profile it can't use.
fn panel(offered: bool, note: Option<&'static str>) -> (Headless, Arc<Mutex<Vec<LoadOptions>>>) {
    let mut session = lightcraft_engine::Session::new();
    let mut p = Photo::new(PhotoId(1), Source::File { path: "/photos/A7V01320.ARW".into() }, "A7V01320.ARW", "ARW", 7008, 4672, "2026-10-26");
    p.kind = lightcraft_catalog::MediaKind::Raw;
    p.meta.camera = "SONY ILCE-7M5".into();
    session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    // (found here, on the test's thread: the decoder runs on a worker thread that doesn't see the test's folder)
    let usable_key = lightcraft_engine::camera_profiles::local_key("A7 V Chart");
    let loader: FileLoader = Arc::new(move |_, _, options| {
        log.lock().unwrap().push(options);
        let mut img = lightcraft_raster::Rgb32f::new(48, 32);
        img.data.fill([0.18; 3]);
        // (the decoder's note: only for a local profile other than the usable "A7 V Chart" of the test folder)
        let usable = options.profile_key == Some(usable_key);
        let colour_note = note.filter(|_| options.colour == CameraColour::Local && !usable).map(str::to_string);
        Ok((img, SourceInfo { raw: true, colorimetric: offered, colour_note, ..SourceInfo::default() }))
    });
    session.media.file_loader = Some(loader);
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    for (method, params) in
        [("engine.execute", json!({"command": "library.select", "params": {"ids": [1]}})), ("ui.set", json!({"view": "detail", "right": "edit"}))]
    {
        let r = h.request(method, params, T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.settle(SETTLE);
    (h, seen)
}

fn widget(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn colour(h: &Headless) -> CameraColour {
    h.app.session.catalog.photo(PhotoId(1)).unwrap().develop.profile.camera_colour
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

#[test]
fn camera_matching_is_chosen_in_the_profile_menu() {
    let (mut h, seen) = edit_panel(true);
    assert!(widget(&h, "label:profile.cameraColour"), "the Edit panel says which way the colour starts");
    assert_eq!(colour(&h), CameraColour::Auto, "Camera Matching is the default");
    click(&mut h, "dropdown:profile");
    assert!(widget(&h, "profileMenu:cameraColour:auto") && widget(&h, "profileMenu:cameraColour:colorimetric"), "both entries are listed");
    click(&mut h, "profileMenu:cameraColour:colorimetric");
    assert_eq!(colour(&h), CameraColour::Colorimetric);
    assert_eq!(seen.lock().unwrap().last().map(|o| o.colour), Some(CameraColour::Colorimetric), "decoded again without the match");
    // one undo step back to Camera Matching
    let r = h.request("engine.execute", json!({"command": "edit.undo"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(colour(&h), CameraColour::Auto);
    assert!(widget(&h, "label:profile.cameraColour"), "still offered");
}

#[test]
fn camera_matching_is_not_offered_where_the_colour_is_not_matched() {
    let (mut h, _) = edit_panel(false);
    assert!(!widget(&h, "label:profile.cameraColour"));
    click(&mut h, "dropdown:profile");
    assert!(!widget(&h, "profileMenu:cameraColour:auto") && !widget(&h, "profileMenu:cameraColour:colorimetric"));
}

/// A folder of profile files: one for the photo's camera (named "A7 V Chart"), one for another body.
fn profiles_folder(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-ui-local-profiles-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let profile = |model: &str| lightcraft_engine::camera_profiles::CameraProfile::new(model, 40, 9000, lightcraft_color::Mat3::IDENTITY, None);
    lightcraft_engine::camera_profiles::save_named(&profile("ILCE-7M5"), &dir, "A7 V Chart").unwrap();
    lightcraft_engine::camera_profiles::save_named(&profile("ILCE-7M4"), &dir, "Other Body").unwrap();
    dir
}

fn stored(h: &Headless) -> (CameraColour, Option<String>) {
    let p = &h.app.session.catalog.photo(PhotoId(1)).unwrap().develop.profile;
    (p.camera_colour, p.camera_profile.clone())
}

/// LT10: the Profile menu and the profile browser list the local profiles made for the photo's camera (not another
/// body's), by name; one click chooses it (one undo step); a profile the photo stores but cannot use is listed greyed,
/// with the permanent note.
#[test]
fn the_profile_menu_lists_the_local_profiles_of_the_photos_camera() {
    let dir = profiles_folder("menu");
    let _dir = lightcraft_engine::camera_profiles::use_dir_for_tests(&dir);
    let (mut h, seen) =
        panel(true, Some("Edited with the profile 'Gone', which isn't available on this computer. This picture uses Camera Matching."));
    click(&mut h, "dropdown:profile");
    assert!(widget(&h, "profileMenu:cameraColour:local:A7 V Chart"), "the profile of the photo's camera is listed");
    assert!(!widget(&h, "profileMenu:cameraColour:local:Other Body"), "another body's is not");
    assert!(widget(&h, "profileMenu:cameraColour:auto") && widget(&h, "profileMenu:cameraColour:colorimetric"));
    click(&mut h, "profileMenu:cameraColour:local:A7 V Chart");
    assert_eq!(stored(&h), (CameraColour::Local, Some("A7 V Chart".to_string())));
    let last = *seen.lock().unwrap().last().unwrap();
    assert_eq!(last.colour, CameraColour::Local);
    assert!(last.profile_key.is_some(), "decoded with the profile's file");
    assert!(!widget(&h, "label:profile.cameraColourNote"), "the profile is usable: no note");
    // one undo step back
    let r = h.request("engine.execute", json!({"command": "edit.undo"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(stored(&h), (CameraColour::Auto, None));
    // a stored profile with no file here: a greyed row and the note, the setting kept
    let r = h.request("engine.execute", json!({"command": "develop.profile", "params": {"camera_colour": "local", "camera_profile": "Gone"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(widget(&h, "label:profile.cameraColourNote"), "the note says the picture uses Camera Matching");
    click(&mut h, "dropdown:profile");
    assert!(widget(&h, "profileMenu:cameraColour:local:Gone"), "the stored profile is listed, greyed");
    assert_eq!(stored(&h), (CameraColour::Local, Some("Gone".to_string())));
    // the profile browser shows the live cells too
    let r = h.request("engine.execute", json!({"command": "panel.profiles"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(widget(&h, "cameraColourCell:local:A7 V Chart") && widget(&h, "cameraColourCell:auto"), "cells for every row");
    let _ = std::fs::remove_dir_all(&dir);
}

/// LT11: a profile file named after its camera (`ILCE-7M5.json`) used to change that camera's photos inside Camera
/// Matching. The question is put once when the library opens, changes nothing by itself, Switch moves the photos in one
/// undo step, and Leave on Camera Matching leaves them; neither is asked again.
#[test]
fn the_switch_question_is_asked_once_and_moves_nothing_by_itself() {
    let dir = std::env::temp_dir().join(format!("lc-ui-local-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let profile = lightcraft_engine::camera_profiles::CameraProfile::new("ILCE-7M5", 40, 9000, lightcraft_color::Mat3::IDENTITY, None);
    lightcraft_engine::camera_profiles::save(&profile, &dir).unwrap();
    let _dir = lightcraft_engine::camera_profiles::use_dir_for_tests(&dir);
    for leave in [false, true] {
        let (mut h, _) = edit_panel(true);
        for _ in 0..4 {
            h.step();
        }
        let asked =
            |h: &Headless| matches!(&h.app.ui.dialog, Some(crate::state::Dialog::LocalProfileSwitch { id, count: 1, .. }) if id == "ILCE-7M5");
        assert!(asked(&h), "asked when the library opened: {:?}", h.app.ui.dialog);
        assert_eq!(stored(&h), (CameraColour::Auto, None), "asking moves nothing");
        assert!(h.app.session.local_profile_asked.is_empty(), "asking is not answering");
        click(&mut h, if leave { "button:dialogCancel" } else { "button:dialogOk" });
        assert!(h.app.ui.dialog.is_none());
        assert_eq!(h.app.session.local_profile_asked, ["ILCE-7M5"], "Switch and Leave both answer it");
        if leave {
            assert_eq!(stored(&h), (CameraColour::Auto, None), "Leave on Camera Matching");
        } else {
            assert_eq!(stored(&h), (CameraColour::Local, Some("ILCE-7M5".to_string())), "Switch");
            let r = h.request("engine.execute", json!({"command": "edit.undo"}), T);
            assert_eq!(r["ok"], true, "{r}");
            h.settle(SETTLE);
            assert_eq!(stored(&h), (CameraColour::Auto, None), "one undo step");
            assert!(h.app.session.local_profile_asked.is_empty(), "undoing the Switch puts the question back");
            crate::local_profile_offer::check(&mut h.app);
            assert!(asked(&h), "and it is asked again");
            click(&mut h, "button:dialogCancel");
        }
        // not asked again
        crate::local_profile_offer::check(&mut h.app);
        assert!(h.app.ui.dialog.is_none(), "asked once");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Closing the question with Esc is not an answer: nothing moves, nothing is remembered, and the next library opening
/// asks again.
#[test]
fn closing_the_switch_question_with_esc_leaves_it_open_for_next_time() {
    let dir = std::env::temp_dir().join(format!("lc-ui-local-esc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let profile = lightcraft_engine::camera_profiles::CameraProfile::new("ILCE-7M5", 40, 9000, lightcraft_color::Mat3::IDENTITY, None);
    lightcraft_engine::camera_profiles::save(&profile, &dir).unwrap();
    let _dir = lightcraft_engine::camera_profiles::use_dir_for_tests(&dir);
    let (mut h, _) = edit_panel(true);
    for _ in 0..4 {
        h.step();
    }
    assert!(matches!(&h.app.ui.dialog, Some(crate::state::Dialog::LocalProfileSwitch { .. })), "{:?}", h.app.ui.dialog);
    let r = h.request("ui.key", json!({"key": "Escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert!(h.app.ui.dialog.is_none());
    assert_eq!(stored(&h), (CameraColour::Auto, None), "nothing moved");
    assert!(h.app.session.local_profile_asked.is_empty(), "Esc is not an answer");
    crate::local_profile_offer::check(&mut h.app);
    assert!(matches!(&h.app.ui.dialog, Some(crate::state::Dialog::LocalProfileSwitch { .. })), "asked again");
    let _ = std::fs::remove_dir_all(&dir);
}
