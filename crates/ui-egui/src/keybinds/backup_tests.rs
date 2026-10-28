use super::*;
use crate::keybinds::model::{assign, effective, is_changed};

fn customised() -> Keymap {
    let mut k = Keymap::new();
    // a new first key, a second shortcut, a chord, a removed declared key, conditions of every kind
    assign(&mut k, "view.survey", 0, Some("K")).unwrap();
    assign(&mut k, "view.survey", 1, Some("Shift+K")).unwrap();
    conditions::set(&mut k, "view.survey", Some("!(view == detail) && !textFocus")).unwrap();
    assign(&mut k, "view.compare", 0, Some("Cmd+K C")).unwrap();
    assign(&mut k, "view.detail", 0, None).unwrap();
    conditions::set(&mut k, "photo.reject", None).unwrap(); // "always", over the declared one
    conditions::set(&mut k, "view.people", Some("view != grid")).unwrap();
    k
}

fn text(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap()
}

#[test]
fn the_file_names_its_kind_and_version_and_lists_the_customised_commands() {
    let doc = to_json(&customised());
    assert_eq!(doc["kind"], "lightcraft-shortcuts");
    assert_eq!(doc["version"], 1);
    let s = &doc["shortcuts"];
    assert_eq!(s["view.survey"]["shortcut"], "K");
    assert_eq!(s["view.survey"]["shortcut2"], "Shift+K");
    assert_eq!(s["view.survey"]["when"], "!(view == detail) && !textFocus");
    assert_eq!(s["view.compare"]["shortcut"], "Cmd+K C", "a chord");
    assert_eq!(s["view.detail"]["shortcut"], Value::Null, "a removed declared key is null");
    assert_eq!(s["photo.reject"]["when"], Value::Null, "an explicit always");
    assert!(s.get("view.filmstrip").is_none(), "commands left alone are not listed");
}

#[test]
fn a_round_trip_restores_every_binding_and_condition() {
    let before = customised();
    let back = from_text(&text(&to_json(&before)), &Keymap::new()).unwrap();
    assert!(back.notices.is_empty(), "{:?}", back.notices);
    // the five edited commands, and the ones that lost a key to the chord
    assert_eq!(back.commands, to_json(&before)["shortcuts"].as_object().unwrap().len());
    assert!(back.commands >= 5);
    assert_eq!(back.keymap, before);
    for b in shortcuts::bindable() {
        assert_eq!(model::effective_slots(&back.keymap, b.id), model::effective_slots(&before, b.id), "{}", b.id);
        assert_eq!(conditions::text(&back.keymap, b.id), conditions::text(&before, b.id), "{}", b.id);
        assert_eq!(is_changed(&back.keymap, b.id), is_changed(&before, b.id), "{}", b.id);
    }
    assert_eq!(effective(&back.keymap, "view.survey", 1), Some("Shift+K"));
    assert_eq!(conditions::text(&back.keymap, "photo.reject"), None);
}

#[test]
fn an_import_replaces_what_you_had_and_keeps_entries_for_commands_it_does_not_know() {
    let mut now = customised();
    now.insert("future.command".into(), "Cmd+9".into());
    let file = json!({"kind": KIND, "version": 1, "shortcuts": {"view.people": {"shortcut": "J"}}});
    let r = from_text(&text(&file), &now).unwrap();
    assert_eq!(effective(&r.keymap, "view.people", 0), Some("J"));
    assert_eq!(effective(&r.keymap, "view.survey", 0), Some("N"), "back to the declared key");
    assert_eq!(conditions::text(&r.keymap, "view.survey"), None);
    assert_eq!(r.keymap.get("future.command").map(String::as_str), Some("Cmd+9"));
}

