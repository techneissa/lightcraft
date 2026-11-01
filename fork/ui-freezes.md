# Performance: UI freezes

## What it does

Performance: UI freezes. Slow work leaves the UI thread so the window keeps drawing: Edit In renders its copies on a worker (an activity row with ✕), then adds and opens them; Edit In and Open as Layers give files to the editor on a worker (each macOS `open -a` waits), and the PhotoCraft lookup is reused for 10 s; the Keyboard Shortcuts window keeps its rows and clash table until the keymap, search or language changes; a customised photo menu builds its commands when it opens and after a change; the Profile menu lists the camera profiles folder every 2 s; Apply macOS Colour to All Photos counts and moves on a worker (an activity row; one undo step; a photo edited meanwhile keeps its edit). `photo.editExternal`, `profiles.macosForCamera*` and `wait: true` behave as before.

## Branch

`perf/ui-freezes`

## Own files

`crates/engine/src/edit_copies.rs`, `crates/engine/src/macos_colour_job.rs`, `crates/ui-egui/src/edit_in_task.rs` and its `tests.rs`, `crates/ui-egui/src/editor_handoff.rs`, `crates/ui-egui/src/macos_colour/job.rs`, `panels/photo_menu/cached.rs`, `panels/camera_colour/listing.rs`

## Hooks in official files

engine `cmd/convert.rs` (`edit_external` runs `edit_copies`), `cmd/macos_colour.rs` (`plan` split into `candidates` + `plan_of`), `import.rs` (`MacosGate`), `lib.rs` (modules); UI `menus/files.rs` (`edit_in` starts the task, `open_copies`), `layers_task.rs` (`finish` hands off), `keybinds/window.rs` (`listing`), `panels/photo_menu.rs`, `panels/camera_colour.rs`, `macos_colour/picker.rs` (`ask` / `asked`), `dialogs/confirm.rs`, `services.rs` (`open_with_shared`), `lib.rs`, `app.rs`, `app/frame.rs` (polls), `headless.rs` (`busy`), `menus.rs`; app `session.rs`, `editors.rs` (lookup cache)

## Tests

`edit_in_task`, `editor_handoff`, `editors::`, `keybinds::window`, `photo_menu::cached`, `camera_colour::listing`, `macos_colour_job`, `macos_colour::job`
