# Remove without lag

List: Remove, Heal, Clone, Content-Aware and LaMa spots redraw only what an edit reaches (a new spot costs the same with 100 spots as with 1), and hundreds of spots no longer lose the GPU; `perf/82-remove-lag`.

## What it does

- **A spot edit redraws only what it reaches.** Each spot keeps its box as it was before it drew and after. Adding, moving, resizing or deleting a spot (or changing its feather, opacity or source) draws that spot and the later spots whose box or source reads pixels it changed. Later spots it overlaps are taken off and put back from their own results; earlier spots are never drawn again. The result is the same as drawing every spot.
- **What invalidates them**: a change before the spots (white balance, profile, defringe, the photo's crop or size) draws them all again; a change after them (tone, colour, masks, effects, sharpening, noise reduction) draws none.
- **LaMa spots** are a layer of their own, before the white balance, with the same patches: a LaMa spot asks LaMa (and its cache) only when it is new or changed, or reads what a changed spot drew. A fill a drag's draft could not give is asked again by the full render.
- **Content-Aware (PatchMatch)** spots fill only the spot an edit reaches.
- **Three sizes per view** are kept (the full size and a drag's two draft sizes), apart from the stage cache's two, so the full render after a drag still redraws only the changed spot.
- **The open photo's filmstrip thumbnail**, which every edit renders again, keeps its spots too (one photo at a time).
- **Zoomed in**, a new spot keeps the sharp window on screen until the window with the spot arrives (it used to drop to the softer whole frame for a moment, so the photo seemed to render again after every spot). A change of mask overlay or soft proof still drops it at once.
- **Never loses the GPU**: a render submits its GPU work at most 256 passes at a time. About 100 Heal or Remove spots in one submission went past Metal's 4096 command buffers, the device was lost and the app quit.
- `LIGHTCRAFT_PROFILE=1` prints, per render, how many spots it drew and why (`spots: 31 of 31 in 2560×1707: drew 1 (took off 0)`).

Issue #82.

## Branch

`perf/82-remove-lag` (from `custom`: it builds on LaMa fill and the Remove panel changes that are not on the official `main`).

## Own files

`crates/pipeline/src/spots_keep.rs`, `crates/pipeline/src/tests_spots_keep.rs`, `crates/gpu/src/spots_keep.rs`, `crates/gpu/tests/it/spot_cache.rs`, `crates/ui-egui/src/tests/remove_lag.rs`

## Hooks in official files

- `crates/pipeline/src/lib.rs`: `mod spots_keep`, a `spots` store on `StageCache` (its bytes, `clear`), `render_impl` calls `spots_keep::lin_cpu` when it has a cache, `native_long` made `pub(crate)`.
- `crates/pipeline/src/spots.rs`: one line counting each spot drawn.
- `crates/gpu/src/render.rs`: `Cx` counts passes and submits at `FLUSH_PASSES`; `GpuStages` keeps the spots in two stores (device and host); `linear` takes and returns them; `spots` became `draw_spot` (one spot); a test of the pass cap.
- `crates/gpu/src/ctx.rs`, `crates/gpu/src/wgsl/spot.wgsl`: the `spot_patch` kernel. `crates/gpu/src/lib.rs`: `mod spots_keep`. `crates/gpu/tests/it/main.rs`: `mod spot_cache`.
- `crates/ui-egui/src/render.rs`: the open photo's thumbnail stages (`main_photo`, `edited_thumb`), freed with the loupe's.
- `crates/ui-egui/src/region.rs` (`RegionView::view`, `stands_in`), `panels/detail/window.rs` (keep the window on screen while a new edit's renders), `panels/compare.rs` and `region/tests.rs` (the new field), `render/thumbnail_tests.rs` (one test), `tests.rs` (`mod remove_lag`).
- Fork file `crates/pipeline/src/spots_learned.rs`: `apply` split into `apply_one` and `footprint` (same behaviour); `tests_learned_fill.rs`: one test.

## Tests

`fork/tests.d/remove-lag.txt`. They count the spots drawn (`spots_keep::spots_drawn`) and the fills asked per edit, and compare every incremental result with drawing every spot; none measures time.

## When another branch adds a spot flag

A spot that is hidden (or otherwise skipped) must be skipped in `spots_keep::entries` (and `spots_learned::footprint` for LaMa spots) and in the GPU's `draw_spot`, which replaced the GPU's loop over every spot. The CPU still draws each spot through `spots::apply_except`, so a skip there holds.
