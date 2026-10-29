use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use super::{Input, Search, Searchable, before, matches};

const BLEND: [&str; 13] = [
    "Normal",
    "Dissolve",
    "Darken",
    "Multiply",
    "Linear Burn",
    "Lighten",
    "Linear Dodge",
    "Lighter Color",
    "Soft Light",
    "Linear Light",
    "Pin Light",
    "Hard Mix",
    "Delimit",
];

fn names(query: &str, enabled: &[bool]) -> Vec<&'static str> {
    matches(query, &BLEND, enabled).into_iter().filter_map(|i| BLEND.get(i).copied()).collect()
}

#[test]
fn label_starts_come_first_then_word_starts_then_the_rest() {
    let found = names("li", &[]);
    assert_eq!(found, ["Linear Burn", "Lighten", "Linear Dodge", "Lighter Color", "Linear Light", "Soft Light", "Pin Light", "Delimit"]);
}

#[test]
fn case_is_ignored_and_a_blank_query_matches_everything() {
    assert_eq!(names("DARK", &[]), ["Darken"]);
    assert_eq!(names("  ", &[]).len(), BLEND.len());
    assert_eq!(names("zz", &[]), Vec::<&str>::new());
    assert_eq!(names("linear b", &[]), ["Linear Burn"], "a space inside the text counts");
}

#[test]
fn a_disabled_option_never_matches() {
    let mut enabled = [true; 13];
    enabled[5] = false; // Lighten
    assert_eq!(names("lighten", &enabled), Vec::<&str>::new());
    assert!(!names("li", &enabled).contains(&"Lighten"));
    assert!(!names("", &enabled).contains(&"Lighten"));
    // An empty label (a gap in the list) is never picked either.
    assert_eq!(matches("", &["", "A"], &[]), [1]);
}

#[test]
fn letters_beyond_ascii_match_and_never_panic() {
    let labels = ["Normal", "Éclaircir", "Ébène", "Нормальный", "İstanbul", "ǅ", "a\u{301}b"];
    assert_eq!(matches("é", &labels, &[]), [1, 2]);
    assert_eq!(matches("ÉB", &labels, &[]), [2]);
    assert_eq!(matches("но", &labels, &[]), [3]);
    for q in ["i\u{307}", "ǆ", "\u{301}", "ß", "😀", "é\u{301}"] {
        let _ = matches(q, &labels, &[]);
    }
}

#[test]
fn typing_moves_the_highlight_and_enter_picks_it() {
    let mut s = Search::default();
    let r = s.apply(Input::Text("L"), &BLEND, &[]);
    assert!(r.consumed);
    s.apply(Input::Text("i"), &BLEND, &[]);
    assert_eq!(s.query, "Li");
    assert_eq!(s.apply(Input::Enter, &BLEND, &[]).pick, Some(4), "the first match, Linear Burn");
    s.apply(Input::Down, &BLEND, &[]);
    s.apply(Input::Down, &BLEND, &[]);
    assert_eq!(s.apply(Input::Enter, &BLEND, &[]).pick, Some(6), "the third match, Linear Dodge");
    for _ in 0..20 {
        s.apply(Input::Down, &BLEND, &[]);
    }
    assert_eq!(s.apply(Input::Enter, &BLEND, &[]).pick, Some(12), "↓ stops at the last match, Delimit");
    s.apply(Input::PageUp, &BLEND, &[]);
    s.apply(Input::Up, &BLEND, &[]);
    assert_eq!(s.active, 1);
    s.apply(Input::Text("x"), &BLEND, &[]);
    assert_eq!(s.apply(Input::Enter, &BLEND, &[]).pick, None, "nothing matches \"lix\"");
}

