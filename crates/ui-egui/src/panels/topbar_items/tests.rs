use std::time::Duration;

use serde_json::json;

use super::*;
use crate::headless::Headless;
use crate::state::AppSettings;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() })
}

fn demo() -> Headless {
    let mut h = Headless::new(app(), [1400.0, 900.0], 1.0);
    h.settle(Duration::from_secs(120));
    h
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

fn widget(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
    h.step();
}

fn run(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    h.step();
    h.step();
    r
}

/// The widget id of the bar's icon for each item (the sidebar, Back and Filters are named so too).
fn icon(item: &str) -> String {
    format!("icon:{item}")
}

// Scenario: the top bar has no Discord button any more
#[test]
fn the_bar_has_no_discord_button() {
    let h = demo();
    assert!(!has(&h, "icon:discord"));
    // every item is there by default
    for item in ITEMS {
        assert!(has(&h, &icon(item.id)), "{} is on the bar by default", item.id);
    }
    // the Help menu still has it
    let commands = crate::panels::photo_menu::menu_commands(&h.app);
    assert!(commands.iter().any(|(n, path)| matches!(n, MenuNode::Item { id, .. } if id == "app.discord") && path == "Help"));
}

// Scenario: hiding an item takes it off the bar, keeps the rest, and the choice is saved
#[test]
fn hiding_an_item_removes_it_from_the_bar_and_persists() {
    let mut h = demo();
    let (share, appearance) = (widget(&h, "icon:share"), widget(&h, "icon:appearance"));
    let r = run(&mut h, "topBar.setItem", json!({"item": "bell", "shown": false}));
    assert_eq!(r["ok"], true, "{r}");
    assert!(!has(&h, "icon:bell"), "hidden");
    for item in ITEMS.iter().filter(|i| i.id != "bell") {
        assert!(has(&h, &icon(item.id)), "{} stays", item.id);
    }
    assert_eq!(widget(&h, "icon:share"), share, "the icons on its right stay");
    assert!(widget(&h, "icon:appearance").center().x > appearance.center().x, "the icons on its left move over to close the gap");
    // saved with the settings, and read back
    let saved = serde_json::to_value(&h.app.ui.settings).unwrap();
    assert_eq!(saved["topBarHidden"], json!(["bell"]));
    let back: AppSettings = serde_json::from_value(saved).unwrap();
    assert_eq!(back.top_bar_hidden, ["bell"]);
    // an app started with that setting draws the bar without it
    let mut again = app();
    again.ui.settings = back;
    let mut h2 = Headless::new(again, [1400.0, 900.0], 1.0);
    h2.settle(Duration::from_secs(120));
    assert!(!has(&h2, "icon:bell") && has(&h2, "icon:import"));
    // showing it again
    run(&mut h, "topBar.setItem", json!({"item": "bell", "shown": true}));
    assert!(has(&h, "icon:bell"));
    assert!(h.app.ui.settings.top_bar_hidden.is_empty());
}

// Scenario: every item of the bar can be hidden, left side and right side
#[test]
fn every_item_can_be_hidden() {
    let mut h = demo();
    let all: Vec<&str> = ITEMS.iter().map(|i| i.id).collect();
    let r = run(&mut h, "topBar.set", json!({"hidden": all}));
    assert_eq!(r["ok"], true, "{r}");
    for item in ITEMS {
        assert!(!has(&h, &icon(item.id)), "{} is hidden", item.id);
    }
    // what is not an item stays: the search field and the title bar
    assert!(has(&h, "field:search"));
    // the search field gets the room
    let wide = widget(&h, "field:search").width();
    run(&mut h, "topBar.reset", json!({}));
    assert!(wide >= widget(&h, "field:search").width());
}

// Scenario: in a narrow window, hiding Filters gives its room to the search field (it left a gap)
#[test]
fn hiding_filters_widens_the_search_field_in_a_narrow_window() {
    let mut h = Headless::new(app(), [900.0, 700.0], 1.0);
    // the macOS layout: menus in the system menu bar, not left of the field
    h.app.native_menu = true;
    h.settle(Duration::from_secs(120));
    let with_filters = widget(&h, "field:search").width();
    run(&mut h, "topBar.setItem", json!({"item": "filter", "shown": false}));
    let without = widget(&h, "field:search").width();
    assert!(without > with_filters + 20.0, "{with_filters} -> {without}");
}

// Scenario: Reset to Default brings every item back
#[test]
fn reset_shows_every_item_again() {
    let mut h = demo();
    run(&mut h, "topBar.set", json!({"hidden": ["sidebar", "help", "filter"]}));
    assert!(!has(&h, "icon:sidebar") && !has(&h, "icon:help") && !has(&h, "icon:filter"));
    let r = run(&mut h, "topBar.reset", json!({}));
    assert_eq!(r["ok"], true, "{r}");
    for item in ITEMS {
        assert!(has(&h, &icon(item.id)), "{} is back", item.id);
    }
    assert_eq!(r["result"]["custom"], false);
    let saved = serde_json::to_value(&h.app.ui.settings).unwrap();
    assert!(saved.get("topBarHidden").is_none(), "the default writes nothing");
}

// Scenario: a hidden item's feature stays reachable, by its command and in the menu bar
#[test]
fn hidden_items_stay_reachable() {
    let mut app = app();
    let all: Vec<String> = ITEMS.iter().map(|i| i.id.to_string()).collect();
    set_hidden(&mut app, &all);
    let commands = crate::panels::photo_menu::menu_commands(&app);
    for item in ITEMS.iter().filter(|i| !i.command.is_empty()) {
        let in_menu = commands.iter().any(|(n, _)| matches!(n, MenuNode::Item { id, params, .. } if id == item.command && params.is_null()));
        assert!(in_menu || !item.elsewhere.is_empty(), "{} has neither a menu row nor a note on where it is", item.id);
        assert_ne!(reach(&commands, item), "Command only", "{}", item.id);
    }
    // and the command runs with its button hidden
    app.run("view.leftPanel", json!({})).unwrap();
    app.run("view.filterBar", json!({})).unwrap();
    assert!(app.ui.filter_bar);
    app.run("view.appearance", json!({})).unwrap();
}

#[test]
fn commands_refuse_what_they_do_not_know() {
    let mut app = app();
    assert!(app.run("topBar.set", json!({"hidden": ["bell", "discord"]})).is_err());
    assert!(app.run("topBar.set", json!({"hidden": "bell"})).is_err());
    assert!(app.run("topBar.setItem", json!({"item": "discord", "shown": false})).is_err());
    assert!(app.run("topBar.setItem", json!({"item": "bell"})).is_err());
    assert!(app.ui.settings.top_bar_hidden.is_empty(), "nothing changed");
    // get lists every item with where it is in the menu bar
    let r = app.run("topBar.get", json!({})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), ITEMS.len());
    let filter = r["items"].as_array().unwrap().iter().find(|i| i["id"] == "filter").unwrap();
    assert_eq!(filter["reach"], "View › Filter Bar");
    assert_eq!(filter["shown"], true);
}

