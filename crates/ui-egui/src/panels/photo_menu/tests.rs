use serde_json::json;

use super::editor::{View, insert, move_row, remove};
use super::*;
use crate::headless::HeadlessView;
use crate::{LightcraftApp, Services};

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default())
}

/// The demo's first `n` photos, selected; the first is the one right-clicked.
fn select(app: &mut LightcraftApp, n: usize) -> PhotoId {
    let ids: Vec<u64> = app.session.visible_cloned().iter().take(n).map(|p| p.0).collect();
    assert_eq!(ids.len(), n, "the demo has {n} photos");
    app.run("library.select", json!({"ids": ids})).unwrap();
    PhotoId(ids[0])
}

/// The text one frame of `draw` paints, in order.
fn painted(app: &mut LightcraftApp, mut draw: impl FnMut(&mut LightcraftApp, &mut egui::Ui)) -> Vec<String> {
    let mut view = HeadlessView::new();
    let raw = HeadlessView::raw_input(egui::vec2(600.0, 1400.0), 1.0, 0.0, vec![]);
    view.run(raw, |ui| draw(app, ui));
    view.painted_text()
}

fn keys(slots: &[Slot]) -> Vec<String> {
    slots.iter().map(Slot::key).collect()
}

#[test]
fn every_default_slot_is_known() {
    assert_eq!(default_slots().len(), DEFAULT.len());
    assert_eq!(keys(&default_slots()), DEFAULT);
    // each built-in item once, and every built-in item is in the default menu
    for b in items::BUILTINS {
        assert_eq!(DEFAULT.iter().filter(|s| **s == b.id).count(), 1, "{}", b.id);
    }
}

#[test]
fn slots_round_trip_and_unknown_ones_are_skipped() {
    for s in ["rename", "---", "cmd:view.compare", r#"cmd:photo.rate|{"rating":3}"#] {
        assert_eq!(Slot::parse(s).map(|s| s.key()).as_deref(), Some(s));
    }
    assert_eq!(Slot::parse("cmd:photo.rate|{\"rating\":3}"), Some(Slot::Command { id: "photo.rate".into(), params: json!({"rating": 3}) }));
    for bad in ["", "noSuchItem", "cmd:", "cmd:x|{not json", "cmd:|{}"] {
        assert_eq!(Slot::parse(bad), None, "{bad:?}");
    }
    let mut app = app();
    app.ui.settings.photo_menu = Some(vec!["rename".into(), "fromAFutureVersion".into(), "---".into(), "delete".into()]);
    assert_eq!(keys(&slots(&app)), ["rename", "---", "delete"]);
}

#[test]
fn separators_are_tidied() {
    let s = |k: &str| Slot::parse(k).unwrap();
    let menu = vec![s("---"), s("rename"), s("---"), s("---"), s("delete"), s("---")];
    assert_eq!(keys(&tidy(menu)), ["rename", "---", "delete"]);
    assert!(tidy(vec![s("---"), s("---")]).is_empty());
}

#[test]
fn the_default_arrangement_is_saved_as_none() {
    let mut app = app();
    save(&mut app, &default_slots());
    assert_eq!(app.ui.settings.photo_menu, None);
    let mut menu = default_slots();
    menu.swap(0, 2);
    save(&mut app, &menu);
    assert_eq!(app.ui.settings.photo_menu.as_deref().map(<[String]>::len), Some(DEFAULT.len()));
    assert_eq!(keys(&slots(&app)), keys(&menu));
}

#[test]
fn rows_move_insert_and_are_removed() {
    let s = |k: &str| Slot::parse(k).unwrap();
    let mut menu = vec![s("rename"), s("reload"), s("delete")];
    let mut view = View::default();
    move_row(&mut menu, &mut view, 0, 3);
    assert_eq!(keys(&menu), ["reload", "delete", "rename"]);
    assert_eq!(view.picked, Some(2));
    move_row(&mut menu, &mut view, 2, 0);
    assert_eq!(keys(&menu), ["rename", "reload", "delete"]);
    // onto itself, or past the end: nothing moves
    move_row(&mut menu, &mut view, 1, 1);
    move_row(&mut menu, &mut view, 1, 2);
    move_row(&mut menu, &mut view, 5, 0);
    assert_eq!(keys(&menu), ["rename", "reload", "delete"]);
    view.picked = Some(0);
    insert(&mut menu, &mut view, Slot::Separator);
    assert_eq!(keys(&menu), ["rename", "---", "reload", "delete"]);
    assert_eq!(view.picked, Some(1));
    view.picked = None;
    insert(&mut menu, &mut view, s("rotateLeft"));
    assert_eq!(keys(&menu), ["rename", "---", "reload", "delete", "rotateLeft"]);
    remove(&mut menu, &mut view, 4);
    assert_eq!(view.picked, Some(3));
    remove(&mut menu, &mut view, 9);
    assert_eq!(keys(&menu), ["rename", "---", "reload", "delete"]);
}

