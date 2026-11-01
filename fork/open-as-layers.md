# Open as Layers in PhotoCraft

## What it does

Open as Layers in PhotoCraft: Photo ▸ Edit In ▸ Open as Layers in PhotoCraft… (2+ photos selected; also in the right-click menu's catalogue) renders each photo with its edits as a file named after the photo, writes a `.pcstack` layer list (`kind` `photocraft-open-as-layers`, `version` 1, top layer first, 2 to 256 photos) and opens it in PhotoCraft as one document, one layer per photo (active photo on top); a PhotoCraft whose `Info.plist` doesn't list `pcstack` (the official one) gets the files one by one with a toast saying why; the command `photo.openAsLayersInPhotocraft` (`wait: true` for agents), a row in the activity stack with ✕.

## Branch

`feat/open-as-layers`

## Own files

`crates/engine/src/layers.rs` and `layers/tests.rs`, `crates/ui-egui/src/layers_task.rs` and `layers_task/tests.rs`

## Hooks in official files

`crates/engine/src/cmd/convert.rs` (`edit_options`, shared with Edit In), `lib.rs` (module), `open_path.rs` (`openable_stack`), ui-egui `lib.rs` + `app.rs` + `app/frame.rs` + `headless.rs` (`layers`), `services.rs` (`layers_support`), `menus.rs` + `menus/run.rs` + `menus/ui_commands.rs` (the command), `panels/grid.rs` + `panels/photo_menu/items.rs` (the submenu item), `panels/photo_menu.rs` (`NEEDS_SEVERAL`), `apps/lightcraft/src/editors.rs` (`supports_layers`) + `session.rs`, `apps/lightcraft-web/src/web.rs`, `docs/control-protocol.md`, `docs/parity-checklist.md`, `docs/whats-new.md`

## Tests

`cargo test -p lightcraft-engine --lib -- file_trash delete_local layers open_path`, `cargo test -p lightcraft-ui-egui --lib -- layers_task`, `cargo test -p lightcraft --bin lightcraft -- editors`
