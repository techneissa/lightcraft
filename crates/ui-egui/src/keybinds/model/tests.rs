use super::*;

fn keymap() -> Keymap {
    Keymap::new()
}

fn steps(sc: &str) -> Vec<Step> {
    chord::parse_chord(sc).unwrap()
}

#[test]
fn slot_keys_round_trip() {
    assert_eq!(slot_key("view.survey", 0), "view.survey");
    assert_eq!(slot_key("view.survey", 1), "view.survey#2");
    assert_eq!(split_key("view.survey"), ("view.survey", 0));
    assert_eq!(split_key("view.survey#2"), ("view.survey", 1));
    // a number that is no slot, or junk after the #, is part of the id (and so an unknown command)
    assert_eq!(split_key("view.survey#9"), ("view.survey#9", 0));
    assert_eq!(split_key("view.survey#x"), ("view.survey#x", 0));
    assert_eq!(split_key("view.survey#1"), ("view.survey#1", 0));
}

#[test]
fn a_command_has_two_shortcuts() {
    let mut km = keymap();
    assert_eq!(effective_slots(&km, "view.survey"), [Some("N"), None]);
    assign(&mut km, "view.survey", 1, Some("Shift+K")).unwrap();
    // the declared primary stays; only the second slot is saved
    assert_eq!(effective_slots(&km, "view.survey"), [Some("N"), Some("Shift+K")]);
    assert_eq!(km.len(), 1);
    assert_eq!(km.get("view.survey#2").map(String::as_str), Some("Shift+K"));
    assign(&mut km, "view.survey", 0, Some("Shift+J")).unwrap();
    assert_eq!(effective_slots(&km, "view.survey"), [Some("Shift+J"), Some("Shift+K")]);
    assign(&mut km, "view.survey", 1, None).unwrap();
    assert_eq!(effective_slots(&km, "view.survey"), [Some("Shift+J"), None]);
    // the second shortcut is stored under its own entry, which is gone once it is removed
    assert!(!km.contains_key("view.survey#2"));
}

#[test]
fn a_chord_is_a_shortcut_in_either_slot() {
    let mut km = keymap();
    assign(&mut km, "view.survey", 0, Some("Cmd+K  C")).unwrap();
    assign(&mut km, "view.compare", 1, Some("Cmd+K Cmd+Shift+C")).unwrap();
    assert_eq!(effective(&km, "view.survey", 0), Some("Cmd+K C"));
    assert_eq!(effective(&km, "view.compare", 1), Some("Cmd+K Cmd+Shift+C"));
    let all = chords(&km);
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all.iter().any(|(id, [a, b])| *id == "view.survey" && *a == steps("Cmd+K")[0] && *b == steps("C")[0]));
    // a chord is not a single key: the key pass never fires it on its first step
    // (a chord is no single key; the declared second keys are not the user's)
    assert!(second_singles(&km).iter().all(|(_, _, _, declared)| *declared));
}

#[test]
fn a_second_shortcut_that_is_one_key_is_listed() {
    let mut km = keymap();
    assign(&mut km, "view.survey", 1, Some("Shift+K")).unwrap();
    assert!(
        second_singles(&km)
            .iter()
            .any(|(id, step, sc, declared)| *id == "view.survey" && *step == steps("Shift+K")[0] && sc == "Shift+K" && !*declared)
    );
    assert!(extra_taken(&km).contains(&steps("Shift+K")[0]));
}

#[test]
fn the_same_key_moves_between_commands_and_slots() {
    let mut km = keymap();
    // N is Survey's declared key: taking it for Compare's second slot removes it from Survey
    let lost = assign(&mut km, "view.compare", 1, Some("N")).unwrap();
    assert_eq!(lost, vec![("view.survey", 0)]);
    assert_eq!(effective_slots(&km, "view.survey"), [None, None]);
    assert_eq!(effective_slots(&km, "view.compare")[1], Some("N"));
    // and from a second slot: Detail's second N takes it from Compare's second slot
    let lost = assign(&mut km, "view.detail", 1, Some("N")).unwrap();
    assert_eq!(lost, vec![("view.compare", 1)]);
    assert_eq!(effective(&km, "view.compare", 1), None);
}

