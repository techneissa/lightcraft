# Keyboard Shortcuts window, fewer buttons

List: The Keyboard Shortcuts window is one resizable window (opens 1000 × 640, remembered) with VS Code-style recording in the cell, a quiet row selection, Add Shortcut at the search's right, a reset icon on a changed row, Remove Shortcut on the right-click menu and on ⌫ / Delete, a bare trash icon on a hovered cell, and a search that reads cmd / shift / ctrl / alt and ⌘ ⇧ ⌃ ⌥ (issue #49, after PhotoCraft's `feat/shortcuts-window`). Branch `feat/49-shortcuts-window`.

## What it does

Reworks the window of `shortcut-editing` and `shortcut-when` to match PhotoCraft's Keyboard Shortcuts window:

- One resizable size (opens 1000 × 640 on a screen that holds it, minimum 640 × 420, less on a very small screen); the list takes the height the window gives it. The size you leave it at is saved in the settings (`shortcutsWindowSize`) and is where it opens next time. A size at the screen's limit is the screen's, not yours, and is not saved.
- Recording like VS Code: click a shortcut, or pick a row and press Add Shortcut (first free slot); each combination pressed is shown in the cell and added, two make a chord (a third starts over), Enter saves, Esc cancels (the window stays open), the clash warning ("Also used by Survey (Shortcut 1); saving takes it away from there.") shows under the list before Enter. No Assign Shortcut box in the window; the box stays for right-click Assign Shortcut… on icons, menu items and palette rows, and for `app.assignShortcut`.
- A click on a row picks it (a quiet tinted band). ⌫ or Delete removes the picked shortcut (the clicked slot, else the other one) when nothing is being typed or recorded.
- Full-width search with Add Shortcut at its right. A search lists commands without a shortcut too, so one can be found and given a key. The search also matches modifier words (`cmd`, `shift`, `ctrl`, `alt`, `option`, `command`, `control`) and symbols (`⌘ ⇧ ⌃ ⌥`), together (`cmd shift`, `⌘⇧`, `cmd+shift+k`, `⌘n`); names, menus and ids still match.
- A reset icon on a changed row, tooltip "Reset to ⌘N". Right-click a shortcut cell: Edit Shortcut…, Remove Shortcut. Right-click a row: Add Shortcut (Change Shortcut when both are used), Edit Condition…, Remove Shortcut, Reset to Default. The When column stays as it was (click to type, + to build).
- A bare trash icon (no box, no border) appears at the right of the hovered or picked shortcut cell and removes it.
- Removed: the × button beside each shortcut as a box, the help line under the search (its text is the tooltip of the Shortcut 1 title). Export… / Import… stay as icons, Reset All to Default and "Show commands without a shortcut" sit under the list.
- A recorded shortcut is saved by `app.setShortcut`, exactly as before (a test compares the keymap the window writes with the one the command writes).

## Branch

`feat/49-shortcuts-window` (from `custom`)

## Own files

`crates/ui-egui/src/keybinds/window.rs` (rewritten), `keybinds/window/` (`rows.rs`, `record.rs`, `size.rs`, `search.rs`, `clashes.rs`, `fixed.rs`; `tests.rs`, `when_cell.rs` adapted), `crates/ui-egui/src/tests/shortcuts_window.rs`

## Hooks in official files

`dialogs.rs` (the window gets its own id, is resizable with a minimum and maximum, opens at the remembered size, saves the size when dragged, is not wrapped in the dialog's scroll area), `state/settings.rs` (`shortcuts_window_size`), `keybinds.rs` (`State::selected`), `keybinds/capture.rs` (`Capture::inline`, `open_inline`: no box while the window records), `tests/shortcut_editing.rs`, `tests/dialog_fit.rs` (the shortcuts list no longer uses the dialog list-shrink; it shrinks with the window), `tests.rs` (module line), `docs/control-protocol.md`, `docs/parity-checklist.md` (LR-PREF-SHORTCUTS, in place)

## Tests

```bash
cargo test -p lightcraft-ui-egui --lib -- shortcuts_window keybinds::window shortcut_editing shortcut_when shortcut_backup tests::keymap shortcuts:: dialog_fit
cargo test -p lightcraft-ui-egui --test it -- shortcuts_window_names
```

## What's New entry

### Keyboard Shortcuts window

The Keyboard Shortcuts window is one resizable window that remembers its size. Click a shortcut and press the keys: each combination is added, two make a chord, a warning shows before Enter if another command has the key, Enter saves and Esc cancels. Remove a shortcut with ⌫ / Delete, the trash icon or the right-click menu; the ↺ on a changed row puts it back. The search finds shortcuts by modifier (`cmd shift`, `⌘⇧`).