/// The official menu ([`crate::panels::grid::context_menu`]) and this one in its default
/// arrangement show the same top-level rows in the same order, but for the two this one adds. A
/// row the official menu gains in an update fails this test: add it to `items::BUILTINS` and
/// `DEFAULT`. (Submenus are closed here: a change inside one is not seen.)
#[test]
fn the_default_menu_is_the_official_one_plus_before_after_and_customize() {
    let mut app = app();
    let id = select(&mut app, 3);
    let official = painted(&mut app, |app, ui| crate::panels::grid::context_menu(app, ui, id));
    let ours = painted(&mut app, |app, ui| show(app, ui, id));
    let added = [crate::i18n::tr("Before/After Split").to_string(), crate::i18n::tr("Customize Menu…").to_string()];
    let ours: Vec<String> = ours.into_iter().filter(|t| !added.contains(t)).collect();
    assert_eq!(ours, official);
    assert!(official.iter().any(|t| t == "Export Animated GIF…"), "{official:?}");
}

#[test]
fn one_photo_hides_what_needs_several() {
    let mut app = app();
    let id = select(&mut app, 1);
    let text = painted(&mut app, |app, ui| show(app, ui, id));
    for hidden in ["Export Animated GIF…", "Photo Merge"] {
        assert!(!text.iter().any(|t| t == hidden), "{hidden} shows with one photo: {text:?}");
    }
    for kept in ["Open in Detail", "Before/After Split", "Export…", "Delete Photo", "Customize Menu…"] {
        assert!(text.iter().any(|t| t == kept), "{kept} missing: {text:?}");
    }
    let id = select(&mut app, 2);
    let text = painted(&mut app, |app, ui| show(app, ui, id));
    assert!(text.iter().any(|t| t == "Export Animated GIF…") && text.iter().any(|t| t == "Photo Merge"), "{text:?}");
}

#[test]
fn a_menu_command_can_be_added_and_runs_as_from_the_menu_bar() {
    let mut app = app();
    let r = app.run("photoMenu.set", json!({"items": ["openInDetail", "---", "cmd:view.compare", "cmd:dialog.animatedGif"]})).unwrap();
    assert_eq!(r["custom"], true);
    assert_eq!(r["items"], json!(["openInDetail", "---", "cmd:view.compare", "cmd:dialog.animatedGif"]));
    assert!(r["available"].as_array().unwrap().iter().any(|a| a["item"] == "rename"));
    let id = select(&mut app, 1);
    let text = painted(&mut app, |app, ui| show(app, ui, id));
    let compare = crate::i18n::tr("Compare").to_string();
    // the GIF command needs several photos: hidden with one, and with it the separator stays
    assert_eq!(text, ["Open in Detail".to_string(), compare.clone()]);
    let id = select(&mut app, 2);
    let text = painted(&mut app, |app, ui| show(app, ui, id));
    assert_eq!(text, ["Open in Detail".to_string(), compare, "Export Animated GIF…".to_string()]);
}

#[test]
fn set_refuses_unknown_items_and_reset_restores_the_default() {
    let mut app = app();
    let e = app.run("photoMenu.set", json!({"items": ["rename", "bogus"]})).unwrap_err();
    assert!(e.contains("bogus"), "{e}");
    let e = app.run("photoMenu.set", json!({"items": ["rename", "cmd:no.suchCommand"]})).unwrap_err();
    assert!(e.contains("no.suchCommand"), "{e}");
    assert_eq!(app.ui.settings.photo_menu, None, "nothing changed");
    let e = app.run("photoMenu.set", json!({"items": ["rename", "---", "---", "rename"]})).unwrap_err();
    assert!(e.contains("rename (twice)"), "{e}");
    assert_eq!(app.ui.settings.photo_menu, None, "nothing changed");
    assert!(app.run("photoMenu.set", json!({})).is_err());
    app.run("photoMenu.set", json!({"items": ["rename"]})).unwrap();
    assert_eq!(app.ui.settings.photo_menu, Some(vec!["rename".to_string()]));
    let r = app.run("photoMenu.reset", json!({})).unwrap();
    assert_eq!(r["custom"], false);
    assert_eq!(app.ui.settings.photo_menu, None);
}