#[test]
fn backspace_and_escape_clear_before_anything_closes() {
    let mut s = Search::default();
    // Nothing typed: the keys are the list's.
    for key in [Input::Backspace, Input::Esc, Input::Enter, Input::Up, Input::Down] {
        assert!(!s.apply(key, &BLEND, &[]).consumed, "{key:?}");
    }
    s.apply(Input::Text(" "), &BLEND, &[]);
    assert!(!s.is_active(), "a space doesn't start the text");
    s.apply(Input::Text("dis"), &BLEND, &[]);
    s.apply(Input::Backspace, &BLEND, &[]);
    assert_eq!(s.query, "di");
    assert!(s.apply(Input::Esc, &BLEND, &[]).consumed);
    assert_eq!(s.query, "", "the first Esc clears the text");
    assert!(!s.apply(Input::Esc, &BLEND, &[]).consumed, "the second one is the list's");
    s.apply(Input::Text("dis"), &BLEND, &[]);
    s.apply(Input::Clear, &BLEND, &[]);
    assert!(!s.is_active());
    s.apply(Input::Text("é\u{7}"), &BLEND, &[]);
    assert_eq!(s.query, "é", "control characters are left out");
    s.apply(Input::Backspace, &BLEND, &[]);
    assert!(!s.is_active());
}

const OPTIONS: [&str; 6] = ["Normal", "Dissolve", "Darken", "Multiply", "Darker Color", "Linear Light"];

/// The app's way to a searchable list: `combo_box`, `before` and `Filter::list`.
fn draw_list(ui: &mut egui::Ui, selected: &mut usize, disabled: &[bool]) {
    let labels: Vec<String> = OPTIONS.iter().map(|s| s.to_string()).collect();
    let find = super::before_enabled(ui, "blend-mode", &labels, disabled);
    let items: Vec<usize> = (0..OPTIONS.len()).collect();
    let combo = crate::widgets::combo_box("blend-mode").selected_text(OPTIONS.get(*selected).copied().unwrap_or_default());
    let current = *selected;
    let r = combo.search(ui, &find, |ui| find.list(ui, &items, |i| *i == current, |i| labels.get(*i).cloned().unwrap_or_default()));
    crate::widgets::name_combo(&r.response, "Blend Mode");
    if let Some(i) = r.inner.flatten() {
        *selected = i;
    }
}

fn dropdown_harness() -> Harness<'static, usize> {
    let mut ready = false;
    Harness::builder().with_size(egui::vec2(300.0, 400.0)).build_ui_state(
        move |ui, selected: &mut usize| {
            if !ready {
                crate::theme::install_fonts(ui.ctx());
                crate::theme::apply(ui.ctx());
                ready = true;
            }
            draw_list(ui, selected, &[]);
        },
        0,
    )
}

fn open(h: &mut Harness<'static, usize>) {
    h.get_by_role(Role::ComboBox).click();
    h.run();
}

fn type_text(h: &mut Harness<'static, usize>, text: &str) {
    h.event(egui::Event::Text(text.into()));
    h.run();
}

fn shown(h: &Harness<'static, usize>, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

#[test]
fn an_untouched_list_has_no_field_and_all_its_options() {
    let mut h = dropdown_harness();
    open(&mut h);
    assert!(OPTIONS.iter().all(|o| shown(&h, o)));
    assert!(h.query_by_label("Search").is_none());
    assert!(super::list_open(&h.ctx), "an open list owns the keyboard even before anything is typed");
}

#[test]
fn typing_narrows_the_open_list_and_enter_picks() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "d");
    assert!(shown(&h, "Darken") && shown(&h, "Dissolve") && shown(&h, "Darker Color"));
    assert!(!shown(&h, "Normal") && !shown(&h, "Multiply"));
    type_text(&mut h, "ar");
    assert!(shown(&h, "Darken") && shown(&h, "Darker Color") && !shown(&h, "Dissolve"));
    assert_eq!(*h.state(), 0, "typing alone picks nothing");
    h.key_press(egui::Key::ArrowDown);
    h.run();
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(*h.state(), 4, "↓ then ↩ picks the second match, Darker Color");
    h.run();
    h.run();
    assert!(!super::list_open(&h.ctx), "↩ closes the list");
    // Opened again: the text typed before is gone.
    open(&mut h);
    assert!(shown(&h, "Normal") && shown(&h, "Multiply"));
}

#[test]
fn enter_with_a_search_picks_the_first_match() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "LI");
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(*h.state(), 5, "\"li\" ranks Linear Light first of the matches");
}

