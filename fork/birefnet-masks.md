# BiRefNet fine-edged Subject masks

List: BiRefNet, a pure-Rust (candle) fine-edged subject cut-out, as a model for new Subject and Background parts, next to SAM 3 and a "SAM 3 + BiRefNet Edges" mix (issue #48). Branch `feat/48-birefnet-masks`.

## What it does

Done:
- `crates/birefnet` (`lightcraft-birefnet`) runs BiRefNet (Swin-L backbone, bilateral reference decoder, deformable ASPP) on candle, loading `model.safetensors` directly (F16, BF16 or F32; converted to F32). It matches the PyTorch reference: on a 1024 × 1024 test image the logits differ by at most 0.0038 (range −77 … +22), mean 1e-5, and every pixel is on the same side of 0.
- Speed on this Mac (M4 Pro, Metal, F32, at a machine load of 75–100 from other lanes, so a quiet Mac should be faster): 5.7 s for the first matte, 11–12 s for the next two under the same load (PyTorch on the CPU: 7.6 s). Where it goes (`LIGHTCRAFT_PROFILE=1`, one run): backbone 2.4 s + 0.5 s for the half-size copy, decoder block 1's 7×7 deformable convolution at 256 × 256 about 3 s (its gather is the cost), the rest under 1.5 s. Ideas not tried: F16 activations, fewer taps per chunk.
- `lightcraft_birefnet::combine::refine_edges`: "SAM 3 + BiRefNet Edges". SAM 3's logits decide away from its edge (so BiRefNet's mistakes elsewhere never come in); within a band of 3 % of the matte's side around SAM 3's edge, BiRefNet's cut-out decides; smooth between.
- `SegMask::model` (`crates/develop/src/segmask.rs`): the id of the model that made a stored selection (`birefnet`, `sam3+birefnet`; none means SAM 3). Kept when the selection is turned around (Background). A build without the field ignores it: it reads the selection as SAM 3's, and drops the id the next time it saves the mask.
- Models live in `~/Library/Application Support/ArtCraft/models/birefnet/` (and `birefnet-matting/`), one `model.safetensors` each; `Variant` has the pinned size and SHA-256 of the files (Hugging Face `ZhengPeng7/BiRefNet`, `BiRefNet-matting`). The general model is already downloaded and verified on this Mac. Downloaded models are kept.

