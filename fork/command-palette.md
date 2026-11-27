# Command Palette

## What it does

Command Palette on ⌘P (Print has no shortcut).

It ranks by past use (`ui.json`, key `paletteHistory`, never synced): the word match first, then the command picked for
the typed text, then how much and how lately it was run.

## Branch

`feat/command-palette`

## Own files

`crates/ui-egui/src/panels/command_palette.rs`, its `history.rs` and its `tests.rs`

## Hooks in official files

`panels.rs`, `state.rs`, `menus/run.rs`, `menus/ui_commands.rs`, `shortcuts.rs`, `app/frame.rs`

## Tests

`command_palette`
