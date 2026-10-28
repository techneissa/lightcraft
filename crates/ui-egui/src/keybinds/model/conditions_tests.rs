use super::*;
use crate::keybinds::chord;
use crate::keybinds::model::{assign, conflicts, effective, is_changed, reset};
use crate::keybinds::when::Context;

fn km() -> Keymap {
    Keymap::new()
}

fn steps(sc: &str) -> Vec<chord::Step> {
    chord::parse_chord(sc).unwrap()
}

#[test]
fn the_declared_pairs_exclude_each_other() {
    for (a, b) in shortcuts::CONTEXTUAL {
        assert!(declared(a).is_some() && declared(b).is_some(), "{a} / {b} need declared conditions");
        assert!(overlap(&km(), a, b).is_none(), "{a} and {b} work at the same time");
    }
    for (id, _) in DECLARED {
        assert!(find_bindable(id).is_some(), "{id} is not a command");
    }
}

#[test]
fn declared_conditions_apply_to_the_declared_key_only() {
    let mut keymap = km();
    assert_eq!(text(&keymap, "photo.reject").as_deref(), Some("tool != crop"));
    assert_eq!(text(&keymap, "crop.rotateAspect").as_deref(), Some("tool == crop"));
    assert_eq!(text(&keymap, "view.survey"), None);
    let crop = Context::default().with("view", "detail").with("tool", "crop");
    assert!(!holds(&keymap, "photo.reject", &crop));
    assert!(holds(&keymap, "crop.rotateAspect", &crop));
    // a key the user chose is theirs: no condition comes with it
    assign(&mut keymap, "photo.reject", 0, Some("J")).unwrap();
    assert_eq!(text(&keymap, "photo.reject"), None);
    assert!(holds(&keymap, "photo.reject", &crop));
    // reset brings the declared key and its condition back
    reset(&mut keymap, "photo.reject").unwrap();
    assert_eq!(text(&keymap, "photo.reject").as_deref(), Some("tool != crop"));
}

#[test]
fn a_saved_condition_replaces_the_declared_one_and_empty_means_always() {
    let mut keymap = km();
    assert_eq!(set(&mut keymap, "photo.reject", Some("view == detail")).unwrap().as_deref(), Some("view == detail"));
    assert_eq!(text(&keymap, "photo.reject").as_deref(), Some("view == detail"));
    assert_eq!(keymap.get("photo.reject?when").map(String::as_str), Some("view == detail"));
    assert_eq!(set(&mut keymap, "photo.reject", Some("  ")).unwrap(), None);
    assert_eq!(text(&keymap, "photo.reject"), None, "an empty entry is an explicit \"always\"");
    assert_eq!(keymap.get("photo.reject?when").map(String::as_str), Some(""));
    // the declared text again: no entry needed
    set(&mut keymap, "photo.reject", Some("tool!=crop")).unwrap();
    assert!(!keymap.contains_key("photo.reject?when"));
    // a command with no declared condition: always = no entry
    set(&mut keymap, "view.survey", Some("view == detail")).unwrap();
    assert!(keymap.contains_key("view.survey?when"));
    set(&mut keymap, "view.survey", None).unwrap();
    assert!(!keymap.contains_key("view.survey?when"));
}

#[test]
fn a_bad_condition_is_refused_and_changes_nothing() {
    let mut keymap = km();
    set(&mut keymap, "view.survey", Some("view == detail")).unwrap();
    let before = keymap.clone();
    let e = set(&mut keymap, "view.survey", Some("viw == detail")).unwrap_err();
    assert!(e.contains("Unknown key “viw”"), "{e}");
    assert!(set(&mut keymap, "view.nope", Some("view == detail")).is_err());
    assert_eq!(keymap, before);
}

