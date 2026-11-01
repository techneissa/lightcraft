# Shortcut conditions (When)

## What it does

Shortcut conditions (When): a shortcut can be limited to a context (`view == detail && !textFocus`, `!(view == detail)`, `view != grid`); the Keyboard Shortcuts window's last column is When (chips, click to type with autocomplete, `+` builder, red inline errors, amber conflicts with the reason); two commands may share a key while their conditions exclude each other; the X and A pairs are declared conditions. Saved as the keymap entry `<id>?when`, so an old `ui.json` loads unchanged. Also Export… / Import… of all shortcuts as a JSON file (two icons in the window, `app.exportShortcuts` / `app.importShortcuts`).

## Branch

`feat/shortcut-when`

## Own files

`crates/ui-egui/src/keybinds/when.rs` and `when/` (`keys.rs`, `parse.rs`, `expr.rs`, `suggest.rs`, `tests.rs`), `keybinds/model/conditions.rs` and `conditions_tests.rs`, `keybinds/blocked.rs`, `keybinds/backup.rs` and `backup_tests.rs`, `keybinds/window/when_cell.rs`, `crates/ui-egui/src/tests/shortcut_when.rs` and `shortcut_backup.rs`, a test in `crates/ui-egui/tests/it/accessibility.rs`

## Hooks in official files

`keybinds.rs` (`mod`s, two `State` fields), `keybinds/model.rs` (conflicts, `saved_elsewhere`, `reset`, `chords_in`, `extra_taken_in`), `keybinds/window.rs` (When column replaces Where, `Clashes`), `keybinds/runtime.rs` + `overlay.rs` (chords follow the condition, damaged-condition toast), `shortcuts.rs` (`pressed_commands` checks conditions, `set_shortcut` takes `when`, `fire_owned`; the X / A special cases in `context_key_skipped` are now declared conditions), `menus/menubar.rs` (a conditional key is no native accelerator), `menus/run.rs` + `menus/ui_commands.rs` (`app.shortcutContexts`, `app.exportShortcuts`, `app.importShortcuts`), `icons.rs` (`Icon::Export`, `Icon::Import`), `services.rs` (`save_shortcuts_file`, `pick_shortcuts_file`) with `apps/lightcraft/src/session.rs` and `apps/lightcraft-web/src/web.rs`, `control.rs` (`when` in the command list), `docs/control-protocol.md`, `docs/parity-checklist.md` (LR-PREF-SHORTCUTS), `docs/whats-new.md`

## Tests

`keybinds::when`, `keybinds::model::conditions`, `keybinds::backup`, `shortcut_when`, `shortcut_backup`, and in `--test it`: `shortcuts_window_names`