#[test]
fn a_damaged_saved_value_reads_as_nothing_hidden() {
    let default = serde_json::to_value(AppSettings::default()).unwrap();
    let mut broken = default;
    broken["topBarHidden"] = json!(42);
    let back: AppSettings = serde_json::from_value(broken.clone()).unwrap();
    assert!(back.top_bar_hidden.is_empty());
    // one stray entry costs only itself; an id from a newer version is ignored, then dropped
    broken["topBarHidden"] = json!(["bell", 5, null, "teleporter"]);
    let back: AppSettings = serde_json::from_value(broken).unwrap();
    assert_eq!(back.top_bar_hidden, ["bell", "teleporter"]);
    let mut app = app();
    app.ui.settings = back;
    assert!(!shown(&app, "bell") && shown(&app, "help"));
    set_shown(&mut app, "help", false);
    assert_eq!(app.ui.settings.top_bar_hidden, ["bell", "help"], "in the bar's order, the unknown id dropped");
}

// Scenario: Settings ▸ Top Bar hides and shows items and resets
#[test]
fn the_settings_tab_toggles_items_and_resets() {
    let mut h = demo();
    let r = run(&mut h, "app.settings", json!({"tab": SETTINGS_TAB}));
    assert_eq!(r["ok"], true, "{r}");
    for item in ITEMS {
        assert!(has(&h, &format!("topBarRow:{}", item.id)), "a row for {}", item.id);
    }
    click(&mut h, "check:topBar-help");
    assert_eq!(h.app.ui.settings.top_bar_hidden, ["help"]);
    assert!(!has(&h, "icon:help"), "the bar follows at once");
    click(&mut h, "topBarRow:sidebar");
    assert_eq!(h.app.ui.settings.top_bar_hidden, ["sidebar", "help"]);
    click(&mut h, "button:topBarReset");
    assert!(h.app.ui.settings.top_bar_hidden.is_empty());
    assert!(has(&h, "icon:help") && has(&h, "icon:sidebar"));
}

// Scenario: a right-click on the bar's empty space lists the items and opens the editor
#[test]
fn the_bar_menu_toggles_items_and_opens_the_editor() {
    let mut h = demo();
    // empty bar space: between the filter icon and the icons on the right
    let (filter, appearance) = (widget(&h, "icon:filter"), widget(&h, "icon:appearance"));
    assert!(appearance.left() - filter.right() > 20.0, "there is room between them");
    let p = egui::pos2((filter.right() + appearance.left()) / 2.0, filter.center().y);
    let right_click = |h: &mut Headless| {
        let r = h.request("ui.click", json!({"x": p.x, "y": p.y, "button": "right"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
    };
    right_click(&mut h);
    for item in ITEMS {
        assert!(has(&h, &format!("menu:topBarItem:{}", item.id)), "{} in the menu", item.id);
    }
    click(&mut h, "menu:topBarItem:share");
    assert_eq!(h.app.ui.settings.top_bar_hidden, ["share"]);
    assert!(!has(&h, "icon:share"));
    assert!(has(&h, "menu:customizeTopBar"), "the menu stays open for the next one");
    click(&mut h, "menu:customizeTopBar");
    assert_eq!(h.app.ui.dialog, Some(crate::state::Dialog::Settings { tab: SETTINGS_TAB.into() }));
}
