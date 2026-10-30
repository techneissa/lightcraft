//! The Info panel's editable camera card: typing in a row edits the selected photos' camera, lens or exposure
//! value (the file's stays), the pickers search the known names, a bad value says why, Mixed shows where the
//! selection differs, and the reset buttons (icon only, one size and place) give the file's values back.

use std::time::Duration;

use lightcraft_catalog::{ExifField, PhotoId};
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(60);

/// The demo library, the Info panel open, photos 0 and 1 selected, each with known file values.
fn info_panel() -> (Headless, Vec<PhotoId>) {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1500.0, 1700.0], 1.0);
    let ids: Vec<PhotoId> = h.app.session.visible_cloned().into_iter().take(2).collect();
    for (i, id) in ids.iter().enumerate() {
        let mut m = h.app.session.catalog.photo(*id).unwrap().meta.clone();
        m.camera = "SONY ILCE-7M4".into();
        m.lens = "FE 50mm F1.8".into();
        m.focal_mm = Some(50.0);
        m.aperture = Some(1.8);
        m.shutter = "1/125".into();
        m.iso = Some(100 + 100 * i as u32);
        h.app.session.catalog.apply(lightcraft_catalog::Op::SetMeta { id: *id, meta: Box::new(m) }).unwrap();
    }
    h.app.ui.right = RightPanel::Info;
    let ids_json: Vec<u64> = ids.iter().map(|i| i.0).collect();
    run(&mut h, "library.select", json!({"ids": ids_json, "active": ids[0].0}));
    (h, ids)
}

fn run(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
}

