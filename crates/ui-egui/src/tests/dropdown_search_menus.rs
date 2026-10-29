//! Type to search in the menus that borderless dropdown buttons open (Profile, White Balance, Point
//! Curve, Aspect Ratio, Tool Overlay, Mask Overlay, Filter Presets), and the keyboard they own
//! while open: a typed letter runs no shortcut, in the searchable menus and in the ones that are not
//! (Zoom).

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::ViewMode;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn edit_view() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let mut app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    app.ui.settings.gpu = false;
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail", "right": "edit"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn call(h: &mut Headless, method: &str, params: serde_json::Value) {
    let r = h.request(method, params, T);
    assert_eq!(r["ok"], true, "{method}: {r}");
    h.step();
    h.step();
}

fn open(h: &mut Headless, id: &str) {
    call(h, "ui.clickWidget", json!({"id": id}));
    h.step();
}

fn type_text(h: &mut Headless, text: &str) {
    call(h, "ui.text", json!({"text": text}));
    h.step();
}

fn press(h: &mut Headless, key: &str) {
    call(h, "ui.key", json!({"key": key}));
    h.step();
}

fn shows(h: &Headless, text: &str) -> bool {
    h.painted_text().iter().any(|t| t == text)
}

fn section(h: &mut Headless, set: serde_json::Value) {
    let r = h.request("ui.set", set, T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
}

fn active_develop(h: &Headless) -> lightcraft_develop::DevelopSettings {
    let id = h.app.session.active().expect("an active photo");
    h.app.session.develop_of(id).unwrap_or_default().as_ref().clone()
}

/// Profile: a menu with groups, hover preview and its own scrolling levels.
#[test]
fn the_profile_menu_filters_and_enter_picks() {
    let mut h = edit_view();
    open(&mut h, "dropdown:profile");
    assert!(shows(&h, "Basic") && shows(&h, "Browse…"), "the groups before anything is typed");
    assert!(!shows(&h, "Search"), "no field before typing");
    type_text(&mut h, "viv");
    assert!(shows(&h, "Vivid") && !shows(&h, "Basic") && !shows(&h, "Browse…"), "{:?}", h.painted_text());
    assert!(!shows(&h, "Neutral"));
    press(&mut h, "enter");
    h.settle(SETTLE);
    assert_eq!(active_develop(&h).profile.id, "lc.vivid", "↩ picks the first match");
    assert!(!shows(&h, "Vivid") || !crate::widgets::dropdown_search::list_open(&h.view.ctx), "↩ closes the menu");
}

#[test]
fn a_click_on_the_search_field_keeps_the_menu_open_and_esc_clears_then_closes() {
    let mut h = edit_view();
    open(&mut h, "dropdown:profile");
    type_text(&mut h, "mono");
    assert!(shows(&h, "Monochrome") && !shows(&h, "Vivid"));
    call(&mut h, "ui.clickWidget", json!({"id": "dropdownSearch"}));
    assert!(shows(&h, "Monochrome"), "a click in the field leaves the menu open");
    press(&mut h, "escape");
    assert!(shows(&h, "Browse…") && !shows(&h, "mono"), "the first Esc clears the text and keeps the menu: {:?}", h.painted_text());
    press(&mut h, "escape");
    h.step();
    assert!(!shows(&h, "Browse…"), "the second closes it");
}

#[test]
fn no_match_in_the_profile_menu_says_so() {
    let mut h = edit_view();
    open(&mut h, "dropdown:profile");
    type_text(&mut h, "zzzz");
    assert!(shows(&h, "No match"));
    press(&mut h, "enter");
    assert_eq!(active_develop(&h).profile.id, "lc.color", "↩ with no match picks nothing");
}

/// White Balance: the generic menu of a plain list of options.
#[test]
fn white_balance_filters_and_picks() {
    use lightcraft_develop::WbMode;
    let mut h = edit_view();
    section(&mut h, json!({"openSections": ["color"]}));
    open(&mut h, "dropdown:wbMode");
    assert!(shows(&h, "Daylight") && shows(&h, "Cloudy"));
    type_text(&mut h, "dayl");
    assert!(shows(&h, "Daylight") && !shows(&h, "Cloudy"));
    press(&mut h, "enter");
    h.settle(SETTLE);
    assert_eq!(active_develop(&h).wb.mode, WbMode::Daylight);
}

/// Aspect Ratio: rows plus a custom-ratio form that waits while text is typed.
#[test]
fn aspect_ratio_filters_and_hides_the_custom_form_while_typing() {
    let mut h = edit_view();
    section(&mut h, json!({"right": "crop"}));
    open(&mut h, "dropdown:cropAspect");
    assert!(shows(&h, "Free") && shows(&h, "Original") && shows(&h, "Custom"), "{:?}", h.painted_text());
    type_text(&mut h, "orig");
    assert!(shows(&h, "Original") && !shows(&h, "Free") && !shows(&h, "Custom"), "the custom form waits: {:?}", h.painted_text());
    press(&mut h, "escape");
    assert!(shows(&h, "Free") && shows(&h, "Custom"));
}

/// Tool Overlay: a plain list whose rows are registered for the control channel.
#[test]
fn tool_overlay_filters_and_picks() {
    use crate::state::ToolOverlay;
    let mut h = edit_view();
    section(&mut h, json!({"right": "remove"}));
    open(&mut h, "dropdown:toolOverlay");
    assert!(h.app.widgets.iter().any(|(id, _)| id == "toolOverlay:never"), "the rows stay registered");
    type_text(&mut h, "nev");
    assert!(shows(&h, "Never") && !shows(&h, "Always"));
    press(&mut h, "enter");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.tool_overlay, ToolOverlay::Never);
}

