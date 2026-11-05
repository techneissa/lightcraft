//! The Info panel's editable camera card: at rest it is plain text like the official card; clicking a value opens
//! a text field for it alone, and typing there edits the selected photos' camera, lens or exposure value (the file's
//! stays); the pickers search the known names, a bad value says why, an aperture or focal length the lens cannot give
//! is refused with a "!" that says why, Mixed shows where the selection differs, and the reset buttons (icon only, one
//! size and place) give the file's values back.

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
        m.lens = "FE 24-105mm F1.8".into();
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

/// Click a value (it turns into a text field) and type over it.
fn type_into(h: &mut Headless, field: &str, text: &str) {
    ask(h, "ui.clickWidget", json!({"id": format!("exifValue:{field}")}));
    assert!(has(h, &format!("field:exif:{field}")), "clicking {field} opens its field");
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
    // the command takes every edit back at once; the card has no button for it
    assert!(!has(&h, "button:exifResetAll"), "the card has no Reset All button");
    run(&mut h, "photo.resetExif", json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>()}));
    assert!(ids.iter().all(|id| meta(&h, *id).exif.is_empty()));
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
    assert!(ExifField::ALL.iter().all(|f| !has(&h, &format!("icon:exifReset:{}", f.key()))), "nothing edited, no reset buttons");
    // edit every field of the active photo
    let id = ids[0].0;
    run(
        &mut h,
        "photo.setMeta",
        json!({"ids": [id], "camera": "Canon EOS R5", "lens": "RF50mm F1.2", "focal": 50.4, "aperture": 1.2, "shutter": "1/8000", "iso": 64}),
    );
    run(&mut h, "library.select", json!({"ids": [id], "active": id}));
    let button_rects: Vec<egui::Rect> = ExifField::ALL.iter().map(|f| rect(&h, &format!("icon:exifReset:{}", f.key()))).collect();
    for b in &button_rects {
        assert_eq!(b.size(), button_rects[0].size(), "one size");
        assert!(b.width() <= 16.0, "small: {b:?}");
    }
    // (one column in the rows that have nothing of their own at the right edge: Lens, Focal length, Shutter speed)
    for i in [2, 4] {
        assert!((button_rects[i].min.x - button_rects[1].min.x).abs() < 0.5, "one column: {button_rects:?}");
    }
    // no text on them: the button is the code-drawn icon only, named for screen readers
    assert!(!h.painted_text().iter().any(|t| t.contains('↺') || t == "Reset"), "{:?}", h.painted_text());
}

const KEYS: [&str; 6] = ["camera", "lens", "focal", "shutter", "aperture", "iso"];

#[test]
fn at_rest_the_card_is_plain_text_with_no_text_box() {
    let (h, _) = info_panel();
    for key in KEYS {
        assert!(has(&h, &format!("exifValue:{key}")), "{key} shows its value as text");
        assert!(!has(&h, &format!("field:exif:{key}")), "{key} draws no text box at rest");
    }
    assert!(!has(&h, "button:exifResetAll") && !has(&h, "exifAlert:aperture"), "no Reset All, no alert while nothing is edited");
    let text = h.painted_text();
    for label in ["Camera", "Lens", "Focal length", "Shutter speed", "Aperture", "ISO"] {
        assert!(text.iter().any(|t| t == label), "the {label} label: {text:?}");
    }
    // the lens line carries its own dim label, so "24-105mm F1.8" is not read as a focal length and an aperture
    assert!(text.iter().any(|t| t == "FE 24-105mm F1.8"), "{text:?}");
}

