# Remove: no wait on the source search or the thumbnail

List: a painted spot's source is found on a worker and the open photo's thumbnail renders once after a burst of spot edits, so a spot edit waits on one render; `perf/97-remove-fixed-cost`.

## What it does

- **Source search on a worker.** The Remove tool on the photo sends `spot.add` with `background: true`. The command returns at once and the search for the spot's source (`pick_source` on a 512-pixel copy, which may first be cut from a larger copy or decoded) runs on a worker. Each frame, `Session::poll_spot_sources` adds the spot with its source when the answer comes: one undo step, selected, in the order spots were asked for. The spot shows a frame or two after the click (the search takes a few ms). A failed search, or no worker, adds it without a source (the render chooses one). `spot.add` without the flag (scripts, the CLI, MCP) answers as before, with the spot's index.
- Adding the spot at once and its source afterwards was tried first: every new spot then rendered twice (choosing its own source, then with the found one), which cost more than the search.
- **Thumbnail once per burst.** With the Remove panel open, the open photo's filmstrip thumbnail (a second full render after every edit) waits until its edits have paused for 0.4 s and the loupe has its render, then renders once (`edited_thumb`). A slider drag already held it back the same way.

Issue #97.

## Branch

`perf/97-remove-fixed-cost` (from `custom`: builds on #96, the spot rows and the update fix).

## Own files

`crates/engine/src/spot_source.rs`, `crates/engine/src/tests/spot_source.rs`, `crates/ui-egui/src/edited_thumb.rs`, `crates/ui-egui/src/tests/remove_fixed_cost.rs`

## Hooks in official files

- `crates/engine/src/lib.rs`: `mod spot_source`, `Session::spot_sources`.
- `crates/engine/src/cmd/masks.rs`: `spot.add` takes `background` (and uses `Session::add_spot`).
- `crates/ui-egui/src/panels/detail/retouch.rs`: the painted and keyboard spots send `background: true`.
- `crates/ui-egui/src/app/frame.rs`: polls the searches and wakes for the held thumbnail.
- `crates/ui-egui/src/panels/grid.rs` (`request_thumb`: the hold), `lib.rs` and `app.rs` (`edited_thumb` field, `mod`).
- `crates/ui-egui/src/headless.rs`: `busy()` counts pending searches; `settle_spot_sources()`.
- Tests that paint or key in a spot wait for it: `tests/canvas_keys.rs`, `canvas_pan.rs`, `masking.rs`, `space_hand.rs`, `tool_overlay.rs`, `spot_rows.rs`, `remove_lag.rs`; `tests.rs` and the engine's `tests.rs` (`mod` lines).

## Tests

`fork/tests.d/remove-fixed-cost.txt`. The engine tests hold a search on its worker and check the command answered first, then that the spot arrives with the source the old way finds, as one undo step, in order. The UI test counts the thumbnail requests in a burst of five spots (one, at the end; six without the hold).
