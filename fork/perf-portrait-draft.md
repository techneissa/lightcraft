# Performance: Portrait slider drags

List: a Portrait slider tick (every face feature but Face Reshape) redoes only the retouch step and what follows, and on the GPU only around the face; the colour noise model is no longer measured again on every tick. Branch `perf/portrait-draft`.

## What it does

With any Portrait setting on (Face Reshape alone aside, `fork/retouch-reshape.md`), the scene-linear stage runs on the
CPU, because the retouch step does (`crates/pipeline/src/retouch_step.rs`). Before this change every tick of a Portrait
slider redid all of it and everything after it:

- **The colour noise model, every tick.** The joint colour noise reduction (process V6, on for raws) measures its noise
  model on fixed tiles of the frame (`colour_noise`), which leave the retouching out, but its cache key held the
  retouch settings. A tick measured it again: 30–170 ms on a 1920 × 1280 draft on a test Mac, more than the rest of the
  render together. The key (`Plan::noise_key`) now leaves the retouching out.
- **The image before the step.** The GPU read the resampled frame back, ran white balance, defringe, spots and red eye on
  the CPU, then the step, and uploaded the frame. The image before the step is now kept, keyed by
  `Plan::pre_retouch_key` (`Plan::pre_nr_key` without the retouching): on the CPU in the `StageCache` entry, on the GPU
  on the host and on the device (`crates/gpu/src/pre_retouch.rs`). A tick copies it, runs the step, and uploads only
  the rows of the boxes the step may write.
