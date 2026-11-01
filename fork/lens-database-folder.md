# Lens database folder

## What it does

Settings ▸ Import ▸ Lens profiles: Choose…, Clear and a status line (profiles found, or why none; no line while no folder is set; "Reading…" while a worker reads a big folder); `LIGHTCRAFT_LENSFUN_DIR` still wins; the folder is read again when it changes; `lensDatabase.get` / `set` / `reload` (choosing the same folder again reads it again; `reading: true` until the worker ends).

## Branch

`claude/workflow-qol`

## Own files

`crates/ui-egui/src/dialogs/settings/lens_database.rs`, `crates/engine/src/tests/lens_database.rs`, `crates/ui-egui/src/tests/lens_database.rs`

## Hooks in official files

engine `lens_profiles.rs` (a reloadable database and `status()`, instead of one read per process), `dialogs/settings.rs` and `settings/import.rs` (module, one call), `state/settings.rs` (`lens_database_dir`), `app/prefs.rs` (one `sync` call), `menus/run.rs`, `menus/ui_commands.rs`; `docs/lens-profiles.md`

## Tests

`lens_database`
