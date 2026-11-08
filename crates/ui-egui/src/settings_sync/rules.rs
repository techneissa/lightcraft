//! What the settings file holds of `ui.json`: everything except the keys of one Mac, from a deny
//! list. A new setting is synced without a change here; `every_ui_key_is_decided` fails until the
//! key is named below, either as synced ([`SYNCED`], [`SYNCED_SETTINGS`]) or as left out
//! ([`LEFT_OUT`], [`LEFT_OUT_SETTINGS`]) with the reason, so nobody forgets to decide.

use serde_json::{Map, Value, json};

use super::{KIND, VERSION};
use crate::state::UiState;

/// Top-level keys of the UI state that stay on one Mac, with the reason.
pub const LEFT_OUT: &[(&str, &str)] = &[
    ("window", "the window's size and place"),
    ("seenVersion", "the version whose What's New this Mac has shown: each Mac shows it once after its own update"),
    ("docking", "panel positions"),
    ("dockingVersion", "panel positions"),
    ("view", "the view in use"),
    ("leftPanel", "which panels are open now"),
    ("right", "which panels are open now"),
    ("presets", "which panels are open now"),
    ("filmstrip", "which panels are open now"),
    ("openSections", "open and closed Edit sections: what is open belongs to one screen"),
    ("navigator", "a panel shown or hidden"),
    ("filterBar", "a panel shown or hidden"),
    ("histogram", "a panel shown or hidden"),
    ("leftWidth", "side bar widths"),
    ("rightWidth", "side bar widths"),
    ("zoom", "zoom and pan"),
    ("pan", "zoom and pan"),
    ("beforeAfter", "the view in use"),
    ("thumbSize", "sizes of one screen"),
    ("importThumbSize", "sizes of one screen"),
    ("openFlyouts", "what is open now"),
    ("showClipping", "an old key, read only"),
    ("softProof", "soft proofing of the open photo"),
    ("proof", "printer and paper profiles of this Mac"),
    ("proofPaperInk", "printer and paper profiles of this Mac"),
    ("search", "the text in the search field"),
    ("curveChannel", "the tab in use"),
    ("mixerMode", "the tab in use"),
    ("gradingMode", "the tab in use"),
    ("tool", "the tool in use"),
    ("maskOp", "the next mask part's mode"),
    ("pointColor", "the sample in use"),
    ("eye", "the correction in use"),
    ("eyePet", "the correction in use"),
    ("hiddenLocations", "local folders of this Mac"),
    ("localRoots", "local folders of this Mac"),
    ("keywordListOpen", "the open levels of one library's keywords"),
    ("fullscreen", "this window"),
    ("secondWindow", "the second window's place"),
    ("layoutImage", "a file on this Mac"),
    ("renamingComponent", "a name being typed"),
    ("sync", "this Mac's sync folder and state"),
];

/// Top-level keys of the UI state that go into the file (`settings` is [`SYNCED_SETTINGS`] inside).
pub const SYNCED: &[&str] = &[
    "language",
    "presetThumbs",
    "clickZoom",
    "singlePanel",
    "clipShadows",
    "clipHighlights",
    "maskOverlay",
    "maskOverlayMode",
    "maskOverlayColor",
    "maskOverlayOpacity",
    "maskPins",
    "toolOverlay",
    "maskOverlayAll",
    "cropOverlayOrient",
    "cropOverlay",
    "cropDefaultAspect",
    "showFilenames",
    "gridInfo",
    "showCounts",
    "faceBoxes",
    "mapLockMarkers",
    "mapShowPins",
    "mapFilterBar",
    "sidebar",
    "editSections",
    "toolStrip",
    "brushSize",
    "brushFeather",
    "brushFlow",
    "brushErase",
    "brushAutoMask",
    "brushPressure",
    "removeSize",
    "removeFeather",
    "removeOpacity",
    "removeFill",
    "pointColorVisualize",
    "hdrVisualize",
    "visualizeSpots",
    "spotsThreshold",
    "autoAdvance",
    "keywordingView",
    "foldedSections",
    "infoOverlay",
    "settings",
];

