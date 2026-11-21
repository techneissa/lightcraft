# AI mask models stay loaded while masking

List: the AI mask models (SAM 3, BiRefNet, BiRefNet Matting) stay loaded while the Masking panel is open in the front window, so a mask made after a pause takes about 4 s instead of 17 s, and the panel says "Loading the AI models…" while a mask waits for them (issue #353). Branch `perf/353-ai-mask-speed`.

## What it does

- **The cause.** Since #304 (17 Nov) the SAM 3 worker lets SAM 3 and BiRefNet go a minute after its last request (it was 10 minutes), and the idle rule lets BiRefNet Matting go after a minute too. On Core ML, loading them again and planning each part on the GPU at its first run makes the next mask take about 17 s instead of about 4 s. So every mask made more than a minute after the last one was as slow as the first one after a start.
- **Kept while masking.** While the Masking panel is open in the front window (the Tools panel shown, not full screen), the app holds the models (`Session::keep_mask_models`): the worker does not unload them (`segment/worker/keep.rs`: `unload_at`), and BiRefNet Matting holds an idle-rule use (`HairEdges::keep_loaded`). When the panel closes or the window goes to the background, the minute starts. Outside Masking, #304's rule is unchanged.
- **What the mask waits for.** When a request loads a model, or runs one for the first time after its load (Core ML plans it then), the Masking panel says "Loading the AI models…" for that request instead of "Selecting…" (`Segmenter::loading_models`).
- Not done: loading the models when the panel opens, before the first mask (it would cost GPU time and memory for every visit to Masking, brush masks included).

## How it was measured

`crates/engine/examples/mask_timing.rs`: Session → `mask.add` (Subject, "SAM 3 + BiRefNet Edges") on CC0 portraits (`corpus/portraits`, 9–22 MP), each model's log line printed with the wall and CPU seconds of the process; one process per row; a test Mac with other builds running (load average 15–60), one run each, so wall times swing. Before: `custom` at 9 Nov (`9d834a5c9`: SAM 3 and BiRefNet on candle, no matting edge, no refinement, models kept 10 minutes).

| Mask | 9 Nov build | `custom` before this change | This change |
| --- | ---: | ---: | ---: |
| First after a start | 16.3 s wall, 3.4 s CPU | 17.5 s, 8.4 s CPU | 15.7 s, 8.2 s CPU |
| Next, right after | 19.9 s, 4.2 s CPU | 3.9 s, 2.2 s CPU | 5.8 s, 2.9 s CPU |
| After a 75 s pause | 14.8 s, 3.4 s CPU | 16.9 s, 8.0 s CPU | 4.2 s, 1.8 s CPU |

Where the time goes on `custom` before this change (wall):

| Stage | First mask | Right after | After a 75 s pause |
| --- | ---: | ---: | ---: |
| Model loads (SAM 3, BiRefNet, Matting) | 2.6 + 1.5 + 0.9 s | — | 3.2 + 0.6 + 0.6 s |
| SAM 3 (render, encoder, prompt) | 6.4 s | 2.0 s | 6.1 s |
| BiRefNet | 2.2 s | 0.7 s | 3.0 s |
| BiRefNet Matting (and its 2048 px render) | 3.5 s | 0.9 s | 2.8 s |
| Edge refinement (3072 px render, snap, grids) | 0.4 s | 0.4 s | 0.6 s |

On the 9 Nov build the first mask was SAM 3 9.3 s, BiRefNet 6.1 s (both candle) and loads 0.9 s. So Core ML made a mask in the same session about 4× faster than before, and the matting edge and the refinement (#240) add about 1.2 s of it; the slowness was the minute's unload. One more run with this change, at load average 34, gave 14.3 s after the 75 s pause with no model loaded again (each run slow, the CPU time 5.0 s): the Mac was busy, not the models.

## Branch

`perf/353-ai-mask-speed`

## Own files

`crates/engine/src/segment/kept.rs`, `crates/engine/src/segment/worker/keep.rs`, `crates/engine/examples/mask_timing.rs`, `fork/mask-models-kept.md`, `fork/tests.d/mask-models-kept.txt`

## Hooks in official files

- `crates/engine/src/segment.rs`: `mod kept`. `crates/engine/Cargo.toml`: the `mask_timing` example.
- Fork files of other features: `crates/engine/src/segment/worker.rs` (`mod keep`, `Shared::hold` and `Shared::loading`, `Worker::hold` and `Worker::loading`, the idle wait asks `keep::unload_at`, SAM 3's load raises `loading`), `crates/engine/src/segment/worker/birefnet.rs` (BiRefNet's load and an unloaded matting model raise `loading`), `crates/engine/src/hair_edges.rs` and `hair_edges/model.rs` (`Inner::held`, `keep_loaded`, `model_loaded`), `crates/ui-egui/src/background.rs` (the hold follows the Masking panel in the front window), `crates/ui-egui/src/panels/masking.rs` ("Loading the AI models…").

## Tests

`fork/tests.d/mask-models-kept.txt`. `keep::tests`: held models are never due, and the minute starts when the hold ends (the old rule unloaded them a minute after the last request whatever the panel). Timing: `LIGHTCRAFT_CONFIG_DIR=<scratch> MASKING_PANEL=1 cargo run --release -p lightcraft-engine --features sam --example mask_timing -- sam3+birefnet 0,0,75 PHOTO1 PHOTO2 PHOTO3` (`MASKING_PANEL` holds the models as the open panel does).

## What's New entry

### AI Masks Stay Quick While You Mask
- A Subject or Background mask made a while after the last one no longer waits for the AI models to load again: they stay loaded while the Masking panel is open, and are let go a minute after you leave it.
- When a mask does wait for the models (the first one after a start), the Masking panel says "Loading the AI models…".