#[test]
fn a_click_on_a_match_picks_it() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "mul");
    h.get_by_label("Multiply").click();
    h.run();
    h.run();
    assert_eq!(*h.state(), 3);
    h.run();
    h.run();
    assert!(!super::list_open(&h.ctx), "the list closes on a click");
}

#[test]
fn escape_clears_the_text_and_then_closes_the_list() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "dar");
    assert!(!shown(&h, "Normal"));
    h.key_press(egui::Key::Escape);
    h.run();
    h.run();
    assert!(shown(&h, "Normal") && shown(&h, "Darken"), "the first Esc clears the text and keeps the list open");
    h.key_press(egui::Key::Escape);
    h.run();
    h.run();
    h.run();
    assert!(!shown(&h, "Darken"), "the second closes it");
    assert_eq!(*h.state(), 0);
}

#[test]
fn nothing_matching_shows_one_dim_line() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "zzz");
    assert!(shown(&h, "No match"));
    assert!(!shown(&h, "Normal") && !shown(&h, "Darken"));
    h.key_press(egui::Key::Enter);
    h.run();
    assert_eq!(*h.state(), 0, "↩ with no match picks nothing");
    type_text(&mut h, "é");
    assert!(shown(&h, "No match"), "a letter beyond ASCII doesn't panic");
    for _ in 0..4 {
        h.key_press(egui::Key::Backspace);
    }
    h.run();
    assert!(shown(&h, "Normal"), "Backspace takes the text back out");
}

#[test]
fn a_letter_beyond_ascii_finds_its_option() {
    let mut h = Harness::builder().with_size(egui::vec2(300.0, 400.0)).build_ui_state(
        |ui, selected: &mut usize| {
            let labels = ["Normal", "Éclat", "Ébène", "Zoom"];
            let find = before(ui, "accents", &labels);
            let items: Vec<usize> = (0..labels.len()).collect();
            let r = crate::widgets::combo_box("accents")
                .selected_text("Normal")
                .search(ui, &find, |ui| find.list(ui, &items, |_| false, |i| labels.get(*i).map(|s| s.to_string()).unwrap_or_default()));
            crate::widgets::name_combo(&r.response, "Accents");
            if let Some(i) = r.inner.flatten() {
                *selected = i;
            }
        },
        0,
    );
    h.get_by_role(Role::ComboBox).click();
    h.run();
    type_text(&mut h, "é");
    assert!(shown(&h, "Éclat") && shown(&h, "Ébène") && !shown(&h, "Zoom"));
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(*h.state(), 1);
}

#[test]
fn a_disabled_option_is_never_found_or_picked() {
    let mut h = Harness::builder()
        .with_size(egui::vec2(300.0, 400.0))
        .build_ui_state(|ui, selected: &mut usize| draw_list(ui, selected, &[true, true, false]), 0);
    h.get_by_role(Role::ComboBox).click();
    h.run();
    type_text(&mut h, "dar");
    assert!(!shown(&h, "Darken"), "Darken is disabled");
    assert!(shown(&h, "Darker Color"));
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(*h.state(), 4, "↩ skips the disabled Darken");
}

#[test]
fn the_field_is_named_search_and_takes_ime_text() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "d");
    assert!(h.query_by_label("Search").is_some(), "the field has the accessible name Search");
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: "a".into(), active_range_chars: None }));
    h.run();
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("ar".into())));
    h.run();
    h.run();
    assert!(h.query_by_label("Darken").is_some() && h.query_by_label("Dissolve").is_none(), "an IME commit lands in the field");
}

#[test]
fn a_composing_ime_shows_the_field_before_any_text_is_typed() {
    let mut h = dropdown_harness();
    open(&mut h);
    assert!(h.query_by_label("Search").is_none(), "no field before typing");
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit { text: "d".into(), active_range_chars: None }));
    h.run();
    h.run();
    assert!(h.query_by_label("Search").is_some(), "the field shows to take the preedit");
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("dar".into())));
    h.run();
    h.run();
    assert!(h.query_by_label("Darken").is_some() && h.query_by_label("Normal").is_none());
}

