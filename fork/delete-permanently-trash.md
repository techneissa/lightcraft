# Delete Permanently asks about the files

## What it does

Delete Permanently asks about the files: a dialog (Move to Trash / Remove from Library / Cancel) on Delete Permanently and Empty Recently Deleted, and Settings ▸ General ▸ Culling ▸ Delete Permanently (Ask Each Time, Move Files to Trash, Remove from Library Only; `deleteFiles` in `ui.json`). Trash moves the files and XMP sidecars with the `trash` crate (MIT; `NSFileManager` on macOS), never a file another photo still uses. Commands take `files: keep|trash`, default `keep`.

## Branch

`fix/delete-permanently-trash`

## Own files

`crates/engine/src/file_trash.rs`, `crates/ui-egui/src/dialogs/delete_permanently.rs`, `crates/ui-egui/src/state/delete_files.rs`, tests `crates/engine/src/tests/file_trash.rs`, `crates/ui-egui/src/tests/delete_permanently.rs`

## Hooks in official files

engine `lib.rs` (module, `trasher` field), `cmd/library/specs.rs` (both commands); UI `dialogs.rs` (title, button, body, width), `dialogs/confirm.rs`, `dialogs/frame.rs`, `dialogs/settings/general.rs`, `menus/menubar.rs` (`run_item`), `panels/grid.rs`, `state.rs`, `state/dialog.rs`, `state/settings.rs`, `tests/scroll.rs`; on `custom` also `panels/photo_menu/items.rs` (calls `run_item`); `trash` in `Cargo.toml` and `crates/engine/Cargo.toml`

## Tests

`cargo test -p lightcraft-engine --lib file_trash`, `cargo test -p lightcraft-ui-egui --lib delete_permanently`