#[test]
fn a_click_opens_the_field_of_that_value_only_and_enter_saves_escape_cancels() {
    let (mut h, ids) = info_panel();
    ask(&mut h, "ui.clickWidget", json!({"id": "exifValue:iso"}));
    for key in KEYS {
        assert_eq!(has(&h, &format!("field:exif:{key}")), key == "iso", "{key}");
    }
    // Esc leaves the value as it was
    ask(&mut h, "ui.text", json!({"text": "6400"}));
    ask(&mut h, "ui.key", json!({"key": "Escape"}));
    assert_eq!(meta(&h, ids[0]).effective_iso(), Some(100));
    assert!(!has(&h, "field:exif:iso"), "Esc closes the field");
    // Enter saves through photo.setMeta, on the selected photos
    type_into(&mut h, "iso", "6400");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(ids.iter().all(|id| meta(&h, *id).effective_iso() == Some(6400)));
    assert!(!has(&h, "field:exif:iso"), "Enter closes the field");
    // clicking away saves too
    type_into(&mut h, "shutter", "1/500");
    ask(&mut h, "ui.clickWidget", json!({"id": "exifValue:focal"}));
    assert_eq!(meta(&h, ids[0]).effective_shutter(), "1/500");
    assert!(has(&h, "field:exif:focal") && !has(&h, "field:exif:shutter"), "the click went on to open the next value");
}

/// The rows' rectangles, top to bottom (Camera, Lens, Focal length, Shutter speed, Aperture, ISO).
fn rows(h: &Headless) -> Vec<egui::Rect> {
    ["camera", "lens", "focal", "shutter", "aperture", "iso"].iter().map(|k| rect(h, &format!("exifRow:{k}"))).collect()
}

#[test]
fn the_rows_are_roomy_and_the_card_is_a_bounded_height() {
    let (mut h, _) = info_panel();
    ask(&mut h, "ui.set", json!({"rightWidth": 480}));
    assert!(!has(&h, "info:focus"), "the demo photo has no focus point");
    let r = rows(&h);
    for pair in r.windows(2) {
        assert!(pair[1].top() - pair[0].bottom() >= 5.0, "a gap between rows: {pair:?}");
    }
    assert!(r.iter().all(|row| (row.height() - 24.0).abs() < 0.5), "one-line rows are one height: {r:?}");
    let card = rect(&h, "info:card");
    assert!(card.height() <= 6.0 * 24.0 + 5.0 * 6.0 + 20.0 + 1.5, "at rest the card is {} tall", card.height());
    // with something edited: no Reset All row and no "Edited" label, so the card does not grow
    type_into(&mut h, "iso", "6400");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(!has(&h, "button:exifResetAll"), "no Reset All button");
    assert!(!h.painted_text().iter().any(|t| t == "Edited" || t == "Reset All"), "no Edited label");
    assert!(rect(&h, "info:card").height() <= card.height() + 0.5, "edited the card is {} tall", rect(&h, "info:card").height());
}

#[test]
fn the_edit_marks_sit_halfway_down_a_wrapped_value() {
    let (mut h, ids) = info_panel();
    let long = "Sigma 24-70mm F2.8 DG DN II Art Mirrorless Zoom for Sony E and L-mount Extra Long Name";
    run(&mut h, "photo.setMeta", json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "lens": long}));
    let (value, reset) = (rect(&h, "exifValue:lens"), rect(&h, "icon:exifReset:lens"));
    assert!(value.height() > 24.5, "the lens name wraps: {value:?}");
    assert!((value.center().y - reset.center().y).abs() < 1.0, "reset {reset:?} is level with the middle of {value:?}");
}

#[test]
fn the_format_badge_and_the_size_stand_at_the_right_edge_level_with_their_rows() {
    let (h, _) = info_panel();
    let (r, card) = (rows(&h), rect(&h, "info:card"));
    let level = |key: &str, row: egui::Rect| {
        let t = rect(&h, &format!("exifTrailing:{key}"));
        assert!((t.center().y - row.top() - 12.0).abs() < 1.5, "{key} is level with its row: {t:?} {row:?}");
        assert!(t.right() <= card.right() && card.right() - t.right() < 14.0, "{key} is at the card's right edge: {t:?} {card:?}");
        t
    };
    let badge = level("camera", r[0]);
    let (dims, size) = (level("aperture", r[4]), level("iso", r[5]));
    assert!(!has(&h, "exifTrailing:lens") && !has(&h, "exifTrailing:focal") && !has(&h, "exifTrailing:shutter"));
    assert!(badge.width() < 60.0 && dims.right() == size.right(), "the two texts are right aligned");
    let text = h.painted_text();
    assert!(text.iter().any(|t| t.contains(" × ") && !t.contains('·')), "the dimensions are a text of their own: {text:?}");
    assert!(!text.iter().any(|t| t.contains('·')), "no combined size line any more: {text:?}");
}