/// A letter typed in the search ("g" is Grid ↔ Square Grid) must not run its shortcut, and a held
/// Backslash (Show Original) must not count while a list is open.
#[test]
fn keys_typed_in_an_open_list_never_reach_the_shortcuts() {
    use crate::LightcraftApp;
    use crate::state::ViewMode;

    let app = LightcraftApp::new(lightcraft_engine::Session::new(), Default::default());
    let mut ready = false;
    let mut h = Harness::builder().with_size(egui::vec2(400.0, 400.0)).build_ui_state(
        move |ui, app: &mut LightcraftApp| {
            if !ready {
                crate::theme::install_fonts(ui.ctx());
                crate::theme::apply(ui.ctx());
                ready = true;
                return;
            }
            crate::shortcuts::handle(app, ui.ctx());
            let mut selected = 0;
            draw_list(ui, &mut selected, &[]);
        },
        app,
    );
    h.run_steps(4);
    let before = h.state().ui.view;
    let key = |key, pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: egui::Modifiers::NONE };
    // Closed, G runs its shortcut (the control for the rest of the test).
    h.event(key(egui::Key::G, true));
    h.event(key(egui::Key::G, false));
    h.run_steps(3);
    assert_ne!(h.state().ui.view, before, "G toggles the grid with no list open");
    let grid = h.state().ui.view;
    assert!(matches!(grid, ViewMode::PhotoGrid | ViewMode::SquareGrid), "{grid:?}");
    h.get_by_role(Role::ComboBox).click();
    h.run_steps(3);
    // "g" is Linear Light's letter; Space is Toggle Zoom's key
    h.event(key(egui::Key::G, true));
    h.event(egui::Event::Text("g".into()));
    h.event(key(egui::Key::G, false));
    h.run_steps(3);
    assert_eq!(h.state().ui.view, grid, "a typed G in the open list runs no shortcut");
    assert!(super::list_open(&h.ctx));
    assert!(h.query_by_label("Search").is_some() && h.query_by_label("Linear Light").is_some() && h.query_by_label("Normal").is_none());
    let zoom = h.state().ui.zoom;
    h.event(key(egui::Key::Space, true));
    h.event(egui::Event::Text(" ".into()));
    h.event(key(egui::Key::Space, false));
    h.run_steps(3);
    assert_eq!((h.state().ui.view, h.state().ui.zoom), (grid, zoom), "Space typed in the search runs no shortcut");
    assert!(h.query_by_label("Linear Light").is_some(), "the space is part of the text");
    // Closed again, the shortcut works.
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(4);
    assert!(!super::list_open(&h.ctx));
    h.event(key(egui::Key::G, true));
    h.event(key(egui::Key::G, false));
    h.run_steps(3);
    assert_ne!(h.state().ui.view, grid, "G works again once the list is closed");
}

fn many_presets() -> lightcraft_engine::presets::PresetList {
    let mut items: Vec<lightcraft_develop::Preset> = (0..2000)
        .map(|i| lightcraft_develop::Preset {
            id: format!("p{i}"),
            name: format!("Look {i:04}"),
            group: "Looks".into(),
            settings: serde_json::json!({}),
            favorite: false,
            builtin: false,
        })
        .collect();
    items.push(lightcraft_develop::Preset {
        id: "warm".into(),
        name: "Golden Warm Portrait".into(),
        group: "Portraits".into(),
        settings: serde_json::json!({}),
        favorite: false,
        builtin: false,
    });
    lightcraft_engine::presets::PresetList::new(items)
}

