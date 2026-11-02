# LaMa fill

List: LaMa (big-lama, pure Rust on Candle) as a second fill for Remove and Content-Aware spots, chosen with the Remove panel's Fill picker, with a one-click download of its model. Branch `feat/47-lama-fill`.

## What it does

Each Remove and Content-Aware spot has a **fill**: Classic (the PatchMatch fill the app always had) or LaMa (a learned fill that rebuilds lines, facades and repeating textures where Classic leaves a grey smudge). Heal and Clone copy a source and ignore it.

- **Fill picker**: the Remove panel's heading has a **Fill** dropdown at its right (Classic | LaMa). It sets the fill of the spots made from now on; every spot keeps its own, so changing it never redraws old spots. Find People / Find Wires → **Remove** and **Remove All** use it too. Without the model the entry reads "LaMa (Download…)": it asks first (206 MB, from huggingface.co, into the folder shared with PhotoCraft) and only then downloads, with a row in the activity stack (✕ stops it; asking again resumes; size and SHA-256 are checked). While it downloads, the entry shows the percentage.
- **Model**: `big-lama.pt` in `~/Library/Application Support/ArtCraft/models/lama/` (shared with PhotoCraft; other systems: `<config>/models/lama`; `LIGHTCRAFT_LAMA_DIR` overrides). Copying the file there by hand works too. Never delete or move that file.
- **Stored with the spot**: `fill` (`"lama"`; Classic is not written) in the sidecar and catalog, so reopening and exports match. A build that does not know it, and a spot saved without it, reads as Classic.
- **Rendering**: LaMa spots are drawn first, before the white balance, so the fill does not depend on the white balance sliders; Classic spots after it, as before (a Classic spot that overlaps a LaMa one is drawn on top of it). LaMa sees the spot's box and a margin of one box side; a window of a render (the loupe) holds the same, so it matches the whole frame. The model runs on a worker thread of the render with a row "Filling with LaMa" (✕ stops it); the result is cached by the pixels it was made from, so slider drags and later renders ask again for free. Renders that follow a slider drag take a cached fill or none and start no run: until the full render has LaMa's fill the spot shows the Classic fill (the preview then updates by itself). Exports wait for the fill, so they match the preview. One run at a time; the model stays loaded for a minute.
- **Never crashes**: no model, a damaged file, a cancelled or failed run, a panic inside the run: the spot shows the Classic fill and the Remove panel says why (the last message under the Find buttons, while LaMa is in use).
- **Commands**: `fill` (`classic` | `lama`) on `spot.add` and `spot.update`, and on `spot.removeCandidates` (also `spot.findPeople` / `spot.findWires` with `add`). `lama` is refused for heal and clone spots and for anything but the two names; for a computer without the model it is kept (the answer carries a `note`). `lama.status` says whether the model is installed and how a download stands; `lama.download` starts it (asking the user first is the caller's job).
- The cache key of a photo with LaMa spots includes whether the model is there, so installing it redraws those photos.

Mac, Linux and Windows; not the web build (no model download there). Issue #47.

## Branch

`feat/47-lama-fill`

## Own files

`crates/lama/` (`checkpoint.rs`, `fft.rs`, `layers.rs`, `cl.rs`, `net.rs`, `region.rs`, `tests.rs`, `examples/compare.rs`), `crates/pipeline/src/learned_fill.rs`, `crates/pipeline/src/spots_learned.rs`, `crates/pipeline/src/tests_learned_fill.rs`, `crates/engine/src/lama_fill.rs`, `crates/engine/src/lama_fill/download.rs`, `crates/engine/src/cmd/lama.rs`, `crates/engine/src/tests/lama_fill.rs`, `crates/ui-egui/src/panels/right/remove_fill.rs`, `crates/ui-egui/src/tests/remove_fill.rs`

## Hooks in official files

`Cargo.toml` and `Cargo.lock` (the new crate and `zip`), `crates/engine/Cargo.toml` (the dependency), `crates/develop/src/settings.rs` (`SpotFill` and `Spot::fill`), `crates/pipeline/src/lib.rs` (two `mod` lines, `Plan::draft`, the learned spots drawn first in `lin_cpu`, the draft flag in `pre_nr_key`), `crates/pipeline/src/spots.rs` (`apply_except`, two functions made `pub(crate)`, the window of a LaMa spot holds its margin), `crates/engine/src/lib.rs` (`mod lama_fill`, `Session::lama`), `crates/engine/src/cmd.rs` (`mod lama` and its specs), `crates/engine/src/cmd/masks.rs` (`fill` of `spot.add` / `spot.update`), `crates/engine/src/cmd/find_removals.rs` (`fill` of Remove / Remove All), `crates/engine/src/media/session.rs` (the cache salt), `apps/lightcraft/src/main.rs`, `apps/lightcraft-cli/src/main.rs` and `crates/mcp/src/headless.rs` (`with_default_lama_model()`), `crates/ui-egui/src/state.rs` (`remove_fill`), `panels/right.rs` (`mod remove_fill`), `panels/right/crop.rs` (the heading), `panels/right/remove_finds.rs` (the fill of Remove / Remove All and the notice), `panels/detail/retouch.rs` (the fill of a painted spot), the `mod` lines of `crates/engine/src/tests.rs` and `crates/ui-egui/src/tests.rs`, and the `Spot { .. }` literals of three older tests that now end in `..Default::default()`.

## Tests

`cargo test -p lightcraft-lama --lib`, `cargo test -p lightcraft-pipeline --lib -- learned_fill spots tests_window`, `cargo test -p lightcraft-engine --lib lama_fill`, `cargo test -p lightcraft-ui-egui --lib remove_fill`. None needs the model except one engine test that runs it when it is installed (it skips otherwise); the pipeline tests use a fake fill, the UI tests a fake model file.

## What's New entry

### LaMa Fill for Remove
- The Remove panel has a Fill choice, Classic or LaMa. LaMa is a learned fill that rebuilds lines, windows and textures where the Classic fill leaves a grey smudge, best on large or busy areas. It sets the fill for new spots; each spot keeps its own. Find People and Find Wires use it too.
- LaMa needs its 206 MB model: choose LaMa and LightCraft asks before it downloads it (or copy `big-lama.pt` into the models folder it shares with PhotoCraft). Without it, spots show the Classic fill.
