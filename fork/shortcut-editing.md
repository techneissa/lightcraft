# Keyboard shortcuts, VS Code style

## What it does

Keyboard shortcuts, VS Code style: two shortcuts per command (the first is what menus and the Command Palette show), two-key chords with a hint (⌘K then C), right-click Assign Shortcut… on icons, buttons, menu items and palette rows, a wider Keyboard Shortcuts window (Action / Shortcut 1 / Shortcut 2 / Where, menu groups, conflict highlight, Reset to Default). Slot 2 is the keymap entry `<id>#2`, so an old `ui.json` loads unchanged.

## Branch

`feat/shortcut-editing`

## Own files

`crates/ui-egui/src/keybinds.rs` and `keybinds/` (`chord.rs`, `model.rs`, `runtime.rs`, `capture.rs`, `context.rs`, `overlay.rs`, `window.rs`), `crates/ui-egui/src/tests/shortcut_editing.rs`

## Hooks in official files

`shortcuts.rs` (`binding`, `conflicts`, `assign`, `reset`, `set_shortcut` delegate to `keybinds::model`; chord poll, second shortcuts and the box's keys in `handle` / `pressed_commands`), `dialogs/keymap.rs` (now one re-export of `keybinds::window::body`), `apps/lightcraft/src/native_menu.rs` (a waiting chord drops the accelerators), `panels/photo_menu.rs` (right-click on a command row), `shortcuts.rs` also trims `ALIASES` (plain second keys became `keybinds::model::SECOND_DEFAULTS`), `menus/menubar.rs` (`shortcut_text` and `native_item_presentation` take chords, right-click on a menu item), `menus/run.rs` + `menus/ui_commands.rs` (`app.assignShortcut`), `widgets.rs` (`icon_button` and `text_button` attach the menu), `panels/command_palette.rs` (right-click on a row), `app/frame.rs` (`keybinds::overlay::show`), `control.rs` (`shortcut2`), `lib.rs` + `app.rs` (module, `keybinds` field), `tests/keymap.rs` and `shortcuts/tests.rs` (a recorded key now waits for Enter), `docs/parity-checklist.md`

## Tests

`keybinds`, `shortcut_editing`, `tests::keymap`, `shortcuts::`