#[test]
fn editing_a_value_moves_no_row() {
    let (mut h, _) = info_panel();
    let (rest, card) = (rows(&h), rect(&h, "info:card"));
    for key in KEYS {
        ask(&mut h, "ui.clickWidget", json!({"id": format!("exifValue:{key}")}));
        assert!(has(&h, &format!("field:exif:{key}")), "{key} is open");
        assert_eq!(rows(&h), rest, "editing {key} moved a row");
        assert_eq!(rect(&h, "info:card"), card, "editing {key} moved the card");
        let field = rect(&h, &format!("field:exif:{key}"));
        let row = rect(&h, &format!("exifRow:{key}"));
        assert!(field.height() <= row.height() + 0.5 && field.top() >= row.top() - 0.5, "{key}: the field fits its row: {field:?} {row:?}");
        ask(&mut h, "ui.key", json!({"key": "Escape"}));
        assert_eq!(rows(&h), rest, "closing {key} moved a row");
    }
}

#[test]
fn a_lens_name_too_long_for_the_row_wraps_and_only_the_rows_below_move() {
    let (mut h, ids) = info_panel();
    let rest = rows(&h);
    let long = "Sigma 24-70mm F2.8 DG DN II Art Mirrorless Zoom Lens for Sony E and Leica L mounts";
    run(&mut h, "library.select", json!({"ids": [ids[0].0], "active": ids[0].0}));
    run(&mut h, "photo.setMeta", json!({"ids": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "lens": long}));
    let r = rows(&h);
    assert!(r[1].height() > 24.0 + 10.0, "the lens row is taller for a second line: {:?}", r[1]);
    assert_eq!(r[0], rest[0], "the row above stays");
    let shift = r[1].height() - rest[1].height();
    for i in 2..6 {
        assert!((r[i].top() - rest[i].top() - shift).abs() < 0.5 && r[i].height() == rest[i].height(), "row {i} moves down by the extra line only");
    }
    let card = rect(&h, "info:card");
    assert!(r[1].right() <= card.right() + 0.5, "the name stays inside the card");
    // editing it still moves nothing
    ask(&mut h, "ui.clickWidget", json!({"id": "exifValue:lens"}));
    assert!(has(&h, "field:exif:lens"));
    assert_eq!(rows(&h), r, "editing a wrapped name moved a row");
    assert_eq!(rect(&h, "info:card"), card);
    ask(&mut h, "ui.key", json!({"key": "Escape"}));
}

/// Selects photo 0 alone with a zoom lens that opens to f/4 at most.
fn photo_with_lens(h: &mut Headless, ids: &[PhotoId], lens: &str) {
    run(h, "library.select", json!({"ids": [ids[0].0], "active": ids[0].0}));
    run(h, "photo.setMeta", json!({"ids": [ids[0].0], "lens": lens}));
}

#[test]
fn a_refused_aperture_keeps_the_value_shows_an_alert_and_the_alert_says_why() {
    let (mut h, ids) = info_panel();
    photo_with_lens(&mut h, &ids, "16-35mm f/4");
    assert!(!has(&h, "exifAlert:aperture"), "the file's own f/1.8 is shown as it is, no alert");
    let before = h.app.session.catalog.to_snapshot();
    type_into(&mut h, "aperture", "f/2.8");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert_eq!(h.app.session.catalog.to_snapshot(), before, "a refused edit changes nothing");
    assert!(toast(&h).is_empty(), "no message line: {:?}", toast(&h));
    assert!(!has(&h, "field:exif:aperture"), "the field closes on the old value");
    assert!(h.painted_text().iter().any(|t| t == "f / 1.8"), "{:?}", h.painted_text());
    assert!(has(&h, "exifAlert:aperture") && !has(&h, "exifAlert:focal"), "a small ! at the end of that row only");
    assert!(!h.painted_text().iter().any(|t| t.contains("opens to")), "the reason shows only when the ! is clicked");
    ask(&mut h, "ui.clickWidget", json!({"id": "exifAlert:aperture"}));
    assert!(h.painted_text().iter().any(|t| t == "This lens opens to f/4 at most"), "{:?}", h.painted_text());
    // a focal length outside the range is refused the same way
    type_into(&mut h, "focal", "85 mm");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert_eq!(h.app.session.catalog.to_snapshot(), before);
    assert!(has(&h, "exifAlert:focal"));
    // a value the lens can give is taken, and its alert goes
    type_into(&mut h, "aperture", "f/5.6");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert_eq!(meta(&h, ids[0]).effective_aperture(), Some(5.6));
    assert!(!has(&h, "exifAlert:aperture"), "the alert goes once the value is fine");
}

