# Performance: Portrait where it is fastest

List: with Portrait on, the GPU makes the image before the retouch step itself and keeps it on the device, the step reads back only the rows it needs, and the face search runs the wrinkle search beside the others (issue #241, first part). Branch `perf/241-portrait-gpu`.

## What it does

- **The image before the step stays on the GPU.** With a Portrait setting on, the GPU render used to read the
  resampled frame back, run white balance, defringe, spots and red eye on the CPU, keep that image on the host and on
  the device, and copy the host image on every tick. Now the device makes it with the kernels a render without
  Portrait uses (`render::linear_on_device`) and keeps only the device copy (`crates/gpu/src/pre_retouch.rs`). Spots
  with no GPU kernel (LaMa, Content-Aware, shaped spots) still make it on the CPU, as before, and upload it. The ⌥-drag
  map of a Portrait slider reads the kept image back when it needs it.
- **The step reads back a band of rows.** When every feature on writes near its faces, the step's boxes
  (`retouch_step::write_boxes`) hold everything it reads for each face. Only the rows from the first box's top to the
  last box's bottom are read back; `retouch_step::apply_rows` retouches them mapped as a zoomed window is (through the
  whole frame, moved by a whole number of rows), so they get the whole frame's pixels to the bit; the boxes' rows go up
  again. Background and Refine strokes may write anywhere: the whole frame is read back and uploaded, as before. On the
  two test portraits the band is 85–100 % of the rows (Eyes reads 0.7 of the face around it, and some features read the
  body and hair masks), so the band saves little there; a small face in a large frame saves most.
- **The wrinkle search runs beside the others.** It reads only the faces' mesh and masks, not their stray strands or
  spots, so the face search runs it on a thread of its own while Stray Hair and then Blemishes search
  (`retouch_input/parts.rs`); its result joins the faces they give, in the same order. A thread that can't start runs
  it after them, as before.
- **Profile lines.** `LIGHTCRAFT_PROFILE=1` prints each feature's run (`retouch <feature> (face …)`), each step of the
  face search (`portrait search: …`, and the float crop the wrinkle search renders) and the GPU's
  `gpu retouch: before it …, read back <n> of <h> rows, step, upload …`.

## Measured

`portrait_open_profile` (below) on two CC0 portraits of the corpus (`corpus/portraits`), the people parse cached, GPU,
release, a test Mac with other builds running (load 13–20), one run each:

| | Face-filling portrait (5760 × 3840), before → after | Small face (5371 × 3581), before → after |
|---|---|---|
| Face search (parse cached) | 1031 → 723 ms | — (the parse ran before) → 426 ms |
| Opening: first 2560 px render, every slider | 228 → 169 ms | 327 → 404 ms |
| Opening: every slider but Background | 98 → 89 ms | 120 → 174 ms |
| Export: full size, every slider | 618 → 580 ms | 729 → 713 ms |
| Export: every slider but Background | 276 → 286 ms | 319 → 238 ms |
| Image before the step (export, every slider) | 62 → 42 ms | 51 → 19 ms |

The render times move by ±50 ms between runs under this load; the image before the step and the face search are the
changes that hold.

Where the time goes now (face-filling portrait, after, the parse cached):

| Step | Time |
|---|---|
| Face search: the 1600 px picture and YuNet | 223 ms (126 ms of it decoding the photo) |
| Face search: people parse | 16 ms cached; 2.2–5.4 s when it runs (#216 moves it to Core ML) |
| Face search: Stray Hair, then spots | 61–67 ms, 14–15 ms |
| Face search: wrinkles (beside them) | 458–480 ms, of which the float crop 164 ms (125 ms decoding the photo again) |
| Opening, every slider: the step | 136 ms: Background 97 (prepared 54, drawn 43), Skin Tone 11, Eyes 9, Skin 8, Wrinkles 6, Blemishes 3 |
| Export, every slider: the step | 328 ms: Background 219 (prepared 53, drawn 166), Blemishes 24, Skin 22, Skin Tone 22, Eyes 18, Wrinkles 18 |
| Export: reading the frame back and uploading it | about 75 ms |

What is next: Background's draw on the GPU (the largest per-pixel step, and the reason for the whole-frame readback),
then the face features on the GPU so the band is not read back at all. The wrinkle crops' decode is now made once and
early (`fork/perf-portrait-analyses.md`).

## Branch

`perf/241-portrait-gpu` (from `custom`).

## Own files

`crates/gpu/tests/it/portrait_gpu.rs`, `crates/engine/examples/portrait_open_profile.rs`, `fork/perf-portrait-gpu.md`,
`fork/tests.d/perf-portrait-gpu.txt`.

## Hooks in official files

- `crates/gpu/src/render.rs`: `linear_on_device` (the device stage before the step, out of `linear`), `cpu_spots`,
  `KeptSpots` is `pub(crate)`; the retouch branch of `linear_stage` keeps the device spots; `effect_pre` reads the kept
  image back; `GpuStages::bytes` counts no host copy. `crates/gpu/src/pre_retouch.rs` (fork file): rewritten around the
  device image and the band. `crates/gpu/tests/it/main.rs`: `mod portrait_gpu`.
- `crates/pipeline/src/retouch_step.rs`: `apply_rows`, `apply_at`; `note_pre_retouch_run` is public (the GPU counts its
  device runs).
- `crates/retouch/src/run.rs`: the per-feature timing line.
- `crates/engine/src/retouch_input/parts.rs`: the wrinkle search on its own thread, the search timing lines;
  `parts/crop.rs`: the float crop's timing line.

## Tests

`fork/tests.d/perf-portrait-gpu.txt`. `a_band_of_rows_gives_the_whole_frames_retouch` (pipeline): for every slider of
every face feature, five points and a mesh, the band's rows equal the whole frame's to the bit and the whole frame's
step writes no row outside it. `portrait_gpu` (GPU): every face slider matches the CPU render within the equivalence
bounds (mean under 0.5, at most 3 levels; measured: at most 1 level, mean ≤ 0.0001 for every slider), a zoomed window
equals the whole frame within a level, and every slider at 0 or the Portrait eye off renders the bytes of the photo
without Portrait. `portrait_drag` compares every tick with a whole-frame render pixel for pixel. None measures time.

Timing: `LIGHTCRAFT_CONFIG_DIR=<scratch> LIGHTCRAFT_PROFILE=1 KEEP_PARSE=1 GPU_ONLY=1 cargo run --release -p
lightcraft-engine --example portrait_open_profile -- OUT_DIR PHOTO`.
