# Remove: no wait on the source search or the thumbnail

List: a painted spot's source is found on a worker and the open photo's thumbnail renders once after a burst of spot edits, so a spot edit waits on one render; `perf/97-remove-fixed-cost`.

## What it does

- **Source search on a worker.** The Remove tool on the photo sends `spot.add` with `background: true`. The command returns at once and the search for the spot's source (`pick_source` on a 512-pixel copy, which may first be cut from a larger copy or decoded) runs on a worker. Each frame, `Session::poll_spot_sources` adds the spot with its source when the answer comes: one undo step, selected, in the order spots were asked for. The spot shows a frame or two after the click (the search takes a few ms). A failed search, or no worker, adds it without a source (the render chooses one).
- **Only in a window.** The window gives the session a waker (`SpotSources::set_waker`), and the worker calls it when it answers: the click is handled after that frame's poll, so without it the spot stayed unseen until some later redraw and a second click added another. With no window (scripts, the CLI, headless MCP) nothing polls, so `background` is ignored there and the search runs at once, returning the spot's index.
- **Order.** A spot that needs no search (an explicit source, Content-Aware), asked behind pending ones, waits its turn too (`{pending: true}`).
- **Drags.** While a drag is open on the photo, answers wait and are added after it ends, as their own undo step (they used to merge into the drag's). A spot asked during a drag that is cancelled is dropped with it; one asked before it is still added.
- **Undo and redo** first wait for the running searches and add their spots, so Undo right after a click undoes that spot.
- **Another library.** An answer asked in a library that has been closed since is dropped.
- Adding the spot at once and its source afterwards was tried first: every new spot then rendered twice (choosing its own source, then with the found one), which cost more than the search.
- **Thumbnail once per burst.** With the Remove panel open, the open photo's filmstrip thumbnail (a second full render after every edit) waits until its edits have paused for 0.4 s and the loupe has its render, then renders once (`edited_thumb`). A slider drag already held it back the same way. The hold ends for good once waited out (the window then stops waking for it), and another photo's thumbnail (an unedited raw next to it) leaves it alone.

Issue #97.

## Branch

`perf/97-remove-fixed-cost` (from `custom`: builds on #96, the spot rows and the update fix).

## Own files

`crates/engine/src/spot_source.rs`, `crates/engine/src/tests/spot_source.rs`, `crates/ui-egui/src/edited_thumb.rs`, `crates/ui-egui/src/tests/remove_fixed_cost.rs`

## Hooks in official files

- `crates/engine/src/lib.rs`: `mod spot_source`, `Session::spot_sources`; `end_interaction` and `cancel_interaction` tell it (`drag_ended`, `drag_cancelled`); `undo_step_with` and `redo_step_with` call `finish_spot_sources` first.
- `crates/engine/src/cmd/masks.rs`: `spot.add` takes `background` (only with a window: `spots_in_background`) and adds the rest in turn (`add_spot_in_turn`).
- `crates/ui-egui/src/panels/detail/retouch.rs`: the painted and keyboard spots send `background: true`.
- `crates/ui-egui/src/app/frame.rs`: gives the session its waker, polls the searches and wakes for the held thumbnail.
- `crates/ui-egui/src/panels/grid.rs` (`request_thumb`: the hold), `lib.rs` and `app.rs` (`edited_thumb` field, `mod`).
- `crates/ui-egui/src/headless.rs`: `busy()` counts pending searches; `settle_spot_sources()` (whether they landed).
- Tests that paint or key in a spot wait for it: `tests/canvas_keys.rs`, `canvas_pan.rs`, `masking.rs`, `space_hand.rs`, `tool_overlay.rs`, `spot_rows.rs`, `remove_lag.rs`; `tests.rs` and the engine's `tests.rs` (`mod` lines).

## Tests

`fork/tests.d/remove-fixed-cost.txt`. The engine tests hold a search on its worker and check the command answered first, then that the spot arrives with the source the old way finds, as one undo step, in order; that the waker is called; that without one the search runs at once; that an answer waits out a drag, is dropped with a cancelled one and after a library reopen; that Undo right after a click undoes the spot. `edited_thumb`'s unit tests check the hold ends, is not re-armed, and ignores other photos. The UI test counts the thumbnail requests in a burst of five spots (one, at the end; six without the hold).