#[test]
fn a_file_that_is_not_ours_or_is_newer_is_refused_with_a_clear_message() {
    let refused = |s: &str| from_text(s, &Keymap::new()).unwrap_err();
    assert!(refused("not json {").contains("isn't a LightCraft shortcuts file"), "text");
    assert!(refused("").contains("isn't readable JSON"), "empty");
    assert!(refused("[1, 2]").contains("no \"kind\""), "an array");
    assert!(refused(r#"{"version": 1, "shortcuts": {}}"#).contains("no \"kind\""));
    let foreign = refused(r#"{"kind": "vscode-keybindings", "version": 1, "shortcuts": {}}"#);
    assert!(foreign.contains("isn't a LightCraft shortcuts file") && foreign.contains("vscode-keybindings"), "{foreign}");
    let newer = refused(r#"{"kind": "lightcraft-shortcuts", "version": 2, "shortcuts": {}}"#);
    assert!(newer.contains("newer LightCraft") && newer.contains("Update LightCraft"), "{newer}");
    assert!(refused(r#"{"kind": "lightcraft-shortcuts", "shortcuts": {}}"#).contains("\"version\""));
    assert!(refused(r#"{"kind": "lightcraft-shortcuts", "version": "1", "shortcuts": {}}"#).contains("\"version\""));
    assert!(refused(r#"{"kind": "lightcraft-shortcuts", "version": 0, "shortcuts": {}}"#).contains("\"version\""));
    assert!(refused(r#"{"kind": "lightcraft-shortcuts", "version": 1}"#).contains("\"shortcuts\" list"));
    assert!(refused(r#"{"kind": "lightcraft-shortcuts", "version": 1, "shortcuts": []}"#).contains("\"shortcuts\" list"));
    assert!(refused(&" ".repeat(MAX_BYTES + 1)).contains("too big"));
}

#[test]
fn unknown_commands_are_skipped_with_a_notice_and_the_rest_is_taken() {
    let file = json!({"kind": KIND, "version": 1, "shortcuts": {
        "no.such.command": {"shortcut": "Q"},
        "view.survey": {"shortcut": "K"},
        "view.compare": "not an object",
    }});
    let r = from_text(&text(&file), &Keymap::new()).unwrap();
    assert_eq!(r.commands, 1);
    assert_eq!(effective(&r.keymap, "view.survey", 0), Some("K"));
    assert!(r.notices.iter().any(|n| n.contains("no.such.command") && n.contains("not a command")), "{:?}", r.notices);
    assert!(r.notices.iter().any(|n| n.contains("view.compare") && n.contains("isn't an object")), "{:?}", r.notices);
    assert!(!r.keymap.keys().any(|k| k.contains("no.such")));
}

#[test]
fn bad_shortcuts_and_conditions_are_dropped_with_a_notice() {
    let file = json!({"kind": KIND, "version": 1, "shortcuts": {
        "view.survey": {"shortcut": "Hyper+Q", "shortcut2": "Shift+K", "when": "viw == detail"},
        "view.compare": {"shortcut": "Cmd+Q", "when": "!(view == detail)"},
        "view.people": {"shortcut": 7, "when": 3},
        "view.detail": {"shortcut": "Cmd+K Cmd+L Cmd+M"},
    }});
    let r = from_text(&text(&file), &Keymap::new()).unwrap();
    let all = r.notices.join("\n");
    assert!(all.contains("Hyper+Q") && all.contains("not a shortcut"), "{all}");
    assert!(all.contains("Dropped the condition of") && all.contains("Unknown key “viw”"), "{all}");
    assert!(all.contains("reserved for the app menu"), "{all}");
    assert!(all.contains("isn't text"), "{all}");
    // what was fine in the same entries is kept
    assert_eq!(effective(&r.keymap, "view.survey", 0), Some("N"), "the bad first shortcut left Survey on its declared key");
    assert_eq!(effective(&r.keymap, "view.survey", 1), Some("Shift+K"));
    assert_eq!(conditions::text(&r.keymap, "view.survey"), None, "the bad condition is not saved");
    assert_eq!(conditions::text(&r.keymap, "view.compare").as_deref(), Some("!(view == detail)"));
    assert_eq!(effective(&r.keymap, "view.compare", 0), Some("Shift+C"), "the app-menu key was refused");
    assert!(conditions::damaged(&r.keymap).is_empty(), "nothing unreadable reaches the keymap");
}

#[test]
fn junk_never_panics() {
    let current = customised();
    let docs = [
        "null",
        "true",
        "{}",
        r#"{"kind": 1}"#,
        r#"{"kind": "lightcraft-shortcuts", "version": 1.5, "shortcuts": {}}"#,
        r#"{"kind": "lightcraft-shortcuts", "version": 1, "shortcuts": {"": {}, "view.survey": null, "view.compare": {"shortcut": [], "when": {}}}}"#,
        r#"{"kind": "lightcraft-shortcuts", "version": 18446744073709551616, "shortcuts": {}}"#,
        "\u{feff}{\"kind\": \"lightcraft-shortcuts\", \"version\": 1, \"shortcuts\": {\"view.survey\": {\"when\": \"((((\"}}}",
    ];
    for d in docs {
        let _ = from_text(d, &current);
    }
    let long = format!(r#"{{"kind": "lightcraft-shortcuts", "version": 1, "shortcuts": {{"view.survey": {{"when": "{}"}}}}}}"#, "(".repeat(3000));
    let r = from_text(&long, &current).unwrap();
    assert!(r.notices.iter().any(|n| n.contains("too long")), "{:?}", r.notices);
}
