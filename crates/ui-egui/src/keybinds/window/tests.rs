use super::*;

fn clash_map(entries: &[(&str, &str)]) -> HashMap<(&'static str, usize), Vec<&'static str>> {
    let keymap: Keymap = entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    clashes::clashes(&keymap)
}

#[test]
fn no_clash_among_the_declared_shortcuts() {
    assert!(clash_map(&[]).is_empty(), "{:?}", clash_map(&[]));
}

#[test]
fn a_key_two_commands_share_is_a_clash_for_both() {
    // a hand-edited file: Survey's second shortcut is D, which is Detail's declared key
    let map = clash_map(&[("view.survey#2", "D")]);
    assert_eq!(map.get(&("view.survey", 1)), Some(&vec!["view.detail"]));
    assert_eq!(map.get(&("view.detail", 0)), Some(&vec!["view.survey"]));
    // two saved first shortcuts on one key clash as well
    let map = clash_map(&[("view.survey", "J"), ("view.detail", "J")]);
    assert_eq!(map.get(&("view.survey", 0)), Some(&vec!["view.detail"]));
}

#[test]
fn a_single_key_and_the_chord_it_leads_clash() {
    let map = clash_map(&[("view.survey#2", "D S")]);
    assert_eq!(map.get(&("view.survey", 1)), Some(&vec!["view.detail"]));
    assert_eq!(map.get(&("view.detail", 0)), Some(&vec!["view.survey"]));
}

#[test]
fn contextual_pairs_are_not_clashes() {
    let map = clash_map(&[("crop.rotateAspect", "X")]);
    assert!(!map.contains_key(&("crop.rotateAspect", 0)), "{map:?}");
}

/// The list and its clash table are built once and reused frame after frame; a new shortcut or
/// search builds them again.
#[test]
fn the_list_is_rebuilt_only_when_what_it_shows_changes() {
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
    let ctx = egui::Context::default();
    let keymap = Keymap::new();
    let (rows, clashes) = listing(&app, &ctx, &keymap, "", false, true);
    let (again, clashes_again) = listing(&app, &ctx, &keymap, "", false, true);
    assert!(std::sync::Arc::ptr_eq(&rows, &again), "the same frame's list is reused");
    assert!(std::sync::Arc::ptr_eq(&clashes, &clashes_again));
    let (narrowed, clashes_same) = listing(&app, &ctx, &keymap, "survey", false, true);
    assert!(!std::sync::Arc::ptr_eq(&rows, &narrowed) && narrowed.len() < rows.len(), "a search narrows it");
    assert!(std::sync::Arc::ptr_eq(&clashes, &clashes_same), "the clashes don't depend on the search");
    let changed: Keymap = [("view.survey#2".to_string(), "D".to_string())].into_iter().collect();
    let (_, new_clashes) = listing(&app, &ctx, &changed, "survey", false, true);
    assert!(!std::sync::Arc::ptr_eq(&clashes, &new_clashes) && new_clashes.slots.contains_key(&("view.survey", 1)), "a new key is seen at once");
}

fn found(app: &LightcraftApp, needle: &str) -> Vec<(&'static str, Vec<String>)> {
    rows(app, &Keymap::new(), needle, false, true).into_iter().map(|r| (r.b.id, r.slots.into_iter().flatten().collect())).collect()
}

fn demo_app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default())
}

#[test]
fn the_search_reads_modifier_words_and_symbols() {
    let app = demo_app();
    // every command found by "cmd shift" has a step with both modifiers
    for needle in ["cmd shift", "⌘⇧", "cmd+shift", "command shift"] {
        let found = found(&app, needle);
        assert!(!found.is_empty(), "{needle}");
        for (id, slots) in &found {
            let ok = slots.iter().flat_map(|s| s.split_whitespace()).any(|step| step.contains("Cmd") && step.contains("Shift"));
            assert!(ok, "{needle}: {id} {slots:?}");
        }
    }
    // a lone modifier word finds only the shortcuts that use it
    for (needle, name) in [("alt", "Alt"), ("option", "Alt"), ("⌥", "Alt"), ("shift", "Shift"), ("⇧", "Shift"), ("ctrl", "Ctrl"), ("⌃", "Ctrl")]
    {
        let found = found(&app, needle);
        assert!(!found.is_empty(), "{needle}");
        assert!(found.iter().all(|(_, slots)| slots.iter().any(|s| s.contains(name))), "{needle}");
    }
    // symbols and a key: ⌘P is the Command Palette
    assert!(found(&app, "⌘p").iter().any(|(id, _)| *id == "view.commandPalette"));
    assert!(found(&app, "cmd+p").iter().any(|(id, _)| *id == "view.commandPalette"));
    // names, menus and ids still match
    assert!(found(&app, "survey").iter().any(|(id, _)| *id == "view.survey"));
    assert!(found(&app, "view.survey").iter().any(|(id, _)| *id == "view.survey"));
}

#[test]
fn a_search_lists_commands_without_a_shortcut_so_one_can_be_given() {
    let app = demo_app();
    let unbound = shortcuts::bindable().iter().find(|b| b.default.is_none() && model::default2(b.id).is_none()).expect("a command with no shortcut");
    assert!(found(&app, "").iter().all(|(id, _)| *id != unbound.id), "without a search it is hidden");
    let by_id = found(&app, &unbound.id.to_lowercase());
    assert!(by_id.iter().any(|(id, slots)| *id == unbound.id && slots.is_empty()), "{}", unbound.id);
    assert!(rows(&app, &Keymap::new(), "", true, true).iter().any(|r| r.b.id == unbound.id), "Show commands without a shortcut lists it too");
}

#[test]
fn the_list_holds_commands_only() {
    let app = demo_app();
    let rows = rows(&app, &Keymap::new(), "", true, true);
    assert!(!rows.is_empty());
    for r in &rows {
        assert!(shortcuts::find_bindable(r.b.id).is_some(), "{}", r.b.id);
    }
    // no recent file, library or open photo stands in the list as a command
    assert!(rows.iter().all(|r| !r.path.contains("Open Recent") && !r.b.id.starts_with("library.openRecent")), "a recent item is listed");
}

#[test]
fn the_columns_fill_the_width_and_keep_the_fixed_ones() {
    let narrow = rows::Cols::for_width(640.0);
    let wide = rows::Cols::for_width(1200.0);
    assert!(wide.action > narrow.action && wide.when > narrow.when, "{narrow:?} {wide:?}");
    assert!((wide.total - 1200.0).abs() < 0.5, "{wide:?}");
    assert!(narrow.action >= 170.0 && narrow.when >= 230.0);
}

#[test]
fn delete_picks_the_slot_that_is_there() {
    let slots = [Some("N".to_string()), None];
    assert_eq!(rows::removable_slot(&slots, 0), Some(0));
    assert_eq!(rows::removable_slot(&slots, 1), Some(0), "the picked slot is empty: the other one");
    assert_eq!(rows::removable_slot(&[None, None], 0), None);
}

#[test]
fn the_reset_tooltip_names_the_shortcut_it_restores() {
    let find = |id| shortcuts::find_bindable(id).expect("a command");
    assert_eq!(rows::reset_tip(find("view.survey"), true), "Reset to N");
    assert_eq!(rows::reset_tip(find("view.commandPalette"), true), "Reset to ⌘P");
    assert_eq!(rows::reset_tip(find("view.commandPalette"), false), "Reset to Ctrl+P");
    let none = shortcuts::bindable().iter().find(|b| b.default.is_none()).expect("a command with none");
    assert_eq!(rows::reset_tip(none, true), "Reset to no shortcut");
}