#[test]
fn group_into_stack_needs_two_photos() {
    let labels = |several, stacked| items::stack_rows(several, stacked).into_iter().map(|(l, _, on)| (l, on)).collect::<Vec<_>>();
    assert!(!labels(false, false).iter().any(|(l, _)| *l == "Group into Stack"));
    assert!(!labels(false, true).iter().any(|(l, _)| *l == "Group into Stack"));
    assert_eq!(labels(true, false).first(), Some(&("Group into Stack", true)));
    // the rest stay, enabled when the photo is in a stack
    assert_eq!(labels(false, true).len(), 7);
    assert!(labels(false, true).iter().all(|(_, on)| *on));
    assert!(labels(true, false).iter().skip(1).all(|(_, on)| !*on));
}

#[test]
fn an_emptied_menu_still_offers_customize() {
    let mut app = app();
    app.run("photoMenu.set", json!({"items": []})).unwrap();
    let id = select(&mut app, 1);
    assert_eq!(painted(&mut app, |app, ui| show(app, ui, id)), ["Customize Menu…"]);
}

#[test]
fn the_arrangement_round_trips_through_saved_settings() {
    let mut app = app();
    app.run("photoMenu.set", json!({"items": ["delete", "---", "cmd:view.compare"]})).unwrap();
    let saved = serde_json::to_value(&app.ui.settings).unwrap();
    assert_eq!(saved["photoMenu"], json!(["delete", "---", "cmd:view.compare"]));
    let back: crate::state::AppSettings = serde_json::from_value(saved).unwrap();
    assert_eq!(back.photo_menu, app.ui.settings.photo_menu);
    // the default writes nothing, and a broken value reads as the default
    let default = serde_json::to_value(crate::state::AppSettings::default()).unwrap();
    assert!(default.get("photoMenu").is_none());
    let mut broken = default;
    broken["photoMenu"] = json!(42);
    let back: crate::state::AppSettings = serde_json::from_value(broken.clone()).unwrap();
    assert_eq!(back.photo_menu, None);
    // one stray entry in a list costs only that entry
    broken["photoMenu"] = json!(["rename", 5, null, "---", "delete"]);
    let back: crate::state::AppSettings = serde_json::from_value(broken).unwrap();
    assert_eq!(back.photo_menu, Some(vec!["rename".to_string(), "---".to_string(), "delete".to_string()]));
}

mod headless {
    use std::time::Duration;

    use serde_json::json;

    use crate::headless::Headless;
    use crate::state::{BeforeAfter, ViewMode};
    use crate::{LightcraftApp, Services};

    const T: Duration = Duration::from_secs(20);

    fn demo(ui: serde_json::Value) -> Headless {
        let services = Services { png: None, ..Default::default() };
        let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
        let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
        let r = h.request("ui.set", ui, T);
        assert_eq!(r["ok"], true, "{r}");
        h.settle(Duration::from_secs(120));
        h
    }

    fn widget(h: &Headless, id: &str) -> egui::Rect {
        h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
    }

    fn has(h: &Headless, id: &str) -> bool {
        h.app.widgets.iter().any(|(w, _)| w == id)
    }

    fn click(h: &mut Headless, id: &str) {
        let r = h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        h.step();
        h.step();
    }

