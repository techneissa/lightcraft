# Spots finished once, reused by every zoom and edit

List: once Remove, Heal, Clone, Content-Aware and LaMa spots are drawn, a later edit, zoom, pan or pinch level draws none of them again and asks LaMa nothing; `perf/176-remove`.

## What it does

- **Finished spots per scale** (`crates/pipeline/src/spots_scale.rs`). A zoomed-in render is a window of the whole frame at its scale. A window drawn with its spots keeps each spot's finished pixels (its box after every spot drew), in pixels of the whole frame, under a key of the whole frame and its size, the settings before the spots, the spots, and a fingerprint of the source (its size and 64 of its pixels), never the window. A spot is kept only when the window held everything it reads (`exact`). A later window of that scale whose spots are all kept does not grow for them and pastes them over its own white-balanced image (CPU: `spots_keep::lin_cpu_pre_retouch`; GPU: `spots_keep::spots` uploads them). Up to 6 scales, 128 MB (fills have their own 128 MB: 256 MB together at most).
- **Windows hold what their spots read.** A window grows to each touching spot's whole reach (`spots_scale::grow`, a few pixels past `window_for_reads`, which left out the bilinear margin of the source): a Content-Aware spot near a window's edge was up to 34 levels off the whole frame's.
- **LaMa fills for every scale.** Each LaMa fill is kept as the model gave it, keyed by the source's fingerprint, the geometry settings and the LaMa spots up to it, never the scale. A render of the spot at that scale or below (another window, a pinch level, the whole frame, a drag's draft) resamples it to its own crop and never asks the model; a finer scale asks once and replaces it. 128 MB.
- **Automatic sources found once.** A window picks a source for each Remove, Heal or Clone spot without one (on a 512-pixel copy of the photo): the pick is kept for the process (`spots_scale::picked`, 4,096 at most), keyed by the source's fingerprint, the settings that place it and the spot, so a pan or zoom searches nothing. A Content-Aware, shaped or LaMa spot reads no source and is never searched (`reads_source`).
- **Only windows keep finished spots** (a whole-frame render has no window to say which are exact; an empty list kept for its scale made later windows paste nothing), and a spot beside a LaMa spot whose fill has not come is not kept either (`exact` takes the spots that drew).
- **A LaMa fill shown smaller** (a finer render's fill at Fit) averages the fill pixels each pixel covers; one bilinear sample aliased textured fills.
- **One store for every view** (`spots_scale::shared`): the zoom's view, let go at Fit, finds its spots and fills again. `StageCache::isolated()` gives a test its own.
- **Content-Aware and shaped spots on the GPU.** A Content-Aware, lasso or object-shaped spot sent the whole photo to the CPU; now only the spot stage runs there (`spots::drawn_on_cpu`, as LaMa spots already did), and a window that pastes them stays on the device.
- **The colour noise model** is keyed by the source's fingerprint and the whole frame before the spots (not the spots, red eye or window), and a view keeps the last one (`NoiseMemo`): a spot edit or a pan no longer measures it again, and the next photo in the same view measures its own.
- `LIGHTCRAFT_PROFILE=1` prints `gpu spots: N of M in W×H: pasted the finished spots` for a window that pasted them.

## Not done

- **Temperature and Tint** draw no spot again since issue #184 (`fork/spot-order.md`), unless Settings asks for the old order.
- **A drag's first draft** draws every spot at the draft's size once (later drafts and drags reuse it; LaMa takes its kept fill).
- **A new scale** (a pinch level) draws its Classic spots once.

Issue #176.

## Own files

`crates/pipeline/src/spots_scale.rs`, `crates/pipeline/src/tests_spot_reuse.rs`, `crates/gpu/tests/it/spot_reuse.rs`, `fork/tests.d/spot-reuse.txt`

## Hooks in official files

- `crates/pipeline/src/lib.rs`: `mod spots_scale`; `plan_kept` (`plan` calls it without a store); the window branch pastes or grows (`window_halos` holds the halo code that was inline); `Plan::spots_at`; `StageCache::spot_finals`, `isolated`, `noise`; `noise_key` without the spots, red eye and window; `NoiseMemo`, `noise_measured`; `lin_cpu_pre_retouch` pastes.
- `crates/pipeline/src/spots.rs`: `drawn_on_cpu`; `MAX_SPOT_SPAN` is `pub(crate)`.
- `crates/pipeline/src/spots_keep.rs`: paste and capture in `lin_cpu_pre_retouch`; LaMa spots through `apply_one_kept`.
- `crates/pipeline/src/spots_learned.rs`: `apply_one_kept`; `fill_crop` also returns the model's fill.
- `crates/pipeline/src/retouch_step.rs`: the sub-plan's `spots_at`.
- `crates/gpu/src/lib.rs`: no whole-photo CPU fallback for Content-Aware and shaped spots. `render.rs`: `plan_kept`, the CPU spot stage for `drawn_on_cpu` spots unless pasting, `GpuStages::spot_finals` and `noise`. `dirty.rs`: `eligible` uses `drawn_on_cpu`. `spots_keep.rs`: paste and capture.
- Tests: `crates/pipeline/src/tests_learned_fill.rs` (one test), `crates/gpu/tests/it/shaped_spot.rs` (now asserts the GPU renders such photos), `crates/gpu/tests/it/main.rs`.

## Tests

`fork/tests.d/spot-reuse.txt`. They count spots drawn (`spots_keep::spots_drawn`), source searches (`spots::sources_picked`), windows that pasted (`spots_scale::windows_pasted`), LaMa fills asked (a fake fill) and noise models measured (`noise_measured`) over pans, pan backs, other scales, a view made again, another photo, Exposure and spot edits, and compare each window with the whole frame's render. None measures time.