#[test]
fn conditions_are_saved_with_the_keymap_and_old_files_load() {
    let mut keymap = km();
    assign(&mut keymap, "view.survey", 0, Some("K")).unwrap();
    set(&mut keymap, "view.survey", Some("!(view == detail) && !textFocus")).unwrap();
    let json = serde_json::to_string(&keymap).unwrap();
    assert!(json.contains("\"view.survey?when\":\"!(view == detail) && !textFocus\""), "{json}");
    let back: Keymap = serde_json::from_str(&json).unwrap();
    assert_eq!(text(&back, "view.survey").as_deref(), Some("!(view == detail) && !textFocus"));
    assert_eq!(effective(&back, "view.survey", 0), Some("K"));
    // a file from before conditions: no entries, every shortcut always works
    let old: Keymap = serde_json::from_str(r#"{"view.survey": "K", "view.survey#2": "Shift+K"}"#).unwrap();
    assert_eq!(text(&old, "view.survey"), None);
    assert!(damaged(&old).is_empty());
    assert!(holds(&old, "view.survey", &Context::default()));
    // an old build reads the entry as an unknown command: it is no shortcut of any command
    assert!(find_bindable(split_key_for_old_build("view.survey?when")).is_none());
}

fn split_key_for_old_build(k: &str) -> &str {
    super::super::split_key(k).0
}

#[test]
fn a_damaged_condition_is_ignored_and_listed() {
    let keymap: Keymap = serde_json::from_str(
        r#"{"view.survey?when": "view ==", "photo.reject?when": "nonsense(", "gone.command?when": "view ==", "view.detail?when": "textFocus"}"#,
    )
    .unwrap();
    // ignored: as if there were none (the declared one comes back)
    assert_eq!(text(&keymap, "view.survey"), None);
    assert_eq!(text(&keymap, "photo.reject").as_deref(), Some("tool != crop"));
    assert_eq!(text(&keymap, "view.detail").as_deref(), Some("textFocus"));
    let bad: Vec<_> = damaged(&keymap).into_iter().map(|(id, ..)| id).collect();
    assert_eq!(bad, ["photo.reject", "view.survey"], "only known commands, only unreadable ones");
    // nothing panics on junk
    for junk in ["\u{0}", "((((", "é", &"x".repeat(10_000)] {
        let k: Keymap = [("view.survey?when".to_string(), junk.to_string())].into();
        let _ = (text(&k, "view.survey"), damaged(&k), of(&k, "view.survey"));
    }
}

#[test]
fn entries_of_conditions_are_not_shortcuts() {
    let mut keymap = km();
    set(&mut keymap, "view.survey", Some("view == detail")).unwrap();
    assert!(crate::keybinds::model::has_entries(&keymap));
    assert!(is_changed(&keymap, "view.survey"));
    assert!(crate::keybinds::model::chords(&keymap).is_empty());
    assert_eq!(crate::keybinds::model::second_singles(&keymap).len(), crate::keybinds::model::second_singles(&km()).len());
    assert_eq!(command_of("view.survey#2?when"), "view.survey");
    assert_eq!(command_of("view.survey?when"), "view.survey");
    assert_eq!(command_of("view.survey#2"), "view.survey");
    // Reset drops the condition with the keys
    reset(&mut keymap, "view.survey").unwrap();
    assert!(!keymap.contains_key("view.survey?when"));
}

#[test]
fn two_commands_may_share_a_key_while_their_conditions_exclude_each_other() {
    let mut keymap = km();
    set(&mut keymap, "view.survey", Some("view == detail")).unwrap();
    set(&mut keymap, "view.compare", Some("view == grid")).unwrap();
    assign(&mut keymap, "view.survey", 0, Some("J")).unwrap();
    let lost = assign(&mut keymap, "view.compare", 0, Some("J")).unwrap();
    assert!(lost.is_empty(), "{lost:?}");
    assert_eq!(effective(&keymap, "view.survey", 0), Some("J"));
    assert_eq!(effective(&keymap, "view.compare", 0), Some("J"));
    // widening one of them to always makes them clash
    set(&mut keymap, "view.compare", None).unwrap();
    assert_eq!(conflicts(&keymap, "view.compare", &steps("J")), vec![("view.survey", 0)]);
    // and a key given to a third command with no condition takes it from both
    let lost = assign(&mut keymap, "view.people", 0, Some("J")).unwrap();
    assert_eq!(lost.len(), 2, "{lost:?}");
}

#[test]
fn negated_conditions_decide_what_clashes() {
    let clash = |a: &str, b: &str| {
        let mut keymap = km();
        set(&mut keymap, "view.survey", Some(a)).unwrap();
        set(&mut keymap, "view.compare", Some(b)).unwrap();
        overlap(&keymap, "view.survey", "view.compare").is_some()
    };
    assert!(!clash("!(view == detail)", "view == detail"));
    assert!(!clash("view != detail", "view == detail"));
    assert!(!clash("!textFocus", "textFocus"));
    assert!(!clash("!(textFocus || brush)", "brush"));
    assert!(clash("view != detail", "view == grid"));
    assert!(clash("!(view == detail)", "!(view == grid)"), "both hold in the compare view");
    assert!(clash("!textFocus", "!brush"));
    assert!(!clash("!(view == detail && tool == crop) && view == detail", "tool == crop"));
    assert!(clash("!(view == detail && tool == crop)", "tool == crop"), "crop outside the loupe is in both");
}

#[test]
fn a_command_with_a_condition_has_no_native_menu_key() {
    let mut keymap = km();
    let (label, key) =
        crate::menubar::native_item_presentation(&keymap, "view.survey", &serde_json::Value::Null, "Survey", Some("Cmd+J"));
    assert_eq!((label.as_str(), key), ("Survey", Some("Cmd+J")), "no condition: AppKit owns the key");
    set(&mut keymap, "view.survey", Some("view == detail")).unwrap();
    let (label, key) =
        crate::menubar::native_item_presentation(&keymap, "view.survey", &serde_json::Value::Null, "Survey", Some("Cmd+J"));
    assert!(label.starts_with("Survey (") && key.is_none(), "{label} {key:?}: egui must see the key to check the condition");
}