    fn right_click(h: &mut Headless, id: &str) {
        let c = widget(h, id).center();
        let r = h.request("ui.click", json!({"x": c.x, "y": c.y, "button": "right"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
    }

    fn menu(h: &Headless) -> Vec<String> {
        h.app.ui.settings.photo_menu.clone().unwrap_or_else(|| super::DEFAULT.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn before_after_split_from_the_grid_opens_the_photo_split() {
        let mut h = demo(json!({"view": "photoGrid"}));
        let photo = h.app.session.visible_cloned()[0];
        right_click(&mut h, &format!("thumb:{}", photo.0));
        click(&mut h, "button:contextBeforeAfterSplit");
        assert_eq!(h.app.ui.view, ViewMode::Detail);
        assert_eq!(h.app.ui.before_after, BeforeAfter::Split);
        assert_eq!(h.app.session.active(), Some(photo));
        // in Detail the same item turns it off again
        let r = h.request("ui.click", json!({"x": 700.0, "y": 420.0, "button": "right"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
        click(&mut h, "button:contextBeforeAfterSplit");
        assert_eq!(h.app.ui.before_after, BeforeAfter::Off);
    }

    #[test]
    fn the_settings_tab_removes_adds_moves_and_resets() {
        let mut h = demo(json!({"view": "photoGrid"}));
        let r = h.request("engine.execute", json!({"command": "app.settings", "params": {"tab": "menu"}}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
        // pick the first row, take it out
        click(&mut h, "photoMenuRow:0");
        click(&mut h, "button:photoMenuRemove:0");
        assert_eq!(menu(&h)[0], "beforeAfterSplit");
        assert!(has(&h, "button:photoMenuAdd:openInDetail"), "a removed item is offered again");
        // + puts it below the picked row (now the first)
        click(&mut h, "button:photoMenuAdd:openInDetail");
        assert_eq!(&menu(&h)[..2], ["beforeAfterSplit", "openInDetail"]);
        // drag the second row above the first
        let from = widget(&h, "photoMenuRow:1").center();
        let to = widget(&h, "photoMenuRow:0").top() + 3.0;
        let r = h.request("ui.drag", json!({"x": from.x, "y": from.y, "toX": from.x, "toY": to, "steps": 10}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
        assert_eq!(menu(&h), super::DEFAULT, "back where it was: the default again");
        assert_eq!(h.app.ui.settings.photo_menu, None);
        // a change, then Reset to Default
        click(&mut h, "photoMenuRow:2");
        click(&mut h, "button:photoMenuRemove:2");
        assert!(h.app.ui.settings.photo_menu.is_some());
        click(&mut h, "button:photoMenuReset");
        assert_eq!(h.app.ui.settings.photo_menu, None);
        // keys: ⌥↓ moves the picked row down, ↑ picks the one above, Delete takes it out
        click(&mut h, "photoMenuRow:0");
        let key = |h: &mut Headless, k: &str, alt: bool| {
            let r = h.request("ui.key", json!({"key": k, "alt": alt}), T);
            assert_eq!(r["ok"], true, "{r}");
            h.step();
            h.step();
        };
        key(&mut h, "ArrowDown", true);
        assert_eq!(&menu(&h)[..2], ["beforeAfterSplit", "openInDetail"]);
        key(&mut h, "ArrowUp", false);
        key(&mut h, "delete", false);
        assert_eq!(&menu(&h)[..2], ["openInDetail", "editIn"]);
    }

    #[test]
    fn a_menu_command_in_the_photo_menu_runs() {
        let mut h = demo(json!({"view": "photoGrid"}));
        let r = h.request("engine.execute", json!({"command": "photoMenu.set", "params": {"items": ["openInDetail", "cmd:view.compare"]}}), T);
        assert_eq!(r["ok"], true, "{r}");
        let photo = h.app.session.visible_cloned()[0];
        right_click(&mut h, &format!("thumb:{}", photo.0));
        click(&mut h, "button:contextCommand:view.compare");
        assert_eq!(h.app.ui.view, ViewMode::Compare);
    }

    #[test]
    fn a_right_click_on_a_menu_command_row_opens_the_assign_shortcut_box() {
        let mut h = demo(json!({"view": "photoGrid"}));
        let r = h.request("engine.execute", json!({"command": "photoMenu.set", "params": {"items": ["openInDetail", "cmd:view.compare"]}}), T);
        assert_eq!(r["ok"], true, "{r}");
        let photo = h.app.session.visible_cloned()[0];
        right_click(&mut h, &format!("thumb:{}", photo.0));
        right_click(&mut h, "button:contextCommand:view.compare");
        h.step();
        assert_eq!(h.app.keybinds.capture.as_ref().map(|c| c.id.as_str()), Some("view.compare"));
        assert_eq!(h.app.ui.view, ViewMode::PhotoGrid, "a right-click does not run the row");
    }

    #[test]
    fn customize_menu_opens_the_settings_tab() {
        let mut h = demo(json!({"view": "photoGrid"}));
        let photo = h.app.session.visible_cloned()[0];
        right_click(&mut h, &format!("thumb:{}", photo.0));
        click(&mut h, "button:contextCustomizeMenu");
        assert_eq!(h.app.ui.dialog, Some(crate::state::Dialog::Settings { tab: super::SETTINGS_TAB.into() }));
    }
}