Engine and Masking panel (issue #48):
- Subject and Background parts can be made by SAM 3 (as before), BiRefNet, or "SAM 3 + BiRefNet Edges". A dropdown on the Masking heading's row (right side) sets the model for **new** parts only; it lists the installed models and is hidden with one (then a quiet "Get BiRefNet" offers the download, with the size, after asking). Each part keeps the model that made it (`SegMask::model`; hover the part's row: "Made with …"); Sky, Object and Describe stay SAM 3; a BiRefNet or combined part gets no SAM 3 detail pass.
- `crates/engine/src/segment/birefnet.rs`: `SelectionModel`, the settings (models folder, saved choice in `<config>/selection-model.json`, BiRefNet download), which models are installed, `Session::concept_kind` (the worker request). The worker (`segment/worker/birefnet.rs`) loads BiRefNet beside SAM 3 on the same thread (same idle unload, GPU→CPU retry, cancel), runs it on the 1008 px render of the photo and stores a 1024² grid; the combined model runs SAM 3 first, then BiRefNet, then `refine_edges`; when SAM 3 finds nothing (a fern, a bottle) BiRefNet's cut-out stands alone and is stored as BiRefNet's.
- Commands: `segment.selectionModel {model?}` (validated), `birefnet.model.status`, `birefnet.model.download {confirm: true}` (refused without `confirm`; resume, SHA-256 and an activity row through the existing downloader), `birefnet.model.cancel`.
- A BiRefNet-only part works without SAM 3 installed (Sky, Object and Describe still need it). Remote SAM workers: BiRefNet still runs here; "SAM 3 + BiRefNet Edges" is not offered with a remote worker.
- Default: with no saved choice, BiRefNet when installed (in the comparison of 13 CC0 photos its edges were finer than SAM 3's on every one, and it found a subject on the five photos where SAM 3 found none: ferns, branches, bottles; SAM 3 + BiRefNet Edges was no better than BiRefNet alone, and worse on a bag the coarse SAM 3 edge cut). `BiRefNet-matting` (885 MB, downloaded and verified, kept in `ArtCraft/models/birefnet-matting/`) gave slightly cleaner hair strands than the general model at the same 5.7 s: not offered, `crates/birefnet/examples/matte_dump.rs` runs it.
- Comparison: `crates/engine/examples/birefnet_compare.rs` writes each model's grid and seconds for photos; the sheets are in the PR.
- A build without this feature reads a BiRefNet part as a SAM 3 selection (an unknown `model` is ignored) and drops the id on its next save.

## Branch

`feat/48-birefnet-masks`

## Own files

`crates/birefnet/` (`src/{lib,run,model,swin,deform,blocks,params,weights,image,combine,fetch}.rs`, `examples/compare.rs`), `crates/engine/src/segment/birefnet.rs`, `crates/engine/src/segment/worker/birefnet.rs`, `crates/engine/src/cmd/birefnet.rs`, `crates/engine/src/tests/birefnet.rs`, `crates/engine/examples/birefnet_compare.rs`, `crates/ui-egui/src/panels/selection_model.rs`, `crates/ui-egui/src/tests/selection_model.rs`, `fork/birefnet-masks.md`, `fork/tests.d/birefnet-masks.txt`

## Hooks in official files

- `Cargo.toml`: workspace dependency `lightcraft-birefnet`. `xtask/src/layers.rs`: `birefnet` is layer 3. `NOTICE`: the BiRefNet and Swin (MIT) entry.
- `crates/engine/Cargo.toml`: optional dependency `lightcraft-birefnet` (in the `sam` feature), the `birefnet_compare` example. `crates/engine/src/cmd.rs`: `mod birefnet;`, `birefnet::specs()`. `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`, `crates/ui-egui/src/panels.rs`: `mod` lines.
- `crates/engine/src/segment.rs`: `pub mod birefnet;`, field `Segmenter::birefnet`, `Tag::NewConcept.model`, `activity_label` (BiRefNet rows), `segment_job` (BiRefNet alone needs no SAM 3 folder), `segment_concept` and `segment_concept_later` call `concept_kind`, `segment_detail` skips parts another model made, the two test literals.
- `crates/engine/src/segment/worker.rs`: `mod birefnet;`, `Kind::BiRefNet`, `State::birefnet` and `State::loaded`, the BiRefNet arm of `run_job` (before the remote check), `remote_job`'s refusal. `crates/engine/src/segment/download.rs`: the thread name and log lines use the model's name.
- `crates/engine/src/config.rs`: `with_default_sam3_model` also sets `segmenter.birefnet` (`LIGHTCRAFT_BIREFNET_DIR` overrides the models folder). `crates/engine/src/cmd/concepts.rs`: `wanted` asks `can_select`. `crates/engine/src/cmd/masks.rs` (`resolve_ai`, `mask.add`, `mask.addComponent`): Subject and Background requests no longer require SAM 3 first.
- `crates/ui-egui/src/panels/masking.rs` (`show_mask_panel`): `selection_model::show` after the heading. `panels/mask_parts.rs` (`part_rows`): the "Made with …" tooltip.
- `crates/develop/src/segmask.rs`: field `SegMask::model`, `SegMask::made_by`; `invert` keeps it. Test literals of `SegMask { … }` in `crates/pipeline/src/masks/tests.rs` and `crates/engine/src/segment.rs` got `model: None`.

## Tests

`cargo test -p lightcraft-birefnet --lib` · `cargo test -p lightcraft-develop --lib -- segmask` · `cargo test -p lightcraft-engine --features sam --lib -- birefnet segment:: concepts::` · `cargo test -p lightcraft-ui-egui --features lightcraft-engine/sam --lib -- selection_model` (also in `fork/tests.d/birefnet-masks.txt`). Against the reference: `cargo run --release -p lightcraft-birefnet --example compare -- <model dir> <reference dir>`, where the reference dir holds `input.f32` and `logits.f32` from `ref.py`.

## What's New entry

### Fine-Edged Subject Masks With BiRefNet
- Subject and Background masks can use BiRefNet, a model that cuts hair, fur and lace out far more finely than SAM 3. Masking has a model dropdown on its heading (SAM 3, BiRefNet, SAM 3 + BiRefNet Edges) for new parts; each part keeps the model that made it, and its row says so on hover.
- BiRefNet is a 444 MB download (MIT licence), offered as "Get BiRefNet" on the Masking heading and only after you agree. It runs on the Mac's GPU in a few seconds per photo.
- An older build reads a BiRefNet part as a SAM 3 selection.

## Parity rows

| LR-MASK-SUBJECT | Select subject | P2 | 🟡 | `cmd:mask.add` (`subject`), `crates/pipeline/src/masks.rs` | with SAM 3 installed (or a remote worker configured) the subject is what SAM 3 finds for `person | animal | vehicle | subject, main object, foreground` (a person first, else an animal, a vehicle, any main object), stored with the mask (288² logits, a zoomed-in detail pass, Edge) so previews and exports match and reopening needs no model, and added when found like Describe, with a row in the activity stack; nothing found is a message, never a rough mask; without SAM 3 the saliency heuristic, and the part says it is a rough mask and how to get SAM 3; a photo that carries person mattes (DNG semantic masks: iPhone ProRAW portrait / skin / hair / teeth / glasses mattes) uses them instead (synthetic files only: no corpus file carries person mattes). New parts can be made by BiRefNet (MIT, pure Rust on candle: fine hair, fur and lace edges, a 1024² stored grid, no detail pass) or SAM 3 + BiRefNet Edges, chosen in the Masking heading's dropdown (installed models only; hidden with one; `cmd:segment.selectionModel`, `cmd:birefnet.model.status`, `cmd:birefnet.model.download`, `cmd:birefnet.model.cancel`, `crates/birefnet`); each part keeps its model (`SegMask::model`, shown on hover) and changing the dropdown touches no existing part; not compared against Lightroom |
| LR-MASK-BACKGROUND | Select background | P2 | 🟡 | `cmd:mask.add` (`background`) | with SAM 3: everything but the subject, the subject's logits turned around and stored with the mask (no detail pass); without it the inverse of the heuristic subject (a rough mask, flagged), or of the photo's person mattes. Made by the model the dropdown names (BiRefNet and SAM 3 + BiRefNet Edges too; see Select subject), the subject's logits turned around, the model id kept |