#[test]
fn a_lens_change_keeps_an_edit_and_marks_it_while_it_is_out_of_range() {
    let (mut h, ids) = info_panel();
    photo_with_lens(&mut h, &ids, "24-70mm f/2.8");
    type_into(&mut h, "aperture", "f/2.8");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    type_into(&mut h, "focal", "30");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(!has(&h, "exifAlert:aperture") && !has(&h, "exifAlert:focal"), "both fit the lens");
    run(&mut h, "photo.setMeta", json!({"ids": [ids[0].0], "lens": "70-200mm f/4"}));
    assert_eq!((meta(&h, ids[0]).effective_aperture(), meta(&h, ids[0]).effective_focal_mm()), (Some(2.8), Some(30.0)), "the edits stay");
    assert!(has(&h, "exifAlert:aperture") && has(&h, "exifAlert:focal"), "both are now out of range, and say so");
    assert!(!has(&h, "exifAlert:iso") && !has(&h, "exifAlert:lens"));
    // a lens that fits again clears them
    run(&mut h, "photo.setMeta", json!({"ids": [ids[0].0], "lens": "24-70mm f/2.8"}));
    assert!(!has(&h, "exifAlert:aperture") && !has(&h, "exifAlert:focal"));
}

#[test]
fn a_refusal_is_over_when_the_next_edit_begins_and_the_files_own_values_are_never_flagged() {
    let (mut h, ids) = info_panel();
    photo_with_lens(&mut h, &ids, "16-35mm f/4");
    // the file's own f/1.8 sits on a lens that opens to f/4 only: shown as it is, never flagged
    assert!(!has(&h, "exifAlert:aperture"));
    type_into(&mut h, "aperture", "f/2");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(has(&h, "exifAlert:aperture"), "the refused edit is the one on screen");
    assert!(h.painted_text().iter().any(|t| t == "f / 1.8"), "the value is still the file's");
    // the next edit begins (any value): the old refusal no longer marks the file's value
    ask(&mut h, "ui.clickWidget", json!({"id": "exifValue:iso"}));
    assert!(!has(&h, "exifAlert:aperture"), "a ! must not stay on a row that shows the file's own value");
    ask(&mut h, "ui.key", json!({"key": "Escape"}));
    assert!(!has(&h, "exifAlert:aperture") && !has(&h, "exifAlert:iso"));
    // refused again, then a reset: gone as well
    type_into(&mut h, "aperture", "f/2");
    ask(&mut h, "ui.key", json!({"key": "Enter"}));
    assert!(has(&h, "exifAlert:aperture"));
    run(&mut h, "photo.setMeta", json!({"ids": [ids[0].0], "iso": 800}));
    run(&mut h, "photo.resetExif", json!({"ids": [ids[0].0]}));
    assert!(!has(&h, "exifAlert:aperture"), "a reset ends it too");
}

#[test]
fn a_click_on_camera_or_lens_opens_the_list_of_names_but_not_on_a_number() {
    let (mut h, _) = info_panel();
    for key in ["camera", "lens"] {
        ask(&mut h, "ui.clickWidget", json!({"id": format!("exifValue:{key}")}));
        assert!(has(&h, &format!("exifSuggestion:{key}:0")), "{key}: the list opens on the click, before anything is typed");
        ask(&mut h, "ui.key", json!({"key": "Escape"}));
        assert!(!has(&h, &format!("exifSuggestion:{key}:0")), "{key}: Esc closes it");
    }
    ask(&mut h, "ui.clickWidget", json!({"id": "exifValue:iso"}));
    assert!(!has(&h, "exifSuggestion:iso:0") && !has(&h, "exifSuggestion:camera:0"));
}
