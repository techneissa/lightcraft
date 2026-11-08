# An edit renders only the tiles it changed

List: a Remove, Heal or Clone edit recomputes only the 64 px tiles it reaches through noise reduction, the planes and the per-pixel stage on the GPU, with a whole render's pixels; `perf/111-dirty-render`.

## What it does

- **Tiles.** Each view keeps, per output size, every stage of its last GPU render (the resampled source, the linear image, its planes) and now its finished 8-bit output. A spot edit takes the boxes `spots_keep` rewrote (spots taken off, drawn, put back) as the linear image's dirty tiles (64 × 64 px). Each later stage grows them by its reach and recomputes only its dirty tiles, from a cut of its kept input with that margin, into a copy of its kept output. The per-pixel stage runs on the union of the planes' dirty tiles.
- **Reach at a 2560 px loupe**: noise reduction 5–45 px (process V6's joint colour noise reduction, on by default for raws: about 170 px, its band halo on every side, with the whole frame's noise model), texture and sharpening 5–30 px, Highlights / Shadows about 250 px, Clarity about 200 px, Dehaze about 150 px across and 460 px down, local Moiré / Noise about 90 px. In the app, a Heal spot on a 33 MP A7 V raw at a 2560 × 1707 loupe recomputed 64–100 of 1080 tiles (6–9 %); the tests' scene with every plane on and the joint colour noise reduction, 224–324 (21–30 %, a move counts its old place and its new one; an undo that brings two spots back, 512).
- **Same bits as a whole render.** Every filter is local (box blurs, medians, resampling taps, pointwise kernels). The vertical box blur starts its runs on the whole image's rows (`box_v` takes the cut's first row) and sums each window directly for radii up to 7 (`BOX_DIRECT_MAX`: a run of 16 rows tripled the reach on the guided filters' small grids); a cut is resampled with the whole image's taps, sliced. Tests compare every incremental result with a whole render pixel for pixel.
- **Dropped work still fails.** A render that starts from the last output first clears its dirty rectangles (`rect_clear`), so a dispatch that never runs there leaves alpha 0, which the readback and display checks catch; the fault test `Fault::DropWork` may start from the last render for that reason.
- **What renders whole** (and is kept for the next edit): anything but the spots changed (the settings without the spots, the request, the source info, the plan's eyes, upright, recolor references, window and size: `dirty::Rest`); the spot redo did not start from the image the kept linear stage was made from; the airlight of Dehaze moved (only the per-pixel stage); the joint colour noise reduction measuring its noise on the image itself (a render of the whole frame that is one noise tile), which a spot changes (only the linear stage); a float render (`F32Hdr`); and in stage 1, masks that read the image (ranges, Sky and Subject heuristics, Auto Mask, Refine Edges), red eye, LaMa spots and the camera's local tone map.
- **The CPU renderer** keeps whole renders.
- **Memory**: the kept output is 4 bytes a pixel per size (17 MB at 2560 × 1707), counted in the stage cache's bytes, so the trim frees it with the rest (the local Moiré / Noise chromaticity plane, 12 bytes a pixel, is counted too).
- `LIGHTCRAFT_PROFILE=1` prints `gpu dirty: <why>` and `gpu tiles: <n> of <all>` per render.
- **Measured** (release build, one A7 V raw, 5 Heal spots, a move and a resize, app open throughout, load average 32–50 from other builds): the GPU render's stages, each waited for (`LIGHTCRAFT_PROFILE`), went from a median of 105 ms (60–222) to 56 ms (28–93). Release-to-result did not change beyond the noise (clicks: median 160 ms before, 171 ms after), because most of each loupe job is now CPU work after the GPU render (about 80–120 ms under that load), not measured stage by stage here.
- **Not yet (stage 2)**: the readback of only the dirty tiles; the loupe uploading only them (at rest it gets a Lanczos-fitted copy, converted for the display, so the fit and the conversion must follow the tiles first; `lightcraft_gpu::take_frame_note` already says which output a render started from and what it changed); dirty tiles through masks that read the image, red eye and the LaMa layer; the CPU renderer; drag drafts.

Issue #111.

## Branch

`perf/111-dirty-render` (from `custom`: builds on #96, the incremental spots).

## Own files

`crates/gpu/src/dirty.rs`, `crates/gpu/src/dirty/cut.rs`, `crates/gpu/src/wgsl/dirty.wgsl`, `crates/gpu/tests/it/dirty_render.rs`

## Hooks in official files

- `crates/gpu/src/render.rs`: `linear_stage` returns a `Linear` (with custom's `colour_noise`); `joint_halo`, `joint_band` and `nrcj` are `pub(crate)` for the dirty joint colour noise reduction; `GpuStages::bytes` counts `Planes.chroma`; `Entry` keeps the spots image its linear image came from (`post`) and the last render (`last`); `render` starts from it (linear stage, planes, the per-pixel stage over the dirty rectangles with `X0`) and keeps it on success; `linear` returns the image before noise reduction and what the spots redrew; `box_pass` takes the cut's first row and `box_chunk`; `gaussian_at`, `guided_at`, `guided_coeffs_at`, `resize_with`, `pack_taps` (the whole-image functions call them); `Planes` and `Prep` fields `pub(crate)`.
- `crates/gpu/src/spots_keep.rs`: `spots` also returns the boxes it rewrote.
- `crates/gpu/src/wgsl/blur.wgsl` (`box_v`: P[5], the first row), `wgsl/finish.wgsl` and `params.rs` (`X0`), `ctx.rs` (the `rect_copy` module).
- `crates/gpu/src/lib.rs`: `mod dirty`, its exports, the frame note cleared after a failed render.
- `crates/gpu/tests/it/main.rs`: `mod dirty_render`.
- `crates/gpu/tests/it/fallback.rs` (`dropped_work_in_an_incremental_render_falls_back`) and `mask_cache.rs` (`the_kept_chroma_plane_counts_with_the_stages`): a test each.

## Tests

`fork/tests.d/dirty-render.txt`. They count the tiles each edit's per-pixel stage computes (`lightcraft_gpu::tiles_rendered`) after add, move, resize, hide, delete, undo, the same settings again and a global slider, in the fit view and in a zoomed window, and compare every result with a whole render pixel for pixel; one compares with the CPU within the GPU / CPU bounds. None measures time.
