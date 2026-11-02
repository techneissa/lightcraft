# BiRefNet fine-edged Subject masks

List: BiRefNet, a pure-Rust (candle) fine-edged subject cut-out, as a model for new Subject and Background parts, next to SAM 3 and a "SAM 3 + BiRefNet Edges" mix (issue #48). Branch `feat/48-birefnet-masks`.

**Status: in progress.** The model crate and the stored model id are done and tested; the engine wiring, the dropdown, the download, the comparison sheets and the PR are not (see "What is left").

## What it does

Done:
- `crates/birefnet` (`lightcraft-birefnet`) runs BiRefNet (Swin-L backbone, bilateral reference decoder, deformable ASPP) on candle, loading `model.safetensors` directly (F16, BF16 or F32; converted to F32). It matches the PyTorch reference: on a 1024 × 1024 test image the logits differ by at most 0.0038 (range −77 … +22), mean 1e-5, and every pixel is on the same side of 0.
- Speed on this Mac (M4 Pro, Metal, F32, at a machine load of 75–100 from other lanes, so a quiet Mac should be faster): 5.7 s for the first matte, 11–12 s for the next two under the same load (PyTorch on the CPU: 7.6 s). Where it goes (`LIGHTCRAFT_PROFILE=1`, one run): backbone 2.4 s + 0.5 s for the half-size copy, decoder block 1's 7×7 deformable convolution at 256 × 256 about 3 s (its gather is the cost), the rest under 1.5 s. Ideas not tried: F16 activations, fewer taps per chunk.
- `lightcraft_birefnet::combine::refine_edges`: "SAM 3 + BiRefNet Edges". SAM 3's logits decide away from its edge (so BiRefNet's mistakes elsewhere never come in); within a band of 3 % of the matte's side around SAM 3's edge, BiRefNet's cut-out decides; smooth between.
- `SegMask::model` (`crates/develop/src/segmask.rs`): the id of the model that made a stored selection (`birefnet`, `sam3+birefnet`; none means SAM 3). Kept when the selection is turned around (Background). A build without the field ignores it: it reads the selection as SAM 3's, and drops the id the next time it saves the mask.
- Models live in `~/Library/Application Support/ArtCraft/models/birefnet/` (and `birefnet-matting/`), one `model.safetensors` each; `Variant` has the pinned size and SHA-256 of the files (Hugging Face `ZhengPeng7/BiRefNet`, `BiRefNet-matting`). The general model is already downloaded and verified on this Mac. Downloaded models are kept.

## Branch

`feat/48-birefnet-masks`

## Own files

`crates/birefnet/` (`src/{lib,run,model,swin,deform,blocks,params,weights,image,combine,fetch}.rs`, `examples/compare.rs`), `fork/birefnet-masks.md`, `fork/tests.d/birefnet-masks.txt`

## Hooks in official files

- `Cargo.toml`: workspace dependency `lightcraft-birefnet`. `xtask/src/layers.rs`: `birefnet` is layer 3. `NOTICE`: the BiRefNet and Swin (MIT) entry.
- `crates/develop/src/segmask.rs`: field `SegMask::model`, `SegMask::made_by`; `invert` keeps it. Test literals of `SegMask { … }` in `crates/pipeline/src/masks/tests.rs` and `crates/engine/src/segment.rs` got `model: None`.

## What is left (design for the next lane)

