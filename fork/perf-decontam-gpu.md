# Decontaminated mask edges render on the GPU

List: a mask slider drag with Decontaminate on stays on the GPU: the edge's two colours are estimated once, kept on the device, and edited apart in the finish kernel; `perf/decontam-gpu`.

## What it does

- **Before**: a render with a decontaminated mask (`fork/decontaminate.md`) fell back to the CPU for the whole photo. The split was already cached (prepare 0–1 ms a tick), but the CPU finish stage ran the per-pixel decontamination at every edge pixel: 220–340 ms a draft tick on the synthetic portrait below, against 10–30 ms on the GPU without it.
- **Now**: the GPU render keeps it. After the masks, `crates/gpu/src/decontam.rs` (`attach`) takes each decontaminated mask's edge (`decontam::applies`): from the view's kept edges (`Slot`s, keyed as the CPU's stage cache keys them, `decontam::split_key`, plus the render's quality and process), or estimated once with the CPU's own estimate (`decontam::estimate_counted`) from the linear image and the mask's alpha read back from the device, then uploaded (`Edge::device_words`: the row starts, then 7 words a pixel). A drag of the mask's sliders, its Amount or a global slider neither estimates nor uploads it again. Each render copies the kept edges after the masks buffer's planes (the finish kernel has no storage binding to spare: every supported device has 10) and describes them in `aux` (`DC_N`, `DC_OFF`: mask, Amount, offsets, which edits are made apart).
- **Kernel**: `wgsl/decontam.wgsl` is `decontam::pixel` ported: each hit's scene edits made to the selected colour with the mask wholly on and to the unselected one wholly off, the tone stage once on the composite, the colour edits on each colour's display colour apart. `finish.wgsl` holds the stages as functions (`local_terms`, `scene_pixel`, `tone_delta`, `to_display`, `colour_pixel`, `recolor_masks_over`) for it to call.
- **Byte for byte without it**: `main` is unchanged. A render with an edge runs `main_dc`, a copy of `main` with three hooks made when the kernels are built (`decontam::with_main_dc`). Hooks in `main` itself, or `main` split into the functions, changed the GPU compiler's rounding on some pixels of renders with no decontaminated mask (a mask with every slider moved); with a second entry point, renders without one are the same bytes as before (checked: four mask cases, a synthetic portrait and a raw, full and draft).
- **Matches the CPU**: within one 8-bit level, on a rendered photo, a raw with Contrast, colour edits only and a zoomed window (`crates/gpu/tests/it/decontam.rs`); 11–23 pixels of 154 k differ by one level.
- **Kept edges belong to the photo.** The key holds no source pixels or mattes, so the kept edges also remember the source (by identity, weak) and the segmentation mattes they were estimated on, as `AlphaStore` does: another photo of the same size and settings, or other mattes, gets its own edge. Edges are freed when no mask is decontaminated. The lock is not held while an edge is estimated.
- **Portrait and spot ticks.** The retouch step changes the image, so `lin_key` changes. A kept edge is still used when the key without the retouching (`Plan::edge_lin_key`) is the same, no box the retouch step wrote (in the estimated render or this one, `pre_retouch::changed`), grown by the estimate's reach plus a noise-reduction margin, touches a 32 px square that holds an edge pixel (`Footprint`), and the mask's alpha in the squares within reach of the edge hashes the same as when it was estimated (an AI mask's alpha is guided by the photo, so it is re-evaluated on every tick and may change near the face). Otherwise the edge is estimated again (as the CPU does on every tick).
- **Tiles**: a render that estimated an edge again draws whole (the edge's colours reach 4.8 % of the long side past any tile a spot redrew); one that reused every edge may start from the last render as before.
- **Limits**: an edge over the device's buffer size (or a count over 32 bits) fails the GPU render with `Limit`, and the CPU renders it (test: `an_edge_over_the_buffer_limit_falls_back_to_the_cpu`). A missing `DC_N` / `DC_OFF` field logs one warning and the render draws no decontamination.
- **Memory**: 28 bytes an edge pixel on the device, counted in `GpuStages::bytes` and dropped with the stages.

Measured (a test Mac, release, `crates/engine/examples/decontam_drag_bench.rs`, 1920 × 1280 draft from a 2560 px source, warm stage cache, the Background mask recoloured and its Exposure dragged; other builds running, so medians):

| | Decontaminate 0 | Decontaminate 100, before | Decontaminate 100, after |
|---|---|---|---|
| Synthetic portrait (strands over 48 % of the frame), GPU | 20–23 ms | 382 ms (CPU fallback) | 17.5 ms |
| Synthetic portrait, CPU | 120–170 ms | 300–320 ms | 300–320 ms |
| A raw (Background mask from the Subject model), GPU | 5–13 ms | 102 ms (CPU fallback) | 6.0 ms |
| A raw, CPU | 85–98 ms | 85–99 ms | 85–99 ms |

A Portrait tick with a decontaminated mask (`portrait_drag_bench` with `DC=100`, a Background mask whose edge is a thin ellipse around the face; `DC=0` is the same mask without Decontaminate; draft 1920 × 1280 from 2560 px, GPU, `finish + readback` of a Skin tick from `LIGHTCRAFT_PROFILE=1`; the Mac was busy with other builds, load about 40, so read the gaps, not the absolute times):

| | Decontaminate 0 | Decontaminate 100 |
|---|---|---|
| Edge hugging the face (the retouch reaches it): re-estimated every tick | 4–9 ms | 42 ms (+35 ms a tick) |
| Edge far from the face (`DC_EDGE=0.88`): kept | same as with 0 | same as with 0 (no estimate) |

The +35 ms is the readback of the image and the alpha, the CPU estimate and the upload; the CPU path pays the same estimate on every tick. A matte soft over the whole frame (every pixel an edge pixel) costs 130–150 ms there. A Portrait or spot tick also draws the whole frame when an edge was estimated again (no dirty-tile start).

The first render with an edge reads back the linear image and the alpha and estimates it (as the CPU's first render does). The CPU path is unchanged.

## Branch

`perf/decontam-gpu` (from `custom`)

## Own files

`crates/gpu/src/decontam.rs`, `crates/gpu/src/wgsl/decontam.wgsl`, `crates/gpu/tests/it/decontam.rs`, `crates/engine/examples/decontam_drag_bench.rs`, `crates/engine/examples/portrait_drag_bench.rs` (the `DC` and `DC_EDGE` variables), `fork/perf-decontam-gpu.md`, `fork/tests.d/perf-decontam-gpu.txt`

## Hooks in official files

- `crates/gpu/src/wgsl/finish.wgsl`: the stage functions and `recolor_masks_over` before `main` (`main` unchanged).
- `crates/gpu/src/ctx.rs`: the finish module's `main_dc` entry and `module_source`'s call to `decontam::with_main_dc`; the `DC_*` constants.
- `crates/gpu/src/params.rs`: fields `DC_N`, `DC_OFF`.
- `crates/gpu/src/render.rs`: `GpuStages::decontam` (counted, cleared); in `render`, `decontam::with_edges` (with the source and the retouch boxes) after the masks, `decontam::describe` after `finish_block`, `main_dc` when an edge is attached, and no start from the last render when an edge was estimated again.
- `crates/gpu/src/lib.rs`: `mod decontam`, `edge_uploads`; the CPU fallback for decontaminated masks in `run` is gone.
- `crates/gpu/tests/it/main.rs`: `mod decontam`.
- Fork files of `fork/decontaminate.md`: `crates/pipeline/src/decontam/mod.rs` (`Edge::device_words`, `PX_WORDS`, `split_key`, `layers_of`, `estimate_counted`); `crates/pipeline/src/lib.rs` keys the CPU's edges with `decontam::split_key` and holds `Plan::edge_lin_key`; `Edge::occupied` in `decontam/mod.rs`. `crates/gpu/tests/it/portrait_drag.rs`: `portrait`, `edit` and `whole` are `pub(super)` for the decontamination test.

## Tests

`cargo test -p lightcraft-gpu --test it -- decontam` (photo switch, draft then full, two masks, the limit path, a zoomed window against a crop of the whole render, a Portrait tick keeping or re-estimating an edge) · `cargo test -p lightcraft-gpu --lib -- decontam ctx::tests::every_kernel` · `cargo test -p lightcraft-pipeline --lib -- decontam tests_window::a_decontaminated` (also in `fork/tests.d/perf-decontam-gpu.txt`). Timing: `cargo run --release -p lightcraft-engine --example decontam_drag_bench` (`--features sam -- PHOTO` for a photo, `HASHES=1` for the hashes of renders without decontamination, `SAVE=DIR` for pictures).

## What's New entry

### Faster Drags with Decontaminate
- A mask with Decontaminate on now renders on the graphics card, so dragging its sliders stays smooth.