#[test]
fn a_single_key_and_the_chord_it_leads_conflict_both_ways() {
    let mut km = keymap();
    assign(&mut km, "view.compare", 0, Some("Cmd+K C")).unwrap();
    // reported before saving …
    assert_eq!(conflicts(&km, "view.survey", &steps("Cmd+K")), vec![("view.compare", 0)]);
    // … and a single key that leads a chord takes the chord away when it is saved
    let lost = assign(&mut km, "view.survey", 0, Some("Cmd+K")).unwrap();
    assert_eq!(lost, vec![("view.compare", 0)]);
    assert_eq!(effective(&km, "view.compare", 0), None);
    // the other way round: a chord whose first step is a key another command has
    let lost = assign(&mut km, "view.detail", 1, Some("Cmd+K D")).unwrap();
    assert_eq!(lost, vec![("view.survey", 0)]);
    // two chords with the same leader and different second keys live together
    assign(&mut km, "view.compare", 0, Some("Cmd+K C")).unwrap();
    assert!(conflicts(&km, "view.survey", &steps("Cmd+K S")).is_empty());
}

#[test]
fn a_command_does_not_block_itself() {
    let mut km = keymap();
    assign(&mut km, "view.survey", 0, Some("Cmd+K")).unwrap();
    // the chord led by its own first shortcut replaces that one instead of conflicting
    let lost = assign(&mut km, "view.survey", 1, Some("Cmd+K S")).unwrap();
    assert!(lost.is_empty());
    assert_eq!(effective_slots(&km, "view.survey"), [None, Some("Cmd+K S")]);
}

#[test]
fn contextual_pairs_may_share_a_key() {
    let mut km = keymap();
    let lost = assign(&mut km, "crop.rotateAspect", 1, Some("X")).unwrap();
    assert!(lost.is_empty(), "{lost:?}");
    assert_eq!(effective(&km, "photo.reject", 0), Some("X"));
}

#[test]
fn a_chord_a_contextual_partner_shares_is_still_a_conflict() {
    let mut km = keymap();
    // X is Reject's and the crop swap's on purpose; a chord that X leads would swallow it
    assert_eq!(conflicts(&km, "crop.rotateAspect", &steps("X C")), vec![("photo.reject", 0)]);
    let lost = assign(&mut km, "crop.rotateAspect", 1, Some("X C")).unwrap();
    assert_eq!(lost, vec![("photo.reject", 0)]);
}

#[test]
fn declared_second_shortcuts_are_editable_values() {
    let mut km = keymap();
    assert_eq!(effective_slots(&km, "view.filmstrip")[1], Some("F6"));
    assert_eq!(effective_slots(&km, "library.selectNone")[1], Some("Cmd+D"));
    assert!(!is_changed(&km, "view.filmstrip"));
    let singles = second_singles(&km);
    assert!(singles.iter().any(|(id, _, sc, declared)| *id == "view.filmstrip" && sc == "F6" && *declared));
    // changing it saves an entry; the declared value is not fired any more
    assign(&mut km, "view.filmstrip", 1, Some("F9")).unwrap();
    assert_eq!(effective(&km, "view.filmstrip", 1), Some("F9"));
    assert!(second_singles(&km).iter().any(|(id, _, sc, declared)| *id == "view.filmstrip" && sc == "F9" && !*declared));
    // removing it keeps an empty entry, so F6 does not come back
    assign(&mut km, "view.filmstrip", 1, None).unwrap();
    assert_eq!(effective(&km, "view.filmstrip", 1), None);
    assert_eq!(km.get("view.filmstrip#2").map(String::as_str), Some(""));
    assert!(is_changed(&km, "view.filmstrip"));
    // setting it back to the declared key drops the entry, and Reset does too
    assign(&mut km, "view.filmstrip", 1, Some("F6")).unwrap();
    assert!(km.is_empty(), "{km:?}");
    assign(&mut km, "view.filmstrip", 1, Some("F9")).unwrap();
    reset(&mut km, "view.filmstrip").unwrap();
    assert!(km.is_empty(), "{km:?}");
}