1. **Engine** (new files `crates/engine/src/segment/birefnet.rs`, `crates/engine/src/cmd/birefnet.rs`; small hooks):
   - `SelectionModel { Sam3, BiRefNet, Combined }` with ids `sam3`, `birefnet`, `sam3+birefnet` (and `birefnet-matting` if the comparison shows it is better for hair); `Segmenter` gets `selection` (the dropdown) and the models folder; "installed" per model.
   - `worker.rs`: a `Kind::Subject { text, main, model, birefnet_dir }`; `State` gets `birefnet: Option<(PathBuf, BiRefNet)>` (dropped by the idle unload and on a panic, like SAM 3). BiRefNet only: `render(job)` → `BiRefNet::matte` → `SegMask::from_logits(1024, …).made_by("birefnet")` (1024 is `MAX_SIDE`; nothing found = no logit above 0 → `Ok(None)`). Combined: SAM 3's `text_logits` (288 side) then `refine_edges`. Both under the job's `cancel` (`set_cancel`) and the existing GPU→CPU retry. SAM 3 results get no id.
   - `segment_concept_later` / `segment_concept` choose the kind from `segmenter.selection` for Subject and Background only (Sky, Object and Describe stay SAM 3); the model falls back to SAM 3 when the chosen one is not installed. `Tag::NewConcept` gets the model so `activity_label` says "Selecting with BiRefNet". The detail pass (`segment_detail`) stays SAM 3 for SAM 3 parts; a BiRefNet part could get one by running BiRefNet on the zoomed crop (the same `detail_region`), which would sharpen small subjects in big photos: worth doing if time allows, otherwise skip it for non-SAM parts.
   - Commands in `cmd/birefnet.rs` (registered by a line in `cmd.rs`): `birefnet.model.status`, `birefnet.model.download {variant, acknowledged}`, `birefnet.model.cancel` (reuse `segment/download.rs`'s `Downloader` with `lightcraft_birefnet::fetch::{Variant, mirrors}`; it already has the activity row, resume and SHA-256), and `segment.selectionModel {model?}` to read or set the dropdown. Keep the choice across restarts (the app's settings).
2. **UI** (new file in `crates/ui-egui/src/panels/`): the dropdown at the right of the "Masking" heading row (`right::header` paints the title; put the control in that row's rect), shown only when two or more selection models are installed; changing it touches no existing part; a one-click download offer (size shown, after asking) for BiRefNet when it is not installed; the part row shows the model (small label, tooltip) when the part's `seg.model` is not SAM 3. Locale strings in `crates/ui-egui/locales/*.json`; Title Case, no help line that repeats a label.
3. **Tests**: the model stored per part and round-tripped (mask JSON with `seg.model`), the dropdown setting new parts only (set it, add a Subject, change it, add another: the first keeps its id), a fake worker result with each id, hostile input (done for the crate).
4. **Comparison** (needs the PR): 6+ CC0 or public-domain photos you have looked at (hair against sky, a pet with fur, lace or foliage, a person in a busy street, a product on a plain background), SAM 3 / BiRefNet / combined side by side with timings, sheets in `$WORKSPACE/assets/lightcraft/shots/birefnet-compare/` and on `pr-assets` folder `feat/48-birefnet-masks/compare/`, sources in `$WORKSPACE/assets/lightcraft/design/birefnet-tests/sources.md`. A Haiku scout was started to find the photos on Wikimedia Commons and was rate-limited (HTTP 429); that folder is empty. `crates/birefnet/examples/compare.rs` and the Python reference (`$WORKSPACE/assets/lightcraft/scratch/birefnet-venv/ref.py`, a throwaway venv outside the repo) are the tools; try `BiRefNet-matting` (885 MB, `ZhengPeng7/BiRefNet-matting`) on the hair photos.
5. **After only** for the dropdown (it is new); the comparison sheets show the result.
6. **Docs**: `docs/ai-masks.md` (a short section), a `## What's New entry` and `## Parity rows` here (LR-MASK-SUBJECT / -BACKGROUND, edited in place), and the older-build behaviour in the PR text.

## Tests

`cargo test -p lightcraft-birefnet --lib` · `cargo test -p lightcraft-develop --lib -- segmask` (also in `fork/tests.d/birefnet-masks.txt`). Against the reference: `cargo run --release -p lightcraft-birefnet --example compare -- <model dir> <reference dir>`, where the reference dir holds `input.f32` and `logits.f32` from `ref.py`.