/// The preset picker (settings, import): "None" first, then 2000 presets drawn lazily; typing finds
/// one among them, and ↩ picks it.
#[test]
fn a_preset_picker_searches_its_fixed_rows_and_every_preset() {
    use super::presets::{self, Choice};
    let mut h = Harness::builder().with_size(egui::vec2(400.0, 500.0)).build_ui_state(
        |ui, state: &mut (lightcraft_engine::presets::PresetList, Option<Choice>)| {
            let fixed = [("None".to_string(), true)];
            let find = presets::before(ui, "import-preset", &["None".to_string()], &state.0);
            let r = crate::widgets::combo_box("import-preset").selected_text("None").search(ui, &find, |ui| {
                if let Some(c) = presets::list(ui, &find, "import-preset", &fixed, &state.0, None) {
                    state.1 = Some(c);
                }
            });
            crate::widgets::name_combo(&r.response, "Preset");
        },
        (many_presets(), None),
    );
    h.get_by_role(Role::ComboBox).click();
    h.run();
    assert!(h.query_by_label("None").is_some(), "the fixed row shows before anything is typed");
    h.event(egui::Event::Text("golden".into()));
    h.run();
    h.run();
    assert!(h.query_by_label("Golden Warm Portrait").is_some() && h.query_by_label("None").is_none(), "typing narrows to the one preset");
    assert!(h.query_by_label("Look 0001").is_none());
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().1, Some(Choice::Preset("warm".into())), "↩ picks the match, though 2000 presets are not all drawn");
    // Opened again, "non" finds the fixed row, and a click picks it.
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.event(egui::Event::Text("non".into()));
    h.run();
    h.run();
    h.get_by_label("None").click();
    h.run();
    h.run();
    assert_eq!(h.state().1, Some(Choice::Fixed(0)));
}

/// Down moves the highlight to a match far below the first rows: the list scrolls to it, and ↩ picks it.
#[test]
fn the_highlight_can_go_far_down_a_preset_list() {
    use super::presets::{self, Choice};
    let mut h = Harness::builder().with_size(egui::vec2(400.0, 500.0)).build_ui_state(
        |ui, state: &mut (lightcraft_engine::presets::PresetList, Option<Choice>)| {
            let find = presets::before(ui, "p", &[], &state.0);
            let r = crate::widgets::combo_box("p").selected_text("-").search(ui, &find, |ui| {
                if let Some(c) = presets::list(ui, &find, "p", &[], &state.0, None) {
                    state.1 = Some(c);
                }
            });
            crate::widgets::name_combo(&r.response, "Preset");
        },
        (many_presets(), None),
    );
    h.get_by_role(Role::ComboBox).click();
    h.run();
    h.event(egui::Event::Text("look".into()));
    h.run();
    for _ in 0..40 {
        h.key_press(egui::Key::PageDown);
        h.run();
    }
    h.key_press(egui::Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().1, Some(Choice::Preset("p200".into())), "forty pages of five rows down is the 201st match, Look 0200");
}

/// The Smart Album rule editor's field list searches the fields of its submenus too.
#[test]
fn the_rule_field_list_finds_a_field_inside_a_group() {
    let mut rules = lightcraft_catalog::RuleSet { mode: lightcraft_catalog::Match::All, rules: vec![crate::dialogs::rules_editor::new_rule()] };
    let env = crate::dialogs::rules_editor::Env::default();
    let mut h = Harness::builder().with_size(egui::vec2(700.0, 500.0)).with_accessibility_check(false).build_ui(move |ui| {
        crate::dialogs::rules_editor::edit(ui, &mut rules, "rules", 0, &[], &env);
    });
    h.run();
    h.get_by_label("Field").click();
    h.run();
    h.event(egui::Event::Text("filen".into()));
    h.run();
    h.run();
    assert!(h.query_by_label("Filename").is_some(), "Filename lives in the File submenu and is found by typing");
    assert!(h.query_by_label("Rating").is_none());
}

/// A click on the search field must not close the list (the combo box closes on a click in it).
#[test]
fn a_click_on_the_search_field_keeps_a_combo_list_open() {
    let mut h = dropdown_harness();
    open(&mut h);
    type_text(&mut h, "dar");
    h.get_by_label("Search").click();
    h.run();
    h.run();
    assert!(shown(&h, "Darken") && super::list_open(&h.ctx), "the list is still open after a click in the field");
    h.get_by_label("Darken").click();
    h.run();
    h.run();
    h.run();
    assert_eq!(*h.state(), 2);
    assert!(!super::list_open(&h.ctx), "a click on a row closes it");
}
