# Masks and the finish stage per band

List: a render that keeps nothing (the full-size picture, export) evaluates its masks band by band of rows and finishes each band from them, so it holds one band's alphas instead of a full-size plane a mask. Issue #406, step 1 of `fork/memory-design.md` (#408). Branch `perf/406-masks-per-band`.

## What it does

- **Which renders.** A GPU render with no view stages (nothing kept for the next render), that reads no alpha back (no mask overlay, no mask planes asked for), whose alphas do not fit one band. The loupe, zoom windows, the brush live preview, overlays and mask planes render their masks whole, as before. Sampling, the linear stage and the planes stay whole (step 4, #411).
- **Band size** (`band_rows`). The app's band share (`crate::set_band_budget`, a quarter of the memory budget: 384 MB at the 1.5 GB budget) holds the band's buffer (4 bytes a pixel for each mask, Relight plane and, with local Moiré / Noise, 3 chromaticity planes) and one banded mask's evaluation: 17 scratch planes over the band and twice that mask's reach. The band's buffer and a cut's two-channel buffers also fit the device's largest buffer (128 MiB in a browser). Whole 16-row workgroups, at least 16 rows. The export below (7008 px wide, 12 alpha planes, no chromaticity, a colour range reaching 252 rows): bands of 192 rows, 25 bands. One buffer serves every band, cleared before each, with the decontaminated edges copied after it once.
- **Same bits.** A band's mask is evaluated over the band and the mask's reach around it, with the whole frame's coordinates (`masks::Cut`; the `shape` kernel's P[25] and the `stroke` kernel's P[1], P[2] give the cut's rows), the whole frame's filter radii, and box-filter runs that start on the whole frame's rows (`gaussian_at`). The finish kernel reads a band's alphas through `F_MASK_N` and `F_MASK_Y0`; a whole render sets them to the whole frame.
- **Each kind's reach** (`masks::bands::reach`): gradients, luminance range and brush strokes read only their own pixel (a stroke's pixel stays in the 16-row tile it has in the whole frame, so it picks the same dabs); colour range smooths with a cross guided filter, Refine Edges with another (`guided_reach`: per vertical box pass its radius and one run of the running sum, two Gaussians of three passes each). At 33 MP (from `guided_reach`'s formula) colour range reaches 252 rows, Refine Edges at 5 the same, at 50 1,606.
- **Evaluated once, whole, and copied into each band** (one full-size plane each, and its evaluation's scratch over the whole frame while it runs): a mask with a part the CPU evaluates over the whole frame (`masks::on_cpu`: Auto Mask strokes, stored AI selections, Sky, Subject and the other shapes without a kernel, a gradient too sharp for the warp's position grid, a stroke with more dabs than a device buffer holds), a decontaminated mask (its edge is estimated from its whole alpha), and a mask whose scratch over the band and its reach doesn't fit the share beside a 16-row band, or whose cut would not be shorter than the frame. At 33 MP and the 384 MB share that is Refine Edges at 9 or more (less on a colour range mask, whose smoothing adds its reach). The guide of stored selections and the Auto Mask planes are let go once these are evaluated. Relight's planes are already whole on the host; each band uploads its rows.
- **One band at a time.** The render waits for each band before the next (`Cx::sync`): without the wait every band was recorded before the device ran the first, and the buffers each band let go stayed held until then (the export below peaked at 9.6 GB footprint).

## Measured

On a test Mac, other builds running (load average 10 to 26), one run each: `lightcraft-cli render` of the corpus A7 V raw (7008 × 4672) to a full-size JPEG with 12 masks (5 brush masks of 4 strokes and an erase, 3 linear, 2 radial, a luminance range, a colour range), `LIGHTCRAFT_PROFILE=1`, `/usr/bin/time -l`. Before: `custom` at `9856fd773`.

| | Before | After | After, Refine Edges 20 on the colour range |
| --- | ---: | ---: | ---: |
| Peak footprint | 6.61 GB | 3.16 GB | 7.50 GB |
| Peak GPU buffers | 4,496 MB (masks stage) | 1,828 MB (noise reduction; finish stage 1,709 MB) | 3,247 MB (masks stage: the refined mask whole) |
| Masks + finish stages | 236 + 89 ms | 0 + 784 ms | 325 + 202 ms |
| `develop` / whole job (profile lines) | 822 / 1,383 ms | 1,542 / 2,266 ms | 1,134 / 1,878 ms |
| Whole run | 3.26 s | 3.66 s | 2.48 s |
| Output | | the same JPEG, byte for byte | |

The same `custom` binary took 1.91 s and 3.26 s in two runs a day apart: the whole run varies by more than a second here, mostly before the GPU render (the raw decode). The masks and finish stages are slower: the colour range is evaluated over its band and 252 rows each side (3.6 times its rows), in 25 bands that each wait for the device. The peak is now noise reduction's 1,828 MB (#409), as with no masks: 12 banded masks no longer raise it. A mask evaluated whole still does: Refine Edges at 20 holds its plane and its evaluation's scratch over the whole frame.

The app, once, after only (no run of `custom` on the same script): release build, `--control`, a fresh library and settings folder, SAM 3 kept out, the same raw and 12 masks; strokes through `mask.brushStroke`; 100 % and 200 % with 4 pans and a stroke each, 12 more pans at 100 % with a stroke each, 5 more masks (17), 6 mask slider ticks, 60 s idle at Fit; `footprint -p` every second. Highest footprint 7.4 GB (in the pans round), highest GPU buffers at once 4.7 GB (`ui.inspect` → `memory.gpu.peak`). After the idle: 5.0 GB footprint, of which GPU buffers 0.5 GB. PR #405 measured 7.4 GB and 5.2 GB on its own script, before #409; this run does not say which renders made the peak, nor what holds the idle footprint outside the GPU buffers.

## Branch

`perf/406-masks-per-band`

## Own files

`fork/mask-bands.md`, `fork/tests.d/mask-bands.txt`, `crates/gpu/src/masks/bands.rs`, `crates/gpu/src/masks/bands/tests.rs`

## Hooks in official files

`crates/gpu/src/masks.rs` (`Cut`, `Images`, `Target`, `evaluate` for one mask over a cut, `on_cpu`, `dabs_fit`, `masks` calls it, `guided_cross` and `stamp_stroke` take the cut, `refine_sigma`, `terms`), `crates/gpu/src/wgsl/mask.wgsl` (`shape`: P[25]; `stroke`: P[1], P[2]), `crates/gpu/src/wgsl/finish.wgsl` and `wgsl/decontam.wgsl` (`mask_at`, `chroma_at`; the unused `n` parameters gone), `crates/gpu/src/params.rs` (`MASK_N`, `MASK_Y0`), `crates/gpu/src/decontam.rs` (`found`, `Found`, `for_bands`; `attach` through them; the hooks' anchors), `crates/gpu/src/render.rs` (step 4 through `masks::bands::stage` and `edges`, step 5 through `per_pixel`; `Cx::clear_range`; `Cx::sync`'s comment), `crates/gpu/src/ctx.rs` (`Buf::held`; in tests, `thread_bytes`).

## Tests

`fork/tests.d/mask-bands.txt`: a frame in many bands gives the whole render's pixels with every mask kind; masks that reach far, in bands of about 320 rows, give the whole frame's alphas bit for bit; with 12 masks a render that keeps nothing holds, in every device buffer at once, not much more than with one.
