# Delete Rejected Photos, local photos to the Trash

## What it does

Delete Rejected Photos and Local photos to the Trash: Photo ▸ Delete Rejected Photos (⌘⌫, `photo.deleteRejected`) sends the rejected photos of the current view to Recently Deleted in one undo step; Delete Permanently always trashes Local (browsed) photos' files, whatever `files` or the setting says (an all-Local set gets a plain Move to Trash confirm); "Remove from Library" is now Keep Files.

## Branch

`fix/delete-local-rejected` (built on `fix/delete-permanently-trash`)

## Own files

`crates/engine/src/cmd/library/delete_rejected.rs`, `crates/ui-egui/src/dialogs/delete_rejected.rs`, tests `crates/engine/src/tests/delete_local_rejected.rs`, `crates/ui-egui/src/tests/delete_local_rejected.rs`

## Hooks in official files

engine `cmd.rs`, `cmd/library.rs`, `cmd/library/specs.rs` (help strings); UI `dialogs.rs`, `dialogs/confirm.rs`, `dialogs/frame.rs`, `dialogs/settings/general.rs`, `menus/menubar.rs` (menu entry, `run_item`, enabled), `state/dialog.rs` (`local` field, `ConfirmDeleteRejected`); the previous row's own files

## Tests

`cargo test -p lightcraft-engine --lib delete_local`, `cargo test -p lightcraft-ui-egui --lib delete_local`
