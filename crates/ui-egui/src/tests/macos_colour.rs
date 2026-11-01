//! macOS Colour in the UI (plan §6.1, §6.7): its Profile picker row, the note when the picture uses Camera Matching
//! instead (T14), Apply macOS Colour to All Photos from This Camera… (T27), and Settings ▸ Import ▸ Starting Colour
//! for Raw Photos (T36). The picker row follows this computer's raw decoder, so the tests check what this machine
//! does (a Mac that reads the A7 V, or any other computer).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;
use lightcraft_engine::import::StartingColour;
use lightcraft_engine::media::{FileLoader, LoadOptions};
use lightcraft_pipeline::{MacosUnavailable, SourceInfo};
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

/// The Edit panel on `n` A7 V raws (photo 1 active) whose decoder offers the colour choice and, for a photo set to
/// macOS Colour, says `note`; the decodes' options.
/// (The machine's own camera-profiles folder is replaced by an empty one for as long as the guard lives: a profile file
/// there would ask the one-time switch question over the menus.)
fn panel(n: u64, note: Option<MacosUnavailable>) -> (Headless, Arc<Mutex<Vec<LoadOptions>>>, impl Drop) {
    let guard = super::camera_colour::no_local_profiles(&format!("macos-{n}-{note:?}"));
    let mut session = lightcraft_engine::Session::new();
    for i in 1..=n {
        let path = format!("/photos/A7V0{i}.ARW");
        let mut p = Photo::new(PhotoId(i), Source::File { path: path.clone() }, &path, "ARW", 7008, 4672, "2026-10-28");
        p.kind = MediaKind::Raw;
        p.meta.camera = "SONY ILCE-7M5".into();
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let loader: FileLoader = Arc::new(move |_, _, options| {
        log.lock().unwrap().push(options);
        let mut img = lightcraft_raster::Rgb32f::new(48, 32);
        img.data.fill([0.18; 3]);
        let macos_note = note.filter(|_| options.colour == CameraColour::Macos);
        Ok((img, SourceInfo { raw: true, colorimetric: true, macos_offered: macos_note.is_none(), macos_note, ..SourceInfo::default() }))
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
    (h, seen, guard)
}

fn widget(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn colour(h: &Headless, id: u64) -> CameraColour {
    h.app.session.catalog.photo(PhotoId(id)).unwrap().develop.profile.camera_colour
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.settle(SETTLE);
}

fn run(h: &mut Headless, command: &str, params: serde_json::Value) {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    h.settle(SETTLE);
}

/// Whether this computer's decoder reads photo 1's camera (the row is offered).
fn mac(h: &Headless) -> bool {
    h.app.session.catalog.photo(PhotoId(1)).is_some_and(|p| lightcraft_engine::files::macos_lists(p))
}

/// T14: where this Mac reads the camera, macOS Colour is a third row of the Profile menu and one click puts the photo
/// and its next decode on it; elsewhere it isn't listed for a photo that isn't on it.
#[test]
fn macos_colour_is_a_profile_row_where_this_mac_reads_the_camera() {
    let (mut h, seen, _dir) = panel(1, None);
    click(&mut h, "dropdown:profile");
    assert!(widget(&h, "profileMenu:cameraColour:auto") && widget(&h, "profileMenu:cameraColour:colorimetric"));
    if !mac(&h) {
        assert!(!widget(&h, "profileMenu:cameraColour:macos"), "not offered off the Mac");
        assert!(!widget(&h, "profileMenu:macosAllFromCamera"));
        return;
    }
    assert!(widget(&h, "profileMenu:cameraColour:macos"), "the third row");
    click(&mut h, "profileMenu:cameraColour:macos");
    assert_eq!(colour(&h, 1), CameraColour::Macos);
    assert_eq!(seen.lock().unwrap().last().map(|o| o.colour), Some(CameraColour::Macos), "decoded again on macOS Colour");
    assert!(!widget(&h, "label:profile.macosNote"), "no note while the picture is macOS Colour");
}

/// T14: a photo on macOS Colour whose picture uses Camera Matching keeps its row (greyed off the Mac) and says why
/// under the picker, for as long as it is so.
#[test]
fn a_macos_colour_photo_on_camera_matching_says_so() {
    let (mut h, _, _dir) = panel(1, Some(MacosUnavailable::Size));
    run(&mut h, "develop.profile", json!({"camera_colour": "macos"}));
    assert!(widget(&h, "label:profile.macosNote"), "the permanent note");
    click(&mut h, "dropdown:profile");
    assert!(widget(&h, "profileMenu:cameraColour:macos"), "the stored choice stays listed");
    let row = crate::panels::camera_colour::rows(&h.app, PhotoId(1), &h.app.session.catalog.photo(PhotoId(1)).unwrap().develop)
        .into_iter()
        .find(|r| r.colour == CameraColour::Macos)
        .unwrap();
    assert_eq!(row.enabled, mac(&h), "greyed where this computer can't make it");
    assert_eq!(row.label, "macOS Colour");
}

/// The notes' words, one per reason; AI Denoise's names it.
#[test]
fn each_reason_has_its_note() {
    use crate::macos_colour::picker::note_text;
    assert_eq!(note_text(MacosUnavailable::System), "Edited with macOS Colour. This picture uses Camera Matching, so colours differ.");
    assert_eq!(note_text(MacosUnavailable::Camera), note_text(MacosUnavailable::System));
    assert_eq!(note_text(MacosUnavailable::Size), "macOS reads this raw at a different size, so this picture uses Camera Matching.");
    assert_eq!(note_text(MacosUnavailable::Unreadable), "macOS could not read this raw, so this picture uses Camera Matching.");
    assert_eq!(note_text(MacosUnavailable::Transfer), "AI Denoise uses Camera Matching colours on this photo.");
}

/// T27 (UI): the Profile menu's Apply macOS Colour to All Photos from This Camera… asks with the count first, and
/// Apply moves them in one undo step. Off the Mac the item isn't there.
#[test]
fn all_photos_from_this_camera_asks_first_then_moves_them() {
    let (mut h, _, _dir) = panel(3, None);
    click(&mut h, "dropdown:profile");
    if !mac(&h) {
        assert!(!widget(&h, "profileMenu:macosAllFromCamera"));
        return;
    }
    // (the test's files don't exist, so macOS can't read them: nothing can move, and the item says so instead of asking;
    // which photos move is the engine's plan, tested there)
    click(&mut h, "profileMenu:macosAllFromCamera");
    assert!(h.app.ui.dialog.is_none(), "nothing to ask about");
    h.app.ui.dialog = Some(crate::state::Dialog::MacosForCamera { photo: 1, camera: "SONY ILCE-7M5".into(), count: 2, skipped: 1 });
    h.settle(SETTLE);
    assert!(widget(&h, "label:macosForCameraSkipped"), "the question counts the photos that stay");
    click(&mut h, "button:dialogOk");
    assert!(h.app.ui.dialog.is_none(), "Apply runs profiles.macosForCamera and closes");
}

/// T36: Settings ▸ Import has Starting Colour for Raw Photos with three Title Case choices; a click sets the library's
/// setting through `library.preferences`; off the Mac the hint says new raws start on Camera Matching.
#[test]
fn starting_colour_is_a_setting_in_the_import_tab() {
    assert_eq!(
        crate::dialogs::settings::starting_colour::CHOICES.map(|(_, l)| l),
        ["macOS Colour", "Camera Matching", "No Camera Matching"],
        "Title Case labels"
    );
    let (mut h, _, _dir) = panel(1, None);
    run(&mut h, "app.settings", json!({"tab": "import"}));
    for i in 0..3 {
        assert!(widget(&h, &format!("button:settingsStartingColour-{i}")), "choice {i}");
    }
    assert_eq!(h.app.session.import_defaults.starting_colour, StartingColour::Macos, "macOS Colour by default");
    click(&mut h, "button:settingsStartingColour-1");
    assert_eq!(h.app.session.import_defaults.starting_colour, StartingColour::CameraMatching);
    click(&mut h, "button:settingsStartingColour-0");
    assert_eq!(h.app.session.import_defaults.starting_colour, StartingColour::Macos);
    let says = |h: &Headless| h.app.widgets.iter().any(|(w, _)| w == "label:settingsStartingColourNotMac");
    assert_eq!(says(&h), !lightcraft_engine::files::macos_available(), "the hint follows the computer");
    crate::dialogs::settings::starting_colour::NOT_A_MAC_FOR_TESTS.with(|n| n.set(Some(true)));
    h.settle(SETTLE);
    let r = h.request("ui.screenshot", json!({"headless": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(says(&h), "off the Mac the hint says so");
    crate::dialogs::settings::starting_colour::NOT_A_MAC_FOR_TESTS.with(|n| n.set(None));
}

/// T38 (summary): a preset applied to several photos says how many can't use macOS Colour here; nothing when none.
#[test]
fn a_preset_says_how_many_photos_cant_use_macos_colour_here() {
    use crate::macos_colour::picker::fallback_line;
    assert_eq!(fallback_line(&json!({"macosFallback": 3})).as_deref(), Some("3 photos can't use macOS Colour here and use Camera Matching."));
    assert_eq!(fallback_line(&json!({"macosFallback": 1})).as_deref(), Some("1 photo can't use macOS Colour here and uses Camera Matching."));
    assert_eq!(fallback_line(&json!({"macosFallback": 0})), None);
    assert_eq!(fallback_line(&serde_json::Value::Null), None);
}

/// §6.8 rule 1: the Create Preset dialog includes the Profile group by default, so a preset made on macOS Colour
/// carries it (and says so, `the_create_preset_dialog_says_when_it_includes_the_colour_source`).
#[test]
fn create_preset_includes_the_profile_group_by_default() {
    let dialog = crate::state::Dialog::create_preset();
    assert!(matches!(&dialog, crate::state::Dialog::CreatePreset { groups, .. } if groups.iter().any(|g| g == "profile")), "{dialog:?}");
}
