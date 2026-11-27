# AI mask models and the Masking panel

List: the AI mask models (SAM 3, BiRefNet, BiRefNet Matting) go a minute after the last AI request, also while the Masking panel is open; each AI mask has a Stop AI button that lets its models go at once; the panel says "Loading the AI models…" while a mask waits for them (issues #353, #407, #452). Branches `perf/353-ai-mask-speed`, `perf/407-mask-model-loads`, `feat/release-kept-mask-models`.

**Issue #452 (current rule).** Issues #353 and #407 kept the models loaded for as long as the Masking panel was open in the front window and the photo had an AI mask that used them. On Core ML that held 5.6 GB at rest (7.28 GB with a Subject mask made, 1.57 GB after Stop AI), however long the app sat idle. The hold is gone: the worker lets SAM 3 and BiRefNet go `idle` (`MODEL_IDLE`, 60 s) after its last request and BiRefNet Matting goes on the idle rule (`crate::idle`) the same, panel open or not. The next AI mask loads them again (about 17 s instead of 4 s on a test Mac, #407). The sections below say "kept" and "held" for how it worked before; only what they say about Stop AI, the loading message and `Session::mask_models_by_mask` still holds.

## What it does

- **The cause.** Since #304 (17 Nov) the SAM 3 worker lets SAM 3 and BiRefNet go a minute after its last request (it was 10 minutes), and the idle rule lets BiRefNet Matting go after a minute too. On Core ML, loading them again and planning each part on the GPU at its first run makes the next mask take about 17 s instead of about 4 s. So every mask made more than a minute after the last one was as slow as the first one after a start.
- **Kept while masking, for the photo's AI masks (issue #407).** While the Masking panel is open in the front window (the Tools panel shown, not full screen), the app keeps the models the open photo's AI masks use (`Session::mask_models_on`, `keep_mask_models`): Subject and Background the chosen selection model (SAM 3, BiRefNet or both) and BiRefNet Matting when it is their hair edge, Sky SAM 3, Object and Describe SAM 3, Person masks BiRefNet Matting when it is the people edge. A Subject, Background or Sky mask on a photo with its own matte for it (or with no model for it installed) uses none, as `concepts::wanted` decides when it is made. Brush, gradient and range masks use none, so a session of them holds no model, and a people parse ahead of time (`parse_ahead`) that loads BiRefNet Matting is not held unless the photo has a mask that uses it. The worker keeps SAM 3 and BiRefNet apart (`segment/worker/keep.rs`: `unload_at`, `let_go`), BiRefNet Matting holds an idle-rule use (`HairEdges::keep_loaded`). The UI keeps the session's answer (`Session::mask_models_by_mask`, the same the tests check) and asks again when the photo or its settings change, at most once a second otherwise, as the selection models it reads are found on disk (`panels/mask_stop_ai.rs`). When the panel closes, the window goes to the background, or the photo has no mask that uses a model, the minute starts for it. Outside Masking, #304's rule is unchanged.
- **Stop AI.** Each AI mask's row in the Masks list (pinned above the panel's scroll area, `panels/mask_list.rs`) has a Stop AI button, left of Invert and the eye: a filled rounded square while it can be clicked, its outline while there is nothing to stop. It can be clicked while a model the mask uses is in memory or a request for this mask is under way (on the AI mask worker, `Segmenter::mask_busy`, or Update AI Masks finding its parts, `Adaptive::asking_for`). It runs `mask.stopAi`: this mask's requests are cancelled (`Segmenter::cancel_mask`, `Adaptive::stop_mask`; other masks' go on), and the models it uses are let go once the step under way ends (a model load or one run, up to a few seconds), held or not; the mask keeps its stored selection. A Person mask's people parse runs inside the command that makes it, so none is left to stop; a Person mask whose people edge is Current or Apple Vision uses none of these models, has no button, and `mask.stopAi` refuses it saying so. `mask.aiModels` says what a mask uses, what is loaded, whether a request for it is under way, and whether Stop AI does something. The button is named "Stop AI" for screen readers, with its hover text as its description and its enabled state, and shows a focus ring; after a stop a toast says "AI stopped for <mask>" and the list's polite live region reads it out.
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

`crates/engine/src/segment/kept.rs`, `crates/engine/src/segment/worker/keep.rs`, `crates/engine/src/cmd/mask_stop_ai.rs`, `crates/ui-egui/src/panels/mask_stop_ai.rs`, `crates/engine/examples/mask_timing.rs`, `fork/mask-models-kept.md`, `fork/tests.d/mask-models-kept.txt`

## Hooks in official files

- `crates/engine/src/segment.rs`: `mod kept`, `pub use kept::MaskModels`. `crates/engine/src/cmd.rs`: `mod mask_stop_ai`. `crates/ui-egui/src/panels.rs`: `mod mask_stop_ai`; `crates/ui-egui/src/panels/mask_list.rs`: the button in a mask's row; `crates/ui-egui/src/icons.rs`: `Icon::Stop`. `crates/engine/Cargo.toml`: the `mask_timing` example.
- Fork files of other features: `crates/engine/src/segment/worker.rs` (`mod keep`, `Shared::hold`, `Shared::loading`, `Shared::sam3_in` and `birefnet_in`, `State::publish`, `Kind::Release`, `Worker::hold`, `loaded_models`, `release` and `loading`, the idle wait asks `keep::unload_at` and lets go only unheld models, SAM 3's load raises `loading`), `crates/engine/src/segment/worker/birefnet.rs` (BiRefNet's load and an unloaded matting model raise `loading`), `crates/engine/src/hair_edges.rs` and `hair_edges/model.rs` (`Inner::held`, `Inner::release`, `keep_loaded`, `release`, `model_loaded`, `may_use_matting`), `crates/ui-egui/src/background.rs` (the hold follows the Masking panel in the front window and the photo's AI masks), `crates/ui-egui/src/panels/masking.rs` ("Loading the AI models…").

## Tests

`fork/tests.d/mask-models-kept.txt`. `keep::tests`: held models are never due, the minute starts when the hold ends (the old rule unloaded them a minute after the last request whatever the panel), and a model no mask uses goes while the others are held. `segment::kept::tests` (issue #407): brush, gradient and range masks send no request to the AI mask worker, make no matte or people parse, and hold no model; a Person mask holds BiRefNet Matting; Stop AI lets the model go though it is held and keeps the mask's selection (runs BiRefNet Matting from the shared models folder, skipped without it). Timing: `LIGHTCRAFT_CONFIG_DIR=<scratch> MASKING_PANEL=1 cargo run --release -p lightcraft-engine --features sam --example mask_timing -- sam3+birefnet 0,0,75 PHOTO1 PHOTO2 PHOTO3` (`MASKING_PANEL` holds the models as the open panel does).

## What's New entry

### AI Masks Stay Quick While You Mask
- A Subject or Background mask made a while after the last one no longer waits for the AI models to load again: they stay loaded while the Masking panel is open, and are let go a minute after you leave it.
- When a mask does wait for the models (the first one after a start), the Masking panel says "Loading the AI models…".
- Only the models the photo's AI masks use stay loaded: brush, gradient and range masks keep none.
- Each AI mask has a Stop AI button in the Masks list: it stops the mask's AI work and frees its models at once. The mask stays as it is.
