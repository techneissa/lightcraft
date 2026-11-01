# Top bar items

## What it does

Show or hide top-bar items (Settings ▸ Top Bar, or right-click the bar); the Discord button is gone.

## Branch

`feat/topbar-items`

## Own files

`crates/ui-egui/src/panels/topbar_items.rs` and `topbar_items/` (`editor.rs`, `tests.rs`)

## Hooks in official files

`panels/topbar.rs`, `panels.rs` (module), `state/settings.rs` (field), `dialogs/settings.rs` (tab), `menus/run.rs` + `menus/ui_commands.rs`, `icons.rs` and `links.rs` (Discord removed), `docs/control-protocol.md`

## Tests

`topbar_items`
