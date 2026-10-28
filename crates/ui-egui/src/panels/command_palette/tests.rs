//! The Command Palette: ⌘P opens it, typing filters, Return or a click runs the highlighted row,
//! a disabled command does not run, Esc closes, and Print no longer owns ⌘P.

use std::time::Duration;

use serde_json::json;

use super::*;
use crate::headless::Headless;
use crate::state::{Dialog, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.settle(SETTLE);
    h
}

fn key(h: &mut Headless, k: &str, cmd: bool) {
    let r = h.request("ui.key", json!({"key": k, "cmd": cmd}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn type_text(h: &mut Headless, text: &str) {
    let r = h.request("ui.text", json!({"text": text}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn open(h: &mut Headless) {
    key(h, "P", true);
    assert!(h.app.ui.palette.is_some(), "Cmd+P opens the palette");
}

fn entry(label: &str, path: &str) -> Entry {
    Entry { id: label.into(), params: Value::Null, label: label.into(), path: path.into(), shortcut: None, enabled: true }
}

fn labels(found: &[Entry]) -> Vec<&str> {
    found.iter().map(|e| e.label.as_str()).collect()
}

#[test]
fn print_gives_up_cmd_p_to_the_palette() {
    let print = crate::menus::ui_commands().find(|c| c.0 == "dialog.print").unwrap();
    assert_eq!(print.2, None, "Print has no shortcut of its own any more");
    let palette = crate::menus::ui_commands().find(|c| c.0 == COMMAND).unwrap();
    assert_eq!(palette.2, Some("Cmd+P"));
    let owners: Vec<&str> = crate::menus::ui_commands().filter(|c| c.2 == Some("Cmd+P")).map(|c| c.0).collect();
    assert_eq!(owners, [COMMAND]);
    let engine: Vec<&str> = lightcraft_engine::command_specs().iter().filter(|c| c.shortcut == Some("Cmd+P")).map(|c| c.id).collect();
    assert!(engine.is_empty(), "no engine command uses Cmd+P: {engine:?}");
}

#[test]
fn words_match_labels_and_menu_paths_best_first() {
    let all = [
        entry("Rotate Left", "Photo › Rotate"),
        entry("Print…", "File"),
        entry("Export with Previous", "File"),
        entry("Export…", "File"),
        entry("Reset Sidebar Sections", "View"),
        entry("Show in Finder", "Photo"),
    ];
    assert_eq!(labels(&filter(&all, "")), labels(&all), "no query keeps the menu order");
    assert_eq!(labels(&filter(&all, "print")), ["Print…"]);
    // every word must match: a word prefix of the label, or of the menu path
    assert_eq!(labels(&filter(&all, "exp prev")), ["Export with Previous"]);
    assert_eq!(labels(&filter(&all, "photo rot")), ["Rotate Left"]);
    // a label word beats a menu word, and the shorter label wins a tie
    assert_eq!(labels(&filter(&all, "ex")), ["Export…", "Export with Previous"]);
    // loose letters still find it, in order
    assert_eq!(labels(&filter(&all, "rsts")), ["Reset Sidebar Sections"]);
    assert!(filter(&all, "zzzz").is_empty());
}

#[test]
fn highlight_wraps_and_an_empty_list_is_safe() {
    assert_eq!(step(0, -1, 3), 2);
    assert_eq!(step(2, 1, 3), 0);
    assert_eq!(step(0, 1, 0), 0);
}

#[test]
fn cmd_p_opens_and_closes_it_and_esc_closes_it() {
    let mut h = demo();
    open(&mut h);
    assert!(h.app.widgets.iter().any(|(id, _)| id == SEARCH_WIDGET), "the search box is registered");
    key(&mut h, "P", true);
    assert!(h.app.ui.palette.is_none(), "Cmd+P again closes it");
    open(&mut h);
    key(&mut h, "Escape", false);
    assert!(h.app.ui.palette.is_none(), "Esc closes it");
    assert!(h.app.ui.dialog.is_none());
}

#[test]
fn cmd_p_no_longer_opens_print() {
    let mut h = demo();
    open(&mut h);
    assert!(h.app.ui.dialog.is_none(), "{:?}", h.app.ui.dialog);
}

#[test]
fn typing_filters_and_return_runs_the_top_row() {
    let mut h = demo();
    open(&mut h);
    let all = entries(&h.app, false);
    assert!(all.len() > 50, "the palette lists the menus' commands, {}", all.len());
    assert!(all.iter().all(|e| e.id != COMMAND), "the palette is not listed in itself");
    type_text(&mut h, "square grid");
    let state = h.app.ui.palette.clone().unwrap();
    assert_eq!(state.query, "square grid");
    let shown = filter(&entries(&h.app, false), &state.query);
    assert_eq!(shown.first().map(|e| e.id.as_str()), Some("view.squareGrid"), "{:?}", labels(&shown));
    assert_ne!(h.app.ui.view, ViewMode::SquareGrid);
    key(&mut h, "Enter", false);
    assert!(h.app.ui.palette.is_none(), "running a command closes the palette");
    assert_eq!(h.app.ui.view, ViewMode::SquareGrid);
}

#[test]
fn arrow_keys_move_the_highlight_and_return_runs_that_row() {
    let mut h = demo();
    open(&mut h);
    type_text(&mut h, "grid");
    let shown = filter(&entries(&h.app, false), "grid");
    assert!(shown.len() >= 2);
    key(&mut h, "ArrowDown", false);
    assert_eq!(h.app.ui.palette.as_ref().map(|s| s.selected), Some(1));
    key(&mut h, "ArrowUp", false);
    key(&mut h, "ArrowUp", false);
    assert_eq!(h.app.ui.palette.as_ref().map(|s| s.selected), Some(shown.len() - 1), "up from the first row wraps to the last");
    assert_eq!(h.app.ui.palette.as_ref().map(|s| s.query.as_str()), Some("grid"), "the arrows did not edit the text");
}

#[test]
fn a_disabled_command_is_listed_but_does_not_run() {
    let mut h = demo();
    let all = entries(&h.app, false);
    let off = all.iter().find(|e| !e.enabled).expect("some command is disabled in the demo library");
    let on = all.iter().filter(|e| e.enabled).count();
    assert!(on > 0);
    open(&mut h);
    type_text(&mut h, &off.label);
    let shown = filter(&entries(&h.app, false), &off.label);
    assert_eq!(shown.first().map(|e| (e.key(), e.enabled)), Some((off.key(), false)));
    let (undo, redo, view) = (h.app.session.undo.len(), h.app.session.redo.len(), h.app.ui.view);
    key(&mut h, "Enter", false);
    assert!(h.app.ui.palette.is_some(), "a disabled row does not run, and the palette stays open");
    assert_eq!((h.app.session.undo.len(), h.app.session.redo.len(), h.app.ui.view), (undo, redo, view));
    assert!(h.app.ui.dialog.is_none());
}

#[test]
fn a_click_runs_a_row_and_a_dialog_command_opens_its_dialog() {
    let mut h = demo();
    open(&mut h);
    type_text(&mut h, "settings");
    let row = "palette:row:app.settings";
    assert!(h.app.widgets.iter().any(|(id, _)| id == row), "the row is registered: {:?}", h.app.widgets.iter().map(|w| &w.0).collect::<Vec<_>>());
    let r = h.request("ui.clickWidget", json!({"id": row}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(h.app.ui.palette.is_none());
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Settings { .. })), "{:?}", h.app.ui.dialog);
}

#[test]
fn shortcuts_wait_while_the_palette_is_open() {
    let mut h = demo();
    open(&mut h);
    let view = h.app.ui.view;
    type_text(&mut h, "g");
    assert_eq!(h.app.ui.view, view, "a letter typed into the palette is not the grid shortcut");
    assert_eq!(h.app.ui.palette.as_ref().map(|s| s.query.as_str()), Some("g"));
}

/// The palette rows registered this frame, top to bottom.
fn rows(h: &Headless) -> Vec<(String, egui::Rect)> {
    let mut rows: Vec<(String, egui::Rect)> = h.app.widgets.iter().filter(|(id, _)| id.starts_with("palette:row:")).cloned().collect();
    rows.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    rows
}

/// How many rows a full list shows.
fn full_list() -> usize {
    MAX_ROWS
}

// Given a query that leaves two rows, when it is erased letter by letter, then the list grows back
// to its full height (it used to keep the two-row height until the app quit).
#[test]
fn the_list_grows_back_after_a_query_that_left_few_rows() {
    let mut h = demo();
    open(&mut h);
    let all = entries(&h.app, false).len();
    assert!(all > full_list());
    let narrow = "smart album from filter";
    type_text(&mut h, narrow);
    let few = filter(&entries(&h.app, false), narrow).len();
    assert!((1..=2).contains(&few), "{few}");
    assert_eq!(rows(&h).len(), few);
    for typed in (0..narrow.len()).rev() {
        key(&mut h, "Backspace", false);
        let query = &narrow[..typed];
        assert_eq!(h.app.ui.palette.as_ref().map(|s| s.query.as_str()), Some(query));
        let want = filter(&entries(&h.app, false), query).len().min(full_list());
        assert!(rows(&h).len() >= want, "{query:?}: {} rows shown, {want} expected", rows(&h).len());
    }
    let shown = rows(&h);
    let height = shown.last().unwrap().1.bottom() - shown.first().unwrap().1.top();
    assert!(height >= MAX_LIST_H - ROW_H, "the empty query shows a full list, {height} pt");
}

// Given a short list after a long one, when ↓ walks past the rows that fit, then the highlighted row
// is always one of the rows shown, inside the list.
#[test]
fn the_arrow_keys_scroll_the_highlight_into_view() {
    let mut h = demo();
    open(&mut h);
    type_text(&mut h, "smart album from filter");
    for _ in 0.."smart album from filter".len() {
        key(&mut h, "Backspace", false);
    }
    type_text(&mut h, "e");
    let shown = filter(&entries(&h.app, false), "e");
    assert!(shown.len() > full_list() + 2, "{}", shown.len());
    for _ in 0..full_list() + 2 {
        key(&mut h, "ArrowDown", false);
    }
    let selected = h.app.ui.palette.as_ref().map(|s| s.selected).unwrap();
    assert_eq!(selected, full_list() + 2);
    let key = format!("palette:row:{}", shown[selected].key());
    let visible = rows(&h);
    assert!(visible.iter().any(|(id, _)| *id == key), "{key} is on screen: {visible:?}");
    assert!(visible.len() >= full_list(), "{} rows", visible.len());
    // rows sit one under the other, a row apart, the highlighted one inside the list
    for pair in visible.windows(2) {
        assert_eq!(pair[1].1.top() - pair[0].1.top(), ROW_H, "{pair:?}");
    }
    let list_bottom = visible.first().unwrap().1.top() + MAX_LIST_H;
    let row = visible.iter().find(|(id, _)| *id == key).unwrap().1;
    assert!(row.bottom() <= list_bottom + 0.5, "{row:?} ends above the list's bottom {list_bottom}");
}
