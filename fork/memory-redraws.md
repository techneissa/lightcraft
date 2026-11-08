# Performance: memory and redraws

## What it does

Performance: memory and redraws: the loupe never draws another photo's pixels while a raw renders, nor a stand-in of another look (the camera JPEG, an older edit): only a cached render or thumbnail of the photo's current look, else a plain frame saying "Rendering…"; Auto Import lists its folder only when the folder's time changed (a full listing at most once a minute, every 3 s while a new file's size is being confirmed) and builds the library's path set only when a file is new; the loupe's stage cache (about 250 MB) is freed 5 s after the loupe closes; the grid's thumbnail memo maps are pruned with the visible set; a finished render wakes the window (pending renders poll at 100 ms, not 60 Hz); a toast sleeps until its fade; `ui.inspect` → `perf` has `lastDraftRenderMs` and `lastFullRenderMs`.

## Branch

`perf/memory-redraws`

## Own files

`crates/ui-egui/src/auto_import_watch.rs`, `crates/ui-egui/src/headless/tests/loupe_stand_in.rs` (one test rewritten, one added)

## Hooks in official files

`panels/detail.rs` (`draw_slot`: no held photo, stand-in only of the current look, placeholder; `loupe_drawn`), `render.rs` (`thumb_of_current_look`, `release_closed_loupe`, `pending_poll`, waker, memo pruning, draft / full times), `crates/preview/src/pool.rs` (`set_waker`), engine `media/session.rs` (`quick_view_key`), `cmd/library/import.rs` + `specs.rs` + `library.rs` (`poll_auto_import_folder`, `waiting`), `app/frame.rs` + `state.rs` + `lib.rs` (the Auto Import poll), `panels.rs` (`toast_wake`), `control.rs`, `menus/run.rs`, `docs/control-protocol.md`

## Tests

`loupe_stand_in`, `auto_import`, `render::`, `thumbnail_memos`, `toast`, `lightcraft-preview` (`waker`)

Two official tests in `render/thumbnail_tests.rs` are adapted (`// fork:`), because the look-ahead's stand-in is the raw's
thumbnail render, never the camera JPEG: `a_running_stand_in_the_loupe_waits_for_keeps_its_slot` holds back the file read
instead of the preview read, and `a_stand_in_made_ahead_for_a_smaller_loupe_is_not_taken_over` asks for camera-JPEG
stand-ins, the only ones with a size.
