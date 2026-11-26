# Mask memory

List: masks, brushing and zoom hold each mask's alpha once, a zoom window's GPU stages go when the window leaves the screen, and `ui.inspect` → `memory` shows what each view's GPU stages hold, part by part (issue #402). Branch `perf/402-mask-memory`.

## What it does

- **Each mask's alpha is held once.** A GPU render evaluates its masks into one stack of planes (the buffer the finish stage reads). The alphas a view keeps for the next render (`masks::AlphaStore`) are now ranges of that stack (`Kept { stack, at, len }`), not a copy of each one: a render no longer holds every alpha twice while it runs, and a render that keeps nothing (the full-size picture, export) copies none out. One copy is left: a render with Decontaminate on a mask copies the whole stack once more to append the edges after it, because the finish stage reads both from one buffer. A brush part's plane is copied for keeping only when the render has a store (`brush_part(…, keep)`).
- **Earlier windows' alphas** (pan back, zoom back, issue #175) keep the same caps (`KEPT_ALPHAS`, `KEPT_ALPHA_BYTES` = 384 MB, the current render's alphas counted in), but an earlier render's stack is kept whole or not at all: it costs its whole size the first time one of its alphas is kept. A kept stack also holds planes no kept alpha points at: the chromaticity planes of local Moiré / Noise (3 planes), Relight's planes, and a second mask with the same alpha (1 plane each). Estimate from the plane sizes: on a 2560 × 2048 window a plane is 21 MB, so 21–84 MB a kept stack.
- **A zoom window's stages go with the idle trim.** When the idle trim runs and no zoom window is on screen (30 s without renders while a zoom window is kept at Fit, as its texture or, after an edit at Fit dropped the texture, as its stages only; 3 s otherwise), the `Region` and `RegionBefore` stage caches are dropped with the GPU copy of the original they are cut from. Before, they stayed until the next zoom (about 1 GB at 1:1 on a 33 MP raw with 12 masks). The loupe's own stages stay, for the next edit.
- **Diagnostics.** `ui.inspect` → `memory.stageCaches.views` lists each view's GPU stages by part (`images`, `planes`, `source`, `alphas`, `otherAlphas`, `brushes`, `host`, `other`; `GpuStages::parts`, `lightcraft_gpu::stage_parts`), and `memory.gpu.peak` is the most device bytes allocated at once since launch.

## How it was measured

Release builds on a test Mac, one run each, other lanes building. The app ran with `--control`, a fresh profile and library, the window at 1600 × 1000 points (Retina), on the corpus A7 V raw (`arw-sony-ilce-7m5.arw`, 7008 × 4672). A script added 12 masks (5 brush masks of 4 strokes each with an erase, 2 linear, 2 radial, a luminance range, a colour range, Sky), zoomed to 100 % and 200 % with pans and strokes there; then 40 pans with a stroke each; then mask slider ticks, effect changes and 5 new masks at Fit and at 100 %; 60 s idle after each round. Strokes went through `mask.brushStroke` (the control channel's pointer drags did not paint in a window without focus). SAM 3 was kept out (`LIGHTCRAFT_SAM3_DIR` set to an empty folder): with the Masking panel open it loads at the first pause and adds about 2.2 GB (Core ML surfaces about 1 GB, its GPU buffers about 1.2 GB) to every reading, at a time that varies from run to run. The footprint is `footprint -p`, sampled every second.

| | Before | After |
| --- | ---: | ---: |
| Highest footprint, whole session | 8.9 GB | 7.4 GB |
| Highest GPU buffers at once (the full-size picture's render, 17 masks) | 6.7 GB | 5.2 GB |
| After the zoom round, 60 s idle | 2.68 GB | 1.78 GB |
| After the pans round, 60 s idle | 2.72 GB | 1.78 GB |
| GPU buffers after 60 s idle | 1.49–1.52 GB | 0.58 GB |
| Mask slider tick, render, median (Fit / 100 % brush / 100 % linear) | 69 / 46 / 54 ms | 44 / 41 / 55 ms |

What the views held at 200 % (before): the zoom window's stages 0.9–1.0 GB (alphas 222, earlier windows' alphas 148, brush parts 89, images 281, planes 187 MB) and the loupe's 0.53 GB (alphas 166, brush parts 66, images 216, planes 83 MB), the GPU copy of the original 374 MB, the CPU original 374 MB, preview sources 200 MB.

## Not done

- The full-size picture (`ui-egui/src/full_picture.rs`) renders the whole 33 MP frame at once after each pause past Fit, and every mask's alpha is a full-size plane in it (4 bytes a pixel a mask: 132 MB each, 2.2 GB for 17). That render is the remaining peak (5.2 GB of GPU buffers). Evaluating masks and the finish stage band by band would bound it.
- SAM 3 (about 2.2 GB) stays loaded while the Masking panel is open in the front window (issue #353); BiRefNet Matting runs ahead on its own on some photos: on one private A7 V photo it loaded during the session and the app held about 3 GB more GPU-mapped memory while it stayed loaded.

## Branch

`perf/402-mask-memory`

## Own files

`fork/mask-memory.md`, `fork/tests.d/mask-memory.txt`

## Hooks in official files

`crates/gpu/src/masks.rs` (`Kept`, `AlphaPart`, `with_earlier`, `brush_part`'s `keep`, `masks` returns the stack as `Arc<Buf>`, a test), `crates/gpu/src/render.rs` (`StageParts`, `GpuStages::parts`, `Cx::copy_range`, `masks.as_deref()`), `crates/gpu/src/decontam.rs` (`with_edges` takes and returns `Arc<Buf>`), `crates/gpu/src/lib.rs` (`stage_parts`, `GpuMemory::peak`), `crates/gpu/src/ctx.rs` (`HIGH`), `crates/engine/src/memory.rs` (`GpuUsage::peak`), `crates/ui-egui/src/render.rs` (`trim_when_idle`, `memory`), `docs/control-protocol.md`.

## Tests

`fork/tests.d/mask-memory.txt`: a render's kept alphas share its one stack, after a slider tick too (`masks::tests`); a zoom window's stage caches go with the idle trim and the loupe's stay (`render::stage_budget_tests`).
