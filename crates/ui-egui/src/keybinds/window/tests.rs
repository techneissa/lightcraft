use super::*;

fn clash_map(entries: &[(&str, &str)]) -> HashMap<(&'static str, usize), Vec<&'static str>> {
    let keymap: Keymap = entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    clashes(&keymap)
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