#[test]
fn a_command_that_takes_a_declared_second_key_leaves_it_off_the_first_owner() {
    let mut km = keymap();
    let lost = assign(&mut km, "view.survey", 1, Some("F6")).unwrap();
    assert_eq!(lost, vec![("view.filmstrip", 1)]);
    assert_eq!(effective(&km, "view.filmstrip", 1), None);
    // ↺ on the filmstrip takes it back
    reset(&mut km, "view.filmstrip").unwrap();
    assert_eq!(effective(&km, "view.filmstrip", 1), Some("F6"));
    assert_eq!(effective(&km, "view.survey", 1), None);
    // a saved entry elsewhere also makes the declared key yield (a hand-edited file)
    let mut km = keymap();
    km.insert("view.survey".into(), "F6".into());
    assert_eq!(effective(&km, "view.filmstrip", 1), None);
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let mut km = keymap();
    assert!(assign(&mut km, "no.such", 0, Some("K")).unwrap_err().contains("unknown command"));
    assert!(assign(&mut km, "view.survey", 0, Some("Hyper+K")).unwrap_err().contains("not a shortcut"));
    assert!(assign(&mut km, "view.survey", 0, Some("Cmd+K C D")).unwrap_err().contains("not a shortcut"));
    assert!(assign(&mut km, "view.survey", 2, Some("K")).unwrap_err().contains("slot"));
    assert!(assign(&mut km, "view.survey", 0, Some("Cmd+Q")).unwrap_err().contains("reserved"));
    assert!(assign(&mut km, "view.survey", 1, Some("Cmd+Q K")).unwrap_err().contains("reserved"));
    // a chord may not end in an app-menu key either: the menu would take it
    assert!(assign(&mut km, "view.survey", 1, Some("F9 Cmd+Q")).unwrap_err().contains("reserved"));
    assert!(assign(&mut km, "view.survey", 1, Some("F9 Cmd+,")).unwrap_err().contains("reserved"));
    assert!(km.is_empty(), "{km:?}");
}

#[test]
fn reset_restores_both_slots() {
    let mut km = keymap();
    assign(&mut km, "view.survey", 0, Some("Shift+J")).unwrap();
    assign(&mut km, "view.survey", 1, Some("F9 S")).unwrap();
    assert!(is_changed(&km, "view.survey"));
    reset(&mut km, "view.survey").unwrap();
    assert_eq!(effective_slots(&km, "view.survey"), [Some("N"), None]);
    assert!(km.is_empty(), "{km:?}");
    assert!(!is_changed(&km, "view.survey"));
}

#[test]
fn old_single_shortcut_files_load_unchanged() {
    // what an earlier build saved: one string per command, "" for none
    let old = r#"{"view.survey":"Shift+K","view.detail":"","photo.pick":"Cmd+Alt+P"}"#;
    let km: Keymap = serde_json::from_str(old).unwrap();
    assert_eq!(effective_slots(&km, "view.survey"), [Some("Shift+K"), None]);
    assert_eq!(effective_slots(&km, "view.detail"), [None, None]);
    assert_eq!(effective_slots(&km, "photo.pick"), [Some("Cmd+Alt+P"), None]);
    // saving writes the same format back, plus the second slot's own entries
    let mut km = km;
    assign(&mut km, "view.survey", 1, Some("Cmd+K S")).unwrap();
    let saved = serde_json::to_value(&km).unwrap();
    assert_eq!(saved["view.survey"], "Shift+K");
    assert_eq!(saved["view.survey#2"], "Cmd+K S");
    let back: Keymap = serde_json::from_value(saved).unwrap();
    assert_eq!(back, km);
}

#[test]
fn a_damaged_keymap_falls_back_to_the_declared_shortcuts() {
    let mut km = keymap();
    for (k, v) in [
        ("view.survey", "Hyper+Nope"),
        ("view.compare#2", "Cmd+K C D"),
        ("view.detail#2", "???"),
        ("no.such#2", "K"),
        ("photo.pick#3", "K"),
        ("view.people#2", ""),
    ] {
        km.insert(k.to_string(), v.to_string());
    }
    // an entry that doesn't parse never replaces the declared key, and never fires
    assert_eq!(effective(&km, "view.survey", 0), Some("N"));
    assert_eq!(effective_slots(&km, "view.compare"), [Some("Shift+C"), None]);
    assert_eq!(effective(&km, "view.detail", 1), None);
    assert_eq!(effective(&km, "view.people", 1), None);
    assert!(second_singles(&km).iter().all(|(_, _, _, declared)| *declared));
    assert!(chords(&km).iter().all(|(id, _)| *id != "view.compare"));
    // junk alone is nothing for Reset All to do; an entry of a known command is
    assert!(!has_entries(&km) || km.keys().any(|k| find_bindable(split_key(k).0).is_some()));
    km.retain(|k, _| find_bindable(split_key(k).0).is_none());
    assert!(!has_entries(&km));
    km.insert("view.survey".into(), "K".into());
    assert!(has_entries(&km));
}
