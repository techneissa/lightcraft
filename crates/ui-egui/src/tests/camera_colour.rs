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
    let mut session = lightcraft_engine::Session::new();
    let p = Photo::new(PhotoId(1), Source::File { path: "/photos/A7V01320.ARW".into() }, "A7V01320.ARW", "ARW", 7008, 4672, "2026-10-26");
    session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let loader: FileLoader = Arc::new(move |_, _, options| {
        log.lock().unwrap().push(options);
        let mut img = lightcraft_raster::Rgb32f::new(48, 32);
        img.data.fill([0.18; 3]);
        Ok((img, SourceInfo { raw: true, colorimetric: offered, ..SourceInfo::default() }))
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
