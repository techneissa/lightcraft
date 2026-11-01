# Arrange the photo menu

## What it does

Arrange the photo menu (Settings ▸ Photo Menu), Before/After Split in it, one photo hides what needs several.

## Branch

`feat/photo-menu` (on `feat/animated-gif-export`)

## Own files

`crates/ui-egui/src/panels/photo_menu.rs` and `photo_menu/` (`items.rs`, `editor.rs`, `tests.rs`)

## Hooks in official files

`panels.rs` (module), `state/settings.rs` (`photo_menu` field), `dialogs/settings.rs` (tab), `menus/run.rs` + `menus/ui_commands.rs` (`photoMenu.*`), one call each in `panels/grid.rs`, `detail.rs`, `detail/filmstrip.rs`, `compare.rs` (2); `grid::context_menu` stays as the reference

## Tests

`photo_menu` (its test fails when an update adds an item to the official menu: add it to `items::BUILTINS` and `DEFAULT`)