- **Noise reduction, planes and the per-pixel stage over the whole frame.** On the GPU the step's boxes
  (`retouch_step::write_boxes`: per face, what retouching reads for it, the face's box and its features' masks grown by
  their reach) are the dirty rectangles of issue #111's tiles (`fork/dirty-render.md`): noise reduction, the planes and
  the per-pixel stage are redone only in the tiles they reach, with the bits of a whole render. The dirty path starts
  from the last render when nothing but the spots and the retouching changed (`dirty::Rest` leaves the retouch
  settings out; Face Reshape and the window the retouching reads are in the plan's frame, size and window). A
  picture-wide feature (Background) may write anywhere: the whole frame is redone.
- **One-tile frames.** A frame of at most 144 noise tiles' area (about 2.36 MP) measured the colour noise on the image
  itself, so the retouching changed every pixel's noise reduction and no tick could be redone in tiles. With
  retouching on, such a frame is measured on its tile as larger frames and zoomed windows are, without the retouching.
  This changes the pixels of renders with a Portrait setting on whose frame is one tile (by the noise model's
  difference); a render with every Portrait setting at 0 is byte for byte as before (`an_all_off_block_renders_byte_identical`).

Preview, zoomed window and export stay identical: the zoom tests of the retouch step and of each feature pass
unchanged, and every GPU tick in the tests is compared with a whole render pixel for pixel.

`LIGHTCRAFT_PROFILE=1` prints `retouch: <ms>` (the step), `gpu retouch on the CPU: before it … (made / kept), copy +
step …, upload <n> of <h> rows …` and `gpu denoise: <ms>` (a whole noise reduction).

## Measured

`cargo run --release -p lightcraft-engine --example portrait_drag_bench` (a synthetic face, its five part masks and
spots; the 2560 px preview, a 1920 × 1280 draft, a raw's sharpening and colour noise reduction, tone and clarity; warm
stage cache; median of 11 ticks). On a test Mac with other builds and GPU tests running (load 10–40), so the GPU
column swings by several times between runs; its lowest times are the best guide.

Stages of one Skin tick on the GPU, a half-length portrait, waited for (`LIGHTCRAFT_PROFILE`):

| Stage | Before | After |
|---|---|---|
| colour noise model | 37 ms (every tick) | kept |
| white balance … retouch, noise reduction (`wb/nr`) | 104 ms (whole frame) | 9.2 ms (step 2.9 ms, 324 of 1280 rows up, 100 tiles) |
| planes | 24.5 ms (whole) | 4.1 ms (208 of 600 tiles) |
| finish + readback, histogram | 9.3 ms | 2.9 ms |

Draft frame time per tick (ms): GPU, the lowest tick of three runs (best run's median in brackets); CPU, the
median of one run (its exposure row moved with the load, the change does not touch it).

| Feature | Portrait, GPU | Face-filling, GPU | Portrait, CPU | Face-filling, CPU |
|---|---|---|---|---|
| none (exposure) | 2.8 → 2.5 | 2.1 → 2.3 | 46 → 23 | 49 → 35 |
| Skin | 75 → 13.6 (15.0) | 72 → 46 | 214 → 68 | 243 → 110 |
| Eyes | 61 → 16.9 (20.6) | 92 → 44 | 175 → 66 | 220 → 106 |
| Skin Tone | 79 → 12.1 (14.9) | 69 → 40 | 169 → 86 | 165 → 82 |
| Teeth and Lips | 62 → 12.3 (16.3) | 90 → 33 | 175 → 95 | 158 → 76 |
| Blemishes | 68 → 12.5 (13.3) | 61 → 32 | 134 → 87 | 173 → 71 |

What is left on a face-filling frame: the face is most of the picture, so noise reduction and the planes are redone in
80 % of the tiles, and the step itself works on the whole face. Next: in a draft, add the step's change to the kept
denoised image instead of denoising it again (as noise reduction drafts reuse the wide planes); the CPU renderer still
redoes noise reduction and the planes whole.

## Branch

`perf/portrait-draft` (from `custom`, with Blemishes #140, Face Reshape #143, Wrinkles #139 and Stray Hair #146).

## Own files

`crates/gpu/src/pre_retouch.rs`, `crates/pipeline/src/tests_retouch_cache.rs`, `crates/gpu/tests/it/portrait_drag.rs`,
`crates/engine/examples/portrait_drag_bench.rs`.

## Hooks in official files

- `crates/pipeline/src/lib.rs`: `Plan::pre_retouch_key`; `pre_nr_key`, `lin_key` and `noise_key` built from it
  (`noise_key` without the retouching); `colour_noise` measures a one-tile frame with retouching on; `lin_cpu` is
  `lin_cpu_pre_retouch` and the step; `CacheEntry::pre_retouch` (counted in `StageCache::bytes`) and `render_impl`
  starts from it.
- `crates/pipeline/src/retouch_step.rs`: `pre_retouch_runs` (a test's counter), `write_boxes`, `picture_feature_on`, a
  `retouch` timing line.
- `crates/pipeline/src/spots_keep.rs`: `lin_cpu_pre_retouch`; `lin_cpu` adds the step.
- `crates/gpu/src/render.rs`: `Entry::pre_retouch` and `retouch_boxes` (counted in `GpuStages::bytes`); `linear_stage`
  takes the retouch path when the step is the only CPU work (`pre_retouch::linear`) and hands the boxes to the dirty
  path; `linear` no longer runs the step; a `gpu denoise` timing line.
- `crates/gpu/src/dirty.rs`: `Rest` leaves the retouch settings out.
- `crates/gpu/src/lib.rs`: `mod pre_retouch`. `crates/gpu/tests/it/main.rs`: `mod portrait_drag`.

## Tests

`fork/tests.d/perf-portrait-draft.txt`. The CPU test counts the runs of the stage before the step
(`pre_retouch_runs`): a tick of each feature's slider and an exposure change make none, a moved spot makes one; each
result equals a render without the cache. The GPU test counts those runs and the output tiles (`tiles_rendered`) per
tick, and compares each result with a whole render pixel for pixel; a second test checks every pixel the step changes
lies in its boxes for every slider of every face feature, whole frame and zoomed window. None measures time.