/// Filter Presets and the metadata pickers of the filter bar.
#[test]
fn the_filter_bar_menus_search() {
    let mut h = edit_view();
    call(&mut h, "ui.key", json!({"key": "f", "shift": true}));
    open(&mut h, "dropdown:filterPresets");
    assert!(shows(&h, "No filter presets yet") && shows(&h, "Save Current Filter as Preset…"));
    type_text(&mut h, "x");
    assert!(shows(&h, "No match") && !shows(&h, "Save Current Filter as Preset…"), "{:?}", h.painted_text());
    press(&mut h, "escape");
    press(&mut h, "escape");
    open(&mut h, "dropdown:filter-kind");
    assert!(shows(&h, "Raw"), "{:?}", h.painted_text());
    type_text(&mut h, "vid");
    assert!(!shows(&h, "Raw"), "typing narrows the kind picker: {:?}", h.painted_text());
}

/// The Zoom menu in the bottom bar has no search, but a letter typed while it is open is not a shortcut.
#[test]
fn a_letter_typed_over_an_open_menu_runs_no_shortcut() {
    let mut h = edit_view();
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    open(&mut h, "dropdown:zoom");
    assert!(crate::widgets::dropdown_search::list_open(&h.view.ctx), "the open Zoom menu owns the keyboard");
    press(&mut h, "g");
    assert_eq!(h.app.ui.view, ViewMode::Detail, "G (Grid) did not run over the open Zoom menu");
    press(&mut h, "escape");
    h.step();
    h.step();
    h.step();
    h.step();
    assert!(!crate::widgets::dropdown_search::list_open(&h.view.ctx));
    press(&mut h, "g");
    assert_ne!(h.app.ui.view, ViewMode::Detail, "G works once the menu is closed");
}

/// A letter typed in a searchable menu searches; it is no shortcut either.
#[test]
fn a_letter_typed_in_a_searchable_menu_runs_no_shortcut() {
    let mut h = edit_view();
    open(&mut h, "dropdown:profile");
    press(&mut h, "g");
    type_text(&mut h, "g");
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert!(shows(&h, "g"), "the typed letter is in the search field: {:?}", h.painted_text());
}

/// Point Curve presets: rows, a separator and actions that wait while text is typed.
#[test]
fn point_curve_presets_filter_and_enter_applies() {
    let mut h = edit_view();
    section(&mut h, json!({"openSections": ["light"]}));
    call(&mut h, "ui.clickWidget", json!({"id": "flyout:curve"}));
    h.settle(SETTLE);
    // the preset dropdown sits under the graph: scroll the panel to it
    call(&mut h, "ui.move", json!({"x": 1250.0, "y": 450.0}));
    for _ in 0..6 {
        if h.app.widgets.iter().any(|(id, rect)| id == "dropdown:curvePreset" && rect.bottom() < 700.0) {
            break;
        }
        call(&mut h, "ui.scroll", json!({"dy": -150.0}));
        h.settle(SETTLE);
    }
    open(&mut h, "dropdown:curvePreset");
    assert!(shows(&h, "Save Point Curve…") && shows(&h, "Strong Contrast"), "{:?}", h.painted_text());
    type_text(&mut h, "strong");
    assert!(shows(&h, "Strong Contrast") && !shows(&h, "Medium Contrast") && !shows(&h, "Save Point Curve…"), "{:?}", h.painted_text());
    press(&mut h, "enter");
    h.settle(SETTLE);
    let d = active_develop(&h);
    assert_eq!(lightcraft_engine::cmd::curves::matching_preset(&h.app.session, &d.curve).as_deref(), Some("Strong Contrast"));
}

/// Mask Overlay mode: a plain list in the Masking panel.
#[test]
fn mask_overlay_mode_filters_and_picks() {
    let mut h = edit_view();
    let r = h.request("engine.execute", json!({"command": "panel.masking"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": "mask.add", "params": {"kind": "sky"}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    open(&mut h, "dropdown:maskOverlayMode");
    type_text(&mut h, "white on");
    press(&mut h, "enter");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.mask_overlay_mode, "whiteOnBlack", "{:?}", h.app.ui.mask_overlay_mode);
}
