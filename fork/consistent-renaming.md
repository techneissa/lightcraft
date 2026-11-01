# Renaming, one way everywhere

## What it does

Every list row is renamed in place, by one set of rules, and the file renames no longer freeze the window.

- Albums, folders of albums, keywords, presets, versions, masks, mask components and effect presets: double-click the row (or Rename… in its menu) and the name becomes a text field (`TextField`). Return or a click away saves, Esc cancels, the shortcuts stay quiet while typing.
- A name that is empty, or that another item of the same list has (albums: the same folder; masks, components, versions: the same photo or mask; presets: the same group; letter case ignored), is refused with the reason in a note under the field. Return keeps the field open; a click away puts the old name back and toasts why. The commands (`album.rename`, `mask.rename`, `mask.component` rename, `preset.rename`, `version.rename`) refuse the same names through `names::check_name`, so a field and a command agree.
- A keyword renamed onto an existing one opens the Rename Keyword window, which asks "“x” already exists. Merge “y” into it?" and changes nothing until confirmed. People keep their Rename Person window (it already asks before merging).
- Rename Photos and Rename Folder with `background: true` (what their windows send) run the disk work on a worker thread with a `rename` row in the activity stack. The catalog follows in one undo step when the files have moved; when a move fails, the files that moved are put back and the catalog is untouched (the engine's all-or-nothing `move_all`). The worker also works out the new names (a disk check per photo) and waits for queued XMP writes; automatic XMP writes asked for meanwhile are held and gathered again at the new paths afterwards. The activity row counts the files. One rename runs at a time, none while an export is running, and no export while one runs. A file or folder name, or a Rename Photos template, with a character a file name can't hold (`/ \ : * ? " < > |`, a control character) or an empty template is refused, naming the character; `{date:%Y%m%d}`-style tokens are fine. Without `background`, `photo.rename` and `folder.rename` run in place as before.
- The Rename Photos file-name preview plans on the disk once the template has rested for 0.25 s, not on every key (the first plan, and the first after a pause, are immediate).

Not done: F2 or Return on a selected sidebar row (F2 is Rename Photos), a rename journal for a crash between the file moves and the catalog commit.

## Branch

`feat/27-consistent-renaming`

## Own files

Engine: `crates/engine/src/names.rs`, `rename_job.rs` and `rename_job/tests.rs`, `tests/rename_names.rs`. UI: `crates/ui-egui/src/inline_rename.rs`, `rename_task.rs`, `debounce.rs`, `tests/renaming.rs`.

## Hooks in official files

Engine: `cmd/library/specs.rs` (`album.rename`), `cmd/masks.rs` (`mask.rename`, component rename), `cmd/develop.rs` (`preset.rename`, `version.rename`), `cmd/browse.rs` (`move_folder` split into `prepare_folder_move` / `commit_folder_move`, `rename_folder_target`), `cmd/manage.rs` (`rename_args` is `pub(crate)` and refuses a bad template), `cmd/mask_effects.rs` (`names::check_name`), `rename/tokens.rs` (`name_char_problem`, `rename_template_error`), `rename/moves.rs` (`move_all_with_progress`), `sidecar_queue.rs` (hold during a rename), `sidecar.rs`, `rename/plan.rs` (`apply_file_plans` split into `prepare_file_plans` / `settle_file_plans`), `rename.rs`, `lib.rs`, `tests.rs`. UI: `lib.rs` (modules, `rename_task` field), `state.rs` (`rename_row`), `app.rs` (`photo.rename` / `folder.rename` with `background`), `app/frame.rs` (poll), `panels/left/rows.rs` (the field in a row), `left/albums.rs`, `left/folders.rs`, `left/keywords.rs`, `panels/presets.rs`, `panels/right/history.rs`, `panels/masking.rs`, `panels/mask_effects.rs`, `dialogs.rs` (the merge question), `dialogs/confirm.rs` (`background: true`), `dialogs/rename.rs` (debounce), `tests.rs`, `docs/whats-new.md`.

## Tests

`cargo test -p lightcraft-engine --lib -- names:: rename_names rename_job rename::` and `cargo test -p lightcraft-ui-egui --lib -- renaming debounce mask_list_rename` (in `fork/tests.sh`).
