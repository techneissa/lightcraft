# Top bar items

## What it does

Show or hide top-bar items (Settings ▸ Top Bar, or right-click the bar); the Discord button is gone.

The Library Status icon (the cloud) is not an item (techneissa, 2026-11-09): as on the official side (upstream #3832) it shows by itself only while saving fails. A saved list that still names `cloud` reads fine: the id is ignored, leaves no gap on the bar, and is dropped when the list is next changed; `topBar.setItem {"item": "cloud"}` is refused as unknown.

## Branch

`feat/topbar-items`

## Own files

`crates/ui-egui/src/panels/topbar_items.rs` and `topbar_items/` (`editor.rs`, `tests.rs`)

## Hooks in official files

`panels/topbar.rs` (the cloud joins the right icons only while saving fails), `panels.rs` (module), `state/settings.rs` (field), `dialogs/settings.rs` (tab), `menus/run.rs` + `menus/ui_commands.rs`, `icons.rs` and `links.rs` (Discord removed), `docs/control-protocol.md`

## Tests

`topbar_items`, `tests::unsaved`