/// Keys of `settings` (the Settings window's app preferences) that stay on one Mac, with the reason.
pub const LEFT_OUT_SETTINGS: &[(&str, &str)] = &[
    ("libraryPath", "library paths"),
    ("recentLibraries", "library paths"),
    ("recentLibrariesOpened", "library paths"),
    ("externalEditor", "an application of this Mac"),
    ("photocraftPath", "an application of this Mac"),
    ("gpu", "this Mac's graphics"),
    ("composeWindow", "this Mac's window system"),
    ("previewLimit", "this Mac's graphics and memory"),
    ("memoryMb", "this Mac's memory"),
    ("backgroundPrep", "what this Mac prepares ahead: its speed and memory"),
    ("lensDatabase", "a download answered on this Mac"),
    ("displayProfile", "this Mac's monitor"),
    ("smartEditingOffered", "a question asked once on this Mac"),
    ("shortcutsWindowSize", "a window's size"),
];

/// Keys of `settings` that go into the file.
pub const SYNCED_SETTINGS: &[&str] = &[
    "atLaunch",
    "startupView",
    "askBeforeDelete",
    "confirmDeletePickedOnly",
    "askBeforeMove",
    "deleteFiles",
    "closeDialogsOutside",
    "importDroppedDirectly",
    "externalFile",
    "photocraftFile",
    "smartPreviewsForEditing",
    "mapStartOnOsm",
    "infoSelectedPhotos",
    "filmNames",
    "filmBadges",
    "gridBadges",
    "dimRejected",
    "loupeMessages",
    "keymap",
    "appearanceMode",
    "darkTheme",
    "lightTheme",
    "favoriteThemes",
    "keywordChips",
    "focusPoint",
    "zoomToFocus",
    "loupeInfo",
    "lightroomImport",
    "photoMenu",
    "photoMenuPresets",
    "topBarHidden",
    // (how many hot pixels Remove Hot Pixels may replace: the same raw renders alike on every Mac)
    "hotPixelLimit",
];

/// Why `key` of the UI state stays on one Mac; `None` when it goes into the file.
pub fn left_out(key: &str) -> Option<&'static str> {
    LEFT_OUT.iter().find(|(k, _)| *k == key).map(|(_, why)| *why)
}

/// Why `key` of `settings` stays on one Mac; `None` when it goes into the file.
pub fn left_out_setting(key: &str) -> Option<&'static str> {
    LEFT_OUT_SETTINGS.iter().find(|(k, _)| *k == key).map(|(_, why)| *why)
}

/// The UI state as the file's `ui` part: its saved form without the keys of one Mac.
pub fn ui_part(ui: &UiState) -> Value {
    let mut all = serde_json::to_value(ui).unwrap_or(Value::Null);
    if let Some(m) = all.as_object_mut() {
        m.retain(|k, _| left_out(k).is_none());
        if let Some(settings) = m.get_mut("settings").and_then(Value::as_object_mut) {
            settings.retain(|k, _| left_out_setting(k).is_none());
            // these three are left out of `ui.json` while they hold nothing; a file says so, or a Mac that
            // has an arrangement would keep it when the other Mac went back to the default
            settings.entry("photoMenu").or_insert(Value::Null);
            settings.entry("topBarHidden").or_insert_with(|| json!([]));
            settings
                .entry("photoMenuPresets")
                .or_insert_with(|| serde_json::to_value(crate::panels::photo_menu::presets::Presets::default()).unwrap_or(Value::Null));
        }
    }
    all
}

/// The user presets as a `.lcpreset` file's JSON; `None` when there are none.
pub fn presets_part(presets: &[lightcraft_develop::Preset]) -> Option<Value> {
    let user: Vec<_> = presets.iter().filter(|p| !p.builtin).cloned().collect();
    if user.is_empty() {
        return None;
    }
    serde_json::from_str(&lightcraft_engine::presets::to_lcpreset(&user)).ok()
}

/// The settings file's JSON (without `savedBy`): `presets` are included when given.
pub fn document(ui: &UiState, presets: Option<Value>) -> Value {
    let mut doc = Map::new();
    doc.insert("kind".into(), json!(KIND));
    doc.insert("version".into(), json!(VERSION));
    doc.insert("app".into(), json!("LightCraft"));
    doc.insert("ui".into(), ui_part(ui));
    if let Some(p) = presets {
        doc.insert("presets".into(), p);
    }
    Value::Object(doc)
}

/// The file's text for a save or export.
pub fn text(doc: &Value) -> String {
    let mut text = serde_json::to_string_pretty(doc).unwrap_or_default();
    text.push('\n');
    text
}