fn ask(h: &mut Headless, method: &str, params: serde_json::Value) {
    let r = h.request(method, params.clone(), T);
    assert_eq!(r["ok"], true, "{method} {params}: {r}");
    h.settle(SETTLE);
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn rect(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no {id} on screen"))
}

fn meta(h: &Headless, id: PhotoId) -> lightcraft_catalog::Meta {
    h.app.session.catalog.photo(id).unwrap().meta.clone()
}

fn type_into(h: &mut Headless, field: &str, text: &str) {
    ask(h, "ui.clickWidget", json!({"id": format!("field:exif:{field}")}));
    ask(h, "ui.text", json!({"text": text}));
}

fn toast(h: &Headless) -> String {
    h.app.ui.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
}

#[test]
fn typing_a_number_edits_every_selected_photo_and_reset_gives_the_file_back() {
    let (mut h, ids) = info_panel();
    type_into(&mut h, "iso", "1600");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    for id in &ids {
        let m = meta(&h, *id);
        assert_eq!((m.effective_iso(), m.iso.is_some()), (Some(1600), true), "photo {id:?}");
    }
    assert_eq!(meta(&h, ids[1]).iso, Some(200), "the file's value is untouched");
    assert!(has(&h, "icon:exifReset:iso") && !has(&h, "icon:exifReset:lens"), "a reset button only where something is edited");
    ask(&mut h, "ui.clickWidget", json!({"id": "icon:exifReset:iso"}));
    for (id, file) in ids.iter().zip([100, 200]) {
        assert_eq!((meta(&h, *id).effective_iso(), meta(&h, *id).exif.is_empty()), (Some(file), true));
    }
    // the shutter, aperture and focal length take their usual spellings
    for (field, text) in [("shutter", "2.5\""), ("aperture", "f/4"), ("focal", "85 mm")] {
        type_into(&mut h, field, text);
        ask(&mut h, "ui.key", json!({"key": "Enter"}));
    }
    let m = meta(&h, ids[0]);
    assert_eq!((m.effective_shutter(), m.effective_aperture(), m.effective_focal_mm()), ("2.5", Some(4.0), Some(85.0)));
    // Reset All takes every edit back at once
    ask(&mut h, "ui.clickWidget", json!({"id": "button:exifResetAll"}));
    assert!(ids.iter().all(|id| meta(&h, *id).exif.is_empty()));
    assert!(!has(&h, "button:exifResetAll"), "nothing left to reset");
}

#[test]
fn a_bad_value_says_why_and_changes_nothing() {
    let (mut h, ids) = info_panel();
    let before = h.app.session.catalog.to_snapshot();
    type_into(&mut h, "iso", "lots");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(toast(&h).contains("ISO"), "{:?}", toast(&h));
    assert_eq!(h.app.session.catalog.to_snapshot(), before);
    let _ = ids;
    type_into(&mut h, "shutter", "1/0");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert_eq!(h.app.session.catalog.to_snapshot(), before, "a hostile shutter text changes nothing");
}

#[test]
fn the_camera_picker_searches_the_known_names_and_takes_free_text() {
    let (mut h, ids) = info_panel();
    type_into(&mut h, "camera", "A7 V");
    assert!(has(&h, "exifSuggestion:camera:0"), "typing lists matches");
    // (the press picks, so the row is gone by its release: click by position)
    let at = rect(&h, "exifSuggestion:camera:0").center();
    ask(&mut h, "ui.click", json!({"x": at.x, "y": at.y}));
    assert_eq!(meta(&h, ids[0]).effective_camera(), "Sony ILCE-7M5", "a pick stores the full name");
    assert_eq!(meta(&h, ids[1]).effective_camera(), "Sony ILCE-7M5", "on every selected photo");
    assert_eq!(meta(&h, ids[0]).camera, "SONY ILCE-7M4", "the file's camera stays");
    assert!(!has(&h, "exifSuggestion:camera:0"), "the list closes");
    // free text, however odd
    type_into(&mut h, "camera", "My pinhole box");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert_eq!(meta(&h, ids[0]).effective_camera(), "My pinhole box");
    // the lens list finds a lens by part of its name
    type_into(&mut h, "lens", "FE 24-70");
    assert!(has(&h, "exifSuggestion:lens:0"));
    ask(&mut h, "ui.key", json!({"key": "ArrowDown"}));
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(meta(&h, ids[0]).effective_lens().contains("24-70"), "{:?}", meta(&h, ids[0]).effective_lens());
    ask(&mut h, "ui.key", json!({"key": "Escape"}));
}

#[test]
fn selected_photos_that_differ_say_mixed() {
    let (mut h, ids) = info_panel();
    run(&mut h, "view.infoSelectedPhotos", json!({"on": true}));
    let text = h.painted_text();
    // (other fields of the demo photos differ too: count the ones that are left)
    let mixed = |h: &Headless| h.painted_text().iter().filter(|t| *t == "Mixed").count();
    let before = mixed(&h);
    assert!(before >= 1, "ISO differs between the photos: {text:?}");
    // the shared values are shown
    assert!(text.iter().any(|t| t.contains("SONY ILCE-7M4")), "{text:?}");
    // typing over Mixed sets all of them
    type_into(&mut h, "iso", "800");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(ids.iter().all(|id| meta(&h, *id).effective_iso() == Some(800)));
    assert_eq!(mixed(&h), before - 1, "ISO no longer says Mixed");
}

#[test]
fn reset_buttons_are_one_size_in_one_place_on_every_row() {
    let (mut h, ids) = info_panel();
    let slot_rects: Vec<egui::Rect> = ExifField::ALL.iter().map(|f| rect(&h, &format!("exifResetSlot:{}", f.key()))).collect();
    // edit every field of the active photo
    let id = ids[0].0;
    run(
        &mut h,
        "photo.setMeta",
        json!({"ids": [id], "camera": "Canon EOS R5", "lens": "RF50mm F1.2", "focal": 50.5, "aperture": 1.2, "shutter": "1/8000", "iso": 64}),
    );
    run(&mut h, "library.select", json!({"ids": [id], "active": id}));
    let button_rects: Vec<egui::Rect> = ExifField::ALL.iter().map(|f| rect(&h, &format!("icon:exifReset:{}", f.key()))).collect();
    for (b, s) in button_rects.iter().zip(&slot_rects) {
        assert_eq!(b.size(), s.size(), "the button fills the slot it had");
        assert_eq!(b.size(), button_rects[0].size(), "one size");
        assert!((b.min.x - button_rects[0].min.x).abs() < 0.5 && (b.min.x - s.min.x).abs() < 0.5, "one column: {b:?} {s:?}");
    }
    // no text on them: the button is the code-drawn icon only, named for screen readers
    assert!(!h.painted_text().iter().any(|t| t.contains('↺') || t == "Reset"), "{:?}", h.painted_text());
}
