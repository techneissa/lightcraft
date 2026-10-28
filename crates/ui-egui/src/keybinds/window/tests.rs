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
