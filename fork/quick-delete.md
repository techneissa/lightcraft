# Faster deleting

## What it does

Faster deleting: ⌫ in Recently Deleted runs Delete Permanently (through `run_item`, so its dialog and setting apply); Settings ▸ General ▸ Culling ▸ Only for picked or starred photos (`askBeforeDeletePickedOnly` in `ui.json`, off by default) skips the Recently Deleted confirm when no photo being deleted is picked or starred, with an Undo toast; Delete Rejected Photos follows it.

## Branch

`feat/quick-delete` (built on `fix/delete-local-rejected`)

## Own files

`crates/ui-egui/src/dialogs/quick_delete.rs`, tests `crates/ui-egui/src/tests/quick_delete.rs`

## Hooks in official files

UI `dialogs.rs` (`mod`), `dialogs/delete_rejected.rs`, `dialogs/settings/general.rs`, `menus/files.rs`, `menus/menubar.rs`, `shortcuts.rs`, `state/settings.rs`, `tests.rs`

## Tests

`cargo test -p lightcraft-ui-egui --lib quick_delete`
