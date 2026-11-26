# Memory design: edit a 33 MP raw with a dozen masks on an 8 GB Mac

Issue #408. A design note: where the memory goes today, the target per state, the structure that meets it, the risks, and the ordered steps (each its own issue). Branch `docs/408-memory-design`.

Units: MB = 2^20 bytes. The test photo is the corpus Sony A7 V raw, 7008 × 4672 = 32.7 MP (N). Per pixel of N: 1 byte = 31 MB, 2 bytes = 62 MB, 4 bytes = 125 MB (an f32 plane or alpha; #405 wrote 132 MB, in decimal), 12 bytes = 375 MB (f32 RGB).

## 1. Where the memory goes today

Source of each number: *measured* in [PR #405](https://github.com/techneissa/lightcraft/pull/405) (release build, test Mac, 1600 × 1000 pt Retina window, 12 masks, `footprint -p`, SAM 3 kept out), *measured here* (section 1.1), *read* (file), or *estimate* (what it rests on).

What every state holds:

| Holder | Size at 33 MP | Source |
| --- | ---: | --- |
| CPU original, f32 RGB (`MediaCache`, `full_capacity` 1) | 374 MB | measured #405 |
| GPU copy of the original, f32 RGB (`shared_source`, for zoom windows) | 374 MB | measured #405 |
| Preview sources (2560 px level and others) | 200 MB | measured #405 |
| Loupe stages at Fit, 2560 px edge (alphas 166, brush parts 66, images 216, planes 83) | 0.55 GB | measured #405 (before) |
| Full-size picture textures (2048 px tiles, levels 1.41× apart, RGBA8) | about 250 MB | estimate: 4 B × N × (1 + ½ + ¼ …) |
| App with nothing open (UI, fonts, catalog, grid thumbnails, GPU device) | about 0.4 GB | estimate: the 1.78 GB rest footprint less the original, the previews, the 0.58 GB of GPU buffers and the picture textures |

| State | Footprint | What grows in it | Source |
| --- | ---: | --- | --- |
| Rest (60 s idle, Fit) | 1.78 GB | original 374, previews 200, loupe stages, picture textures; GPU buffers 0.58 GB | measured #405 (after) |
| Brushing at Fit | not measured alone | loupe stages, brush parts (one f32 plane per brush part, `BRUSH_ENTRIES` 4), the live preview's second render (`Slot::BrushFull`, a whole view's stages) | read |
| Zoom 100 % / 200 % | 2.7–3.0 GB | zoom window stages 0.9–1.0 GB (alphas 222, earlier windows' alphas 148, brush parts 89, images 281, planes 187 MB), the GPU original 374 MB | measured #405 (before; after: the window's stages go at idle) |
| Full-size render (past Fit, 1 s after each pause) | 7.4 GB peak | one render of the whole 33 MP frame: every stage's image and plane at full size, and every mask's alpha at full size (12 masks: 1.5 GB; 17: 2.1 GB); 5.2 GB of GPU buffers at once | measured #405 (after) |
| Export of this photo (12 masks) | 8.4 GB peak, alone | the same whole-frame render with every mask at full size, the noise reduction's 2 GB band, then the whole image encoded | measured here (1.1) |
| Making an AI mask | + 2.2 GB (SAM 3), + about 3 GB (BiRefNet Matting) | the models, kept while the Masking panel is open | measured #405, #407 |

The pattern: memory is photo size × (stages + masks), held once per view and once more by the full-size render. Nothing bounds it by what is on screen. The budget (`crates/engine/src/memory.rs`: a quarter of RAM, at most 1.5 GB) covers only part of it: the GPU original, the alpha store (its own 384 MB cap), the full-size render and export's renders sit outside it, and the open photo's view caches may run to 4× their share (`HARD_FACTOR`, `crates/ui-egui/src/render.rs`).

### 1.1 Export, measured here

`lightcraft-cli render` of the corpus A7 V raw to a full-size JPEG, release build of `custom` at `39735a977` (without #405), a test Mac with other builds running, one run each. Masks from a settings file: 5 brush masks (4 strokes and an erase each), 3 linear, 2 radial, a luminance range and a colour range. `/usr/bin/time -l` for the footprint, `LIGHTCRAFT_PROFILE=1` for the GPU buffers per stage.

| Masks | Peak footprint | Peak GPU buffers | Where the GPU peak is |
| ---: | ---: | ---: | --- |
| 0 | 5.2 GB | 2.6 GB | white balance and noise reduction (the joint colour noise reduction's bands) |
| 1 (brush) | 5.2 GB | 2.6 GB | the same |
| 12 | 8.4 GB | 6.2 GB | masks: 1.1 GB before them, 6.2 GB while they are evaluated (about 0.43 GB a mask) |

Two causes, both outside any budget:
- **Masks** hold their full-size alphas (twice on `custom`; #405 holds them once) and their brush parts and smoothing buffers at full size: about 3.4 full-size planes a mask.
- **The joint colour noise reduction** sizes its bands from the device's free memory, which no backend reports on a Mac, so it takes a fixed 2 GB (`crates/gpu/src/render.rs`, `color_nr_joint`): the whole 33 MP frame fits in one band of 1.9 GB. Without a known free size it should take its share of the memory budget.

An export alone, of one photo, can therefore pass the RAM of an 8 GB Mac.

## 2. Target per state, 8 GB Mac

AI models count only while loaded (#407 decides when). One 33 MP raw, 12 masks, the same window.

| State | Target | Today |
| --- | ---: | ---: |
| Rest (Fit, idle a minute) | ≤ 1.0 GB | 1.78 GB |
| Brushing at Fit | ≤ 1.5 GB | not measured |
| Zoom 100 % / 200 %, panning, brushing there | ≤ 2.0 GB | 2.7–3.0 GB |
| Full-size picture being built (at rest past Fit) | ≤ 2.0 GB | 7.4 GB |
| Export (editor open at rest, one photo) | ≤ 2.0 GB | 8.4 GB for the export alone |

How the design meets them (*estimate*: sizes from the formulas above, f32 everywhere unless said):

**Rest, 0.8 GB.** App 0.4. One preview level (2560 px, f32) 0.05. Loupe texture 0.02. Display tiles, capped, 0.10. Loupe stages for the next edit, with cheap alphas not kept and kept alphas cut to their box (step 6), 0.25. The full-size original is let go after the idle time at Fit (it is read again on the next zoom; the display tiles on disk show at once): 0. GPU pool: 0. Total 0.82 GB. Keeping the original at rest as f16 (step 10) would add 0.19: 1.0 GB, at the line.

**Brushing at Fit, 1.2 GB.** Rest 0.82 + the full-size original, when it was read back for a zoom, 0.37 + the live preview's second render, only under the stroke, 0.05. Total 1.24 GB.

**Zoom and brushing, 1.9 GB.** App 0.4. CPU original 0.37. Preview level 0.05. Loupe stages 0.25 (kept for zooming out). The visible tiles' kept stages at 100 % (about 7 MP with the ring around the screen): linear image 84 + planes 140 + costly alphas cut to their box about 84 + brush parts 30 = 0.34. The GPU source cut to the batch and its halo, not the whole original: 0.08. Display tiles and loupe texture 0.12. The render in flight, masks and finish per band: 0.25. Live preview under the stroke: 0.05. Total 1.91 GB.

**Full-size picture being built, 2.0 GB.** The zoom state at rest without the live preview and in-flight render (1.61) + the frame context (section 3.3) 0.05 + one band's working set 0.3. Total 1.96 GB. It no longer exists as one whole-frame render.

**Export, 1.4 GB alone, 2.0 GB with the editor at rest.** App 0.4 + original 0.37 + frame context 0.05 + one band 0.3 + the encoder's rows (step 8; until then the whole 8-bit output, 0.1–0.25). With the editor open at rest (0.8, which shares the app and may share the original) about 2.0 GB.

The band size follows the budget: band pixels = band share ÷ bytes a pixel, where a band's bytes a pixel are about 12 (source cut) + 12 (sampled) + 12 (linear) + 4 × planes + 4 × masks + 4 (output) + temporaries. At 12 masks that is about 130 B/px, so a 0.3 GB band is about 2.4 MP (7008 px × 350 rows) plus its halo.

## 3. The design

### 3.1 Three resolutions, one render path

- **Fit** renders the whole frame at the screen's size (≤ 2560 px edge) from the preview level, as now. 4.4 MP: small, and every slider tick reuses its stages.
- **Past Fit** the view is a grid of 256 px tiles at the zoom's scale. A render covers the missing visible tiles in one batch (their bounding rectangle, rows split by the band size), so the number of GPU dispatches stays as today's one window. Today's zoom window is this with one batch; its 4.5 % margin becomes a ring of tiles rendered after the visible ones.
- **Full size** (the full-size picture, export) is the same batch over the whole frame, in bands.

So there is one way to render a part of the frame, used by the zoom, the full-size picture and export, and memory is bounded by the batch or band, not by the photo.

### 3.2 Pixel formats

- **Working buffers stay f32.** Inside a batch or band the maths is today's, on today's types: the output stays bit-identical. Once work is per band, the working format barely matters to the peak.
- **What is kept is what counts**: the original, kept stages, kept alphas, display tiles. Display tiles are RGBA8 (as the full-size picture is now).
- **f16 for kept data is a later, separate step (step 10)** and changes output within a tolerance (section 3.8). WGSL's core `pack2x16float` / `unpack2x16float` store f16 in `u32` words without the optional `shader-f16` feature, so it works on every backend and in browsers.
- **Mask alphas never go to u8.** An alpha in 1/255 steps under a strong effect bands: a +4 EV mask over a linear gradient changes by 4/255 EV ≈ 0.016 EV per step, about 1 % in light: half an 8-bit level at mid-grey, more once contrast or a curve steepens it, so contour lines across a smooth sky, and every step shows in a 16-bit export. f16's step at 1.0 is 2^-11 (0.0005), 8× below one output level.

### 3.3 The frame context: whole-frame work done once, small

Some stages need the whole frame. Today a band or window repeats that work, which is why three bands of a 45 MP photo took about twice the time of one render (#199, `crates/ui-egui/src/full_picture/build.rs`). In this design the whole-frame work is a separate, small product, made once per edit and shared by every band, tile, view and export of it:

- the joint colour noise reduction's noise model (already shared per view: `NoiseMemo`);
- Dehaze's airlight (already from a 384 px proxy for windows: `AIRLIGHT_PROXY_EDGE`);
- the Highlights / Shadows coarse median (already made once and cut into tiles by the dirty render: `coarse_median`), and the reduced grids of the fast guided filters (`guided_fast`: Highlights / Shadows base, Clarity). Their coefficients already live on a grid 1–16× smaller than the image and are only upsampled bilinearly and applied per pixel, so keeping the grid (about 1 MB at 33 MP) makes these wide planes local: a tile needs one grid cell of halo, not 550–680 px;
- Recolor's references, the lens blur's subject estimate, Background retouching's analysis (already whole-frame products on `Plan`).

The context is made by a pass over the frame in bands (box downscales and medians are local), with the same kernels as the whole render, so the grids hold the same values and the output stays identical. Memory: a few MB, plus Dehaze's full-size plane while Dehaze is on (125 MB; banding it with its halo is the alternative, decided by measuring in step 4).

### 3.4 Masks

- **Stored as what defines them**, as now: strokes (points, size, feather, flow, pressure), gradient and range parameters, colour samples, AI selections as small stored grids (SAM 3: 288 × 288 logits, i8; refined edges' grids), source mattes (u8). None of these grows with the render size.
- **Rasterised per batch or band, on demand.** Every kind is local: a brush dab reaches its radius, gradients and ranges are per pixel, colour range smooths with σ 0.2 % of the long edge, heuristic Subject 3 %, Refine Edges' guided filter up to 4 % (280 px at 33 MP), Decontaminate its `reach`. AI selections read the whole photo only when made; a render samples their grid. So a band evaluates its masks over itself plus the largest reach of its masks, and the alpha stack is band-sized: 12 masks × 2.4 MP × 4 B = 115 MB instead of 1.5 GB.
- **Kept alphas** (for the next slider tick) are kept only where evaluating again costs: brush (many dabs), Auto Mask, colour range (guided smoothing), AI selections (CPU sampling and upload), Refine Edges, Decontaminate. Gradients and luminance ranges are one kernel pass and are evaluated again. Kept alphas are cut to their non-zero box, as the CPU's `MaskAlpha` already is: a radial or brush mask often covers a fifth of the frame.
- **Earlier windows' alphas** (pan back, #175) go: panning back hits the display tiles, and only the edit after it renders.

### 3.5 One cache and one budget

- **One ledger** (`lightcraft_engine::memory`): every holder registers its bytes, CPU or GPU, a class, and a release function. Classes, evicted in this order: *disk-backed* (display tiles with a disk copy, the full-size original that can be read again), *derivable* (kept stages of tiles off screen, preview levels, the GPU pool), *next-edit* (the loupe's and the visible tiles' kept stages, kept alphas), *on screen now* (never evicted while drawn). Within a class, least recently used goes first.
- **Every holder is on it**: `MediaCache`, the views' `GpuStages` (images, planes, alphas, brush parts, host copies, decontaminated edges), the GPU source, the display tiles (today's full-size picture), the brush live preview, the GPU buffer pool, renders in flight (they reserve their band before they start, as decodes do at the work gate), and loaded AI models (counted while loaded; while a model runs, the others are evicted to make room).
- **Two numbers**: a working budget and a rest budget, from the machine's memory. On an 8 GB Mac: working 2 GB, rest 1 GB (a quarter and an eighth of RAM; larger Macs get more, for speed; `LIGHTCRAFT_MEMORY_MB` still overrides). The idle trim (today's 3 s / 30 s rule and `memory::release()`) shrinks to the rest budget. On macOS the system's memory-pressure warning does the same at once (in `crates/sysmem`, where `unsafe` is allowed).
- **CPU and GPU in one number on Apple silicon** (unified memory: GPU buffers count in the footprint). On a discrete GPU (Windows, Linux), the ledger keeps a second number for device memory.
- The runaway allowance (`HARD_FACTOR` 4) and the separate caps (`KEPT_ALPHA_BYTES`, `STAGE_BUDGET_SHARE`, the pool's eighth, the joint noise reduction's fixed 2 GB, the export gate's 2× the budget) go: the ledger is the only limit.

### 3.6 Full-size work and export

- **The full-size picture** becomes display tiles at each level, built from the visible tiles outward at the lowest priority, band by band through the frame context. It never holds the whole frame's stages. Its disk copy is kept per tile, so going back to a photo reads only what is on screen.
- **Export** renders in bands through the same path and hands rows to the encoder. JPEG, PNG and TIFF are written row by row by nature; the encoders here take a whole image (`crates/codecs/src/encode.rs`), so a row interface is a change in `storytold/craft-libs` (AGENTS.md rule 9). Until then export holds only the 8-bit (or 16-bit) output, not the stages. Output sharpening is local (a halo); the JPEG size limit's search re-encodes from the kept rows or the small encoded output.
- `render_weight` (48 B/px today) becomes the band's reservation plus the output, so a batch fits more photos side by side in the same budget.

### 3.7 The brush live preview, the dirty render and the zoom windows

- **Brush live preview** (`fork/brush-live-preview.md`): unchanged on screen. Its coverage is already in 64 px tiles where the stroke paints. Its second render (the view with the brush covering everything, `Slot::BrushFull`) is rendered only for the tiles the stroke has reached, instead of the whole view. Painting stays a screen-space blend in each frame's dirty rectangle, so smoothness does not depend on the render.
- **Dirty render** (`fork/dirty-render.md`): its 64 px dirty tiles nest in the 256 px cache tiles. It grows from spots to brush strokes and moved gradients: a committed stroke changes its mask's alpha only inside the stroke's box and feather, so only those tiles are evaluated and finished again.
- **Zoom windows** (`Region`, `RegionBefore`): become batches of tiles. A pan renders only the newly visible tiles; the window's kept stages become the visible tiles' kept stages, in the ledger.

### 3.8 Output: identical, or within a stated tolerance

- **Steps 1–9 keep the output bit-identical.** Each band, batch or tile is today's window render with a halo that covers every stage's reach (the dirty render and the zoom window already work this way: `box_v` starts its runs on the whole image's rows, a cut is resampled with the whole image's taps). Tests compare every banded and tiled result with the whole render pixel for pixel, with 12 masks of every local kind.
- **Exceptions that render whole, as today**: the lens blur (read back and filtered on the CPU over the whole frame) and a float (`F32Hdr`) render where its stages need it; they reserve their size on the ledger and wait for it.
- **Step 10 (f16 kept data) changes output within a tolerance** and needs techneissa's word. Proposed bound: 8-bit output within 1 level everywhere, unchanged in at least 99 % of pixels; 16-bit export within 2^-10 relative. Why it is safe: f16 keeps 11 bits of every value at any brightness, finer than the shot noise of the brightest raw pixels (about 1/220 at 50 000 electrons) and far finer than one output level. Core Image's default working format is half float for the same reason (sources).

## 4. Risks

- **Speed.** Per-tile overhead is avoided by rendering batches, not tiles. Bands repeat their halos: with the frame context the halos are the local reaches (noise reduction's band halo, Refine Edges' 280 px at full size, Decontaminate's reach), not the wide planes. Step 4 measures one full-size build against today's one render (#199 measured 2× with the context repeated). Brushing stays a screen-space blend; a committed stroke renders fewer tiles than today.
- **Masks that need the whole image.** AI selections (inference at a fixed size when made; stored), LaMa fills and Heal/Clone sources (already windowed: `spots::window_for_reads` grows a window to read a spot's source, and `spots_scale` keeps finished spots). Refine Edges' guided filter at 4 % is the widest mask reach: a band carries 280 px of halo for it. A coefficient grid as in `guided_fast` would remove that but changes output, so it waits for a decision.
- **GPU limits.** Bands keep every buffer small: a 33 MP f32 RGB buffer (375 MB) is over WebGPU's default storage-binding limit (128 MB); wherever a device's limit is that low, such a render goes to the CPU today (`Gpu::fits`). Bands remove that cliff, on low-end GPUs and in browsers.
- **Windows and Linux.** Discrete GPUs have their own memory; wgpu does not report how much is free on every backend, so the device number starts from the adapter's limits and the joint noise reduction's existing free-memory query (`band_budget_from`). Integrated GPUs share memory as on a Mac.
- **WASM.** A 4 GB address space and browser limits: tiles and bands are what make 33 MP possible there at all.
- **Eviction churn.** A budget too small re-renders what it just evicted. The classes keep what is on screen and what the next edit needs; the rest goes first. Measured on the #402 session at each step.
- **Two kinds of cache keys.** Display tiles must never show an old edit: they use the full-size picture's key (`picture_job`: settings, file, size, crop, profiles, display), already tested.

## 5. Ordered steps

Each is one PR, measured on the #402 session (12 masks, zoom 100 / 200 %, pans, strokes, idle) with `footprint -p` and `ui.inspect` → `memory`. Savings are *estimates* from section 2's sizes.

| # | Step | Saves | Output | Issue |
| --- | --- | --- | --- | --- |
| 1 | Masks and the finish stage per band in renders that keep nothing (full-size picture, export) | done in [PR #435](https://github.com/techneissa/lightcraft/pull/435), measured on the export with 12 masks (after #405 and #409): GPU 4.5 → 1.8 GB (the 0-mask level), footprint 6.6 → 3.2 GB. Masks evaluated whole (a CPU part, Decontaminate, Refine Edges at 9 or more at 33 MP) still hold a full-size plane and their scratch: Refine Edges 20 on one mask, 7.5 GB footprint (`fork/mask-bands.md`) | identical | [#406](https://github.com/techneissa/lightcraft/issues/406) |
| 2 | The joint colour noise reduction's bands take a share of the memory budget, not a fixed 2 GB, when the free device memory is unknown | GPU peak of every full-size render and export about 2.6 → 1.5 GB (the planes stage then the peak) | identical | [#409](https://github.com/techneissa/lightcraft/issues/409) |
| 3 | A zoom window uploads only the part of the original it reads | 374 MB GPU while zoomed | identical | [#410](https://github.com/techneissa/lightcraft/issues/410) |
| 4 | The frame context, and every stage per band in the full-size render and export | full-size render and export: GPU about 1.5 → 0.4 GB; full-size render peak about 4.2 → 2.0 GB | identical | [#411](https://github.com/techneissa/lightcraft/issues/411) |
| 5 | One ledger and two budgets (working, rest) for every holder; memory-pressure trim | rest 1.78 → about 1.2 GB (original let go at Fit, stages to the rest budget); working bounded | identical | [#412](https://github.com/techneissa/lightcraft/issues/412) |
| 6 | Kept alphas: only the costly kinds, cut to their box; earlier windows' alphas go | zoom 0.37 → about 0.1 GB of alphas; loupe 166 → about 60 MB | identical | [#413](https://github.com/techneissa/lightcraft/issues/413) |
| 7 | Display tiles replace the full-size picture's textures and the zoom window (also covers #213) | picture textures 250 → 100 MB cap; zoom kept stages to visible tiles | identical | [#414](https://github.com/techneissa/lightcraft/issues/414) |
| 8 | Export streams rows into the encoders (row interface in craft-libs first) | export output buffers about 0.25 GB → rows | identical | [#415](https://github.com/techneissa/lightcraft/issues/415) |
| 9 | Brush strokes and moved gradients render only their tiles; the live preview's second render only under the stroke | brushing: about 0.2–0.4 GB (one view's stages), and faster commits | identical | [#416](https://github.com/techneissa/lightcraft/issues/416) |
| 10 | f16 for kept data (original, kept stages, kept alphas) | original 375 → 187 MB, kept stages about half | within the tolerance of 3.8, techneissa decides | [#417](https://github.com/techneissa/lightcraft/issues/417) |

Step 1 first: it removes the largest single peak with the least change (the finish stage already runs in row bands, `BAND_PIXELS`; every mask kind is local; the zoom window already proves halos give the same bits). Step 2 is a few lines with the next largest saving: the noise reduction's bands are already exact, only their size changes. Step 1 is the first half of #406 (folded in there); step 4 is the second half.

**AI models** ([#407](https://github.com/techneissa/lightcraft/issues/407), worked on separately): a model loads only for an AI mask. With step 5, a loaded model is a ledger holder: while it runs the others are evicted, and how long it stays is #407's decision. SAM 3 (2.2 GB) and BiRefNet Matting (about 3 GB) together do not fit beside 2 GB of work on an 8 GB Mac, so one model at a time is the rule to aim for there.

## Sources

Read for the ideas only; no code copied. No GPL, LGPL or AGPL source was read, and no other app was run.

- OpenImageIO, *ImageCache* documentation (Apache 2.0 project): tiles loaded on demand into a cache with one memory limit (`max_memory_MB`), least recently used tiles released. https://openimageio.readthedocs.io/en/latest/imagecache.html
- J. Ragan-Kelley et al., *Halide: A Language and Compiler for Optimizing Parallelism, Locality, and Recomputation in Image Processing Pipelines*, PLDI 2013, and Adams et al., *Decoupling Algorithms from Schedules*, SIGGRAPH 2012: computing a pipeline per tile with halos versus computing a stage once for the whole image, as a trade between recomputation and memory. https://people.csail.mit.edu/jrk/halide-pldi13.pdf
- K. He, J. Sun, *Fast Guided Filter*, arXiv:1505.00996 (2015): guided-filter coefficients on a subsampled grid, upsampled and applied at full size (what `guided_fast` does; the reason its wide planes can be made local). https://arxiv.org/abs/1505.00996
- Apple, Core Image documentation: a kernel declares the region of its input it reads for an output rectangle (`roi(forInput:arguments:outputRect:)`), so rendering can be tiled; the default working format is half float (`workingFormat`, RGBAh). https://developer.apple.com/documentation/coreimage/ciimageprocessorkernel and https://developer.apple.com/documentation/coreimage/cicontext/workingformat
- Apple, Metal documentation: purgeable memory states (`MTLPurgeableState`) for caches the system may reclaim (not exposed by wgpu; noted for later). https://developer.apple.com/documentation/metal/mtlpurgeablestate
- W3C, WebGPU and WGSL: `shader-f16` is optional; `pack2x16float` / `unpack2x16float` are core built-ins. https://gpuweb.github.io/gpuweb/#shader-f16 and https://gpuweb.github.io/gpuweb/wgsl/
- L. Williams, *Pyramidal Parametrics*, SIGGRAPH 1983: image pyramids, levels a fixed factor apart (as the full-size picture's levels).
