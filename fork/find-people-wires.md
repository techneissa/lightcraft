# Find People and Find Wires

## What it does

The Remove panel gets **Find People** and **Find Wires**. Each finds candidates on the photo and outlines them (people in blue, wires in amber); click one to select it (⇧ or ⌘ keeps the others selected), then **Remove** takes the selected ones out and **Remove All** takes every one, with content-aware spots (`Fill`, grown by 0.5 % of the long edge) in one undo step. Each result is an ordinary spot you can move, resize or delete. Finding changes nothing in the photo. **Clear** forgets the candidates. Mac only; nothing leaves the Mac. Issue #24.

Commands: `spot.findPeople {includeMain?, wait?, add?}`, `spot.findWires {sensitivity?, wait?, add?}`, `spot.candidates`, `spot.selectCandidate {id? | at?, toggle?}`, `spot.removeCandidates {ids? | all?}`, `spot.clearCandidates`. In the app they return at once (like AI masks); the CLI, MCP, tests and `wait: true` wait for the result.

How things are found:
- **People**: SAM 3's "person" mask (the model in the shared `ArtCraft/models/sam3` folder, see `shared-sam3.md`), one candidate per blob; blobs a few pixels apart are one person. Without the model, a rough body shape (head, shoulders, torso, legs) under each face the YuNet detector finds, marked source `faces`. The main subject (the largest share of the photo weighted by closeness to its centre, at least 1 % of the photo) is not proposed unless `includeMain` is set; it comes back with `main: true`.
- **Wires**: SAM 3's "power line" mask. Tried on three real photos (CC BY-SA, kept out of the repo): it found all five cables of a twilight photo, the pylon cables of a stormy-sky photo and the wires of a tangle across a building, though its masks are coarse (288 cells over the photo). The pure-Rust thin-line detector (`pipeline/src/find/wires.rs`: Hessian ridges at three scales, thinned centre lines, a quadratic fit, a check that the sides are alike and brighter) is the fallback when the model is missing or fails: it finds wires on clear sky (all five on the twilight photo) but not over busy backgrounds (nothing on the stormy photo, partial on the tangle).
- Spots: a person's mask becomes dabs on a grid sized to the mask's width; a wire path becomes dabs along it; at most 48 dabs per spot, so a long wire is several spots. Crossing wires and people who touch come as one candidate.

Never crashes: detection runs on its own thread (row "Finding people" / "Finding wires" in the activity stack, cancellable with ✕), a panic becomes an error and leaves the photo untouched; hostile masks, tiny and huge photos are tested.

## Branch

`feat/24-find-people-wires`

## Own files

`crates/pipeline/src/find/` (`mod.rs`, `shape.rs`, `cover.rs`, `people.rs`, `wires.rs`, `tests.rs`), `crates/engine/src/find_removals.rs`, `crates/engine/src/cmd/find_removals.rs`, `crates/engine/src/segment/request.rs`, `crates/engine/src/tests/find_removals.rs`, `crates/ui-egui/src/panels/right/remove_finds.rs`, `crates/ui-egui/src/panels/detail/find_overlay.rs`, `crates/ui-egui/src/tests/find_removals.rs`

## Hooks in official files

`crates/pipeline/src/lib.rs` (`pub mod find;`), `crates/engine/src/lib.rs` (`pub mod find_removals;`, the `finds` field of `Session`), `crates/engine/src/cmd.rs` (`mod find_removals;` and its specs), `crates/engine/src/segment.rs` (`pub(crate) mod request;`), `crates/ui-egui/src/panels/right.rs` (`mod remove_finds;`), `panels/right/crop.rs` (one call in `remove`), `panels/detail.rs` (`mod find_overlay;`), `panels/detail/retouch.rs` (one call in `remove_overlay`), `docs/parity-checklist.md` (LR-REM-DETECT), `docs/whats-new.md`, `crates/engine/src/tests.rs` and `crates/ui-egui/src/tests.rs` (a `mod` line each).

## Tests

`cargo test -p lightcraft-pipeline --lib find::`, `cargo test -p lightcraft-engine --lib --features sam tests::find_removals`, `cargo test -p lightcraft-ui-egui --lib find_removals`. None needs the SAM 3 model (its answers are faked); a person's removal is tested on a procedural wall, a wire's on a procedural sky.
