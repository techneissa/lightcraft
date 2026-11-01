# Reflection removal

List: Remove ▸ Reflections separates a window's reflection with a local model (DIRS-MuGI, or DSRNet from official PR #1220), kept in the folder shared with PhotoCraft. Branch `feat/25-reflection-removal`.

## What it does

The Remove panel gets a **Reflections** section (also Photo ▸ Remove Reflections) that separates a reflection, such as a shop window's, from the photo with a local DSRNet model: DSRNet-S, DSRNet-L or DSRNet-L Nature, at Preview (384 px), Standard (768 px) or High (1024 px). Nothing leaves the Mac. Amount (default 35 %), Protect Sky, Detail Protection (default 60 %) and Limit to Area change the result without running the model again; changing the model or the quality needs Recalculate. The correction is a small grid saved beside the library settings (`reflection-cache`), never in the photo. Mac only (Candle on Metal, CPU as the fallback). Issue #25.

This is official PR storytold/lightcraft #1220 ("Add experimental local DSRNet reflection removal", d4nnyp73), taken early with its commits kept. Its own notes: [docs/ai-reflections.md](../docs/ai-reflections.md).

Commands: `photo.removeReflections`, `reflection.apply {model?, quality?}` (both return at once; the work runs on a thread with a "Reflection Removal" row in the activity stack), `reflection.toggle`, `reflection.reset` (undoable), `reflection.status`, `reflection.install {path?}`, `reflection.models`, `reflection.panel`, `reflection.instructions`; controls `reflection.amount`, `reflection.detailProtection`, `reflection.left` / `top` / `right` / `bottom`.

DIRS-MuGI (added after the first version, `crates/reflections/src/dirs.rs`, `swin.rs`) is used exactly as its authors publish it, with no conversion: the folder `ArtCraft/models/dirs-mugi/` holds `dirs_mugi_lors.ckpt` (345 MB) and `swin_large_o365.pth` (785 MB), read by Candle's pickle reader (no code from the files runs). **Download…** in the panel asks first (1.1 GB from huggingface.co/huqiming513/DIRS-Models), then fetches both files with resume and SHA-256 checks (`reflection.download {accepted: true}`; without `accepted` the command only reports what it would fetch). **Install Reflection Model…** also takes a folder holding the two files. The port matches PyTorch to 4e-6 on a 192×160 image (CPU 2.5e-6, Metal 4.3e-6).

DSRNet is not bundled and the app does not download it. It is converted once from the author's checkpoint (`tools/convert-reflection-model.py`, needs Python with torch), then **Install Reflection Model…** copies the folder (`manifest.json` + `model.safetensors`, checked against its SHA-256) into the models folder. The fork keeps the models in the folder shared with PhotoCraft, `~/Library/Application Support/ArtCraft/models/<model-id>/` (`dsrnet-s`, `dsrnet-l`, `dsrnet-l-nature`), like SAM 3 ([shared-sam3.md](shared-sam3.md)); `LIGHTCRAFT_REFLECTION_MODELS` overrides it, and off the Mac the official `<config>/reflection-models` is used. The installer only creates `<model-id>` there and refuses when it exists: never move a model back, delete the ArtCraft folder or replace a model folder in it.

Licences (checked 2026-11-02):
- DSRNet code (mingcv/DSRNet, ICCV 2023): Apache-2.0. `crates/reflections/src/net.rs` is a modified Rust port of it (NOTICE says so); the crate is Apache-2.0 only.
- DSRNet weights: no licence of their own. DSRNet-S (`weights/dsrnet_s_epoch14.pt`, 40 MB) sits inside the Apache-2.0 repository; L and L Nature are on the author's Google Drive and Baidu pages only. They were trained on Pascal VOC, Zhang et al., IBCLN and SIR² data, which have their own research terms. Never committed or shipped; the user fetches them.
- VGG19 features (loss network inside the model): torchvision code is BSD-3-Clause, the ImageNet weights (`download.pytorch.org/models/vgg19-dcbb9e9d.pth`) carry no licence of their own and follow ImageNet's research terms. Never committed or shipped.
- Dependencies: `candle-core` / `candle-nn` 0.9.2 (Apache-2.0 / MIT, already used by SAM 3), built with `default-features = false`: no Accelerate, MKL or CUDA, so no C or C++ in the product; `cargo tree` shows no `-sys` crate. The only other new dependency edges are the workspace's own `serde`, `serde_json` and `sha2`. Metal is reached through the pure-Rust `objc2-metal` bindings.
- The conversion script's Python packages (`torch`, `safetensors`, `gdown`) are for the one-time conversion only and are not part of the app.

## Branch

`feat/25-reflection-removal`

## Own files

`crates/reflections/` (the DSRNet network, manifest and weight checks), `crates/engine/src/reflections.rs` (the correction grid, its use in a render, the model folder), `crates/engine/src/reflections/shared_models.rs` (the shared folder lookup and its tests, ours), `crates/ui-egui/src/reflections.rs` (the Remove panel section and the commands), `docs/ai-reflections.md`, `tools/convert-reflection-model.py`, `tools/requirements-reflection-convert.txt`

## Hooks in official files

- `Cargo.toml` and `Cargo.lock` (the workspace member), `crates/engine/Cargo.toml` (the dependency), `xtask/src/layers.rs` (the crate's layer), `NOTICE`.
- `crates/develop/src/settings.rs` (`Reflection` settings, absent from the file while default), `crates/develop/src/controls.rs` (the `reflection.*` controls and the rule that keeps Left < Right and Top < Bottom).
- `crates/engine/src/lib.rs` (`pub mod reflections;`) and `crates/engine/src/media/render.rs` (`RenderJob::run` applies the correction to the source before the pipeline, so view, thumbnails and export agree).
- `crates/ui-egui/src/lib.rs` (`pub mod reflections;` and four state fields), `app.rs` (their start values), `menus/ui_commands.rs`, `menus/run.rs` and `menus.rs` (the commands, their dispatch and `ui_enabled`), `panels/right/crop.rs` (one call at the end of `remove`), `icons.rs` (`Icon::Reflection`), `crates/ui-egui/locales/*.json` (20 messages; fi, id, sv and vi were added here because they shipped after the PR), `tests/masking.rs` (two tests).
- `README.md`, `docs/gaps.md` (row 12) and `docs/parity-checklist.md` (the LR-REM-REFLECT row, ⬜ → 🟡), both edited in place.
- When official splits or moves one of these files again (it did before this merge: `media.rs`, `menus.rs`, `lib.rs` and `panels/right.rs` became folders), move the hook with the code.

## Tests

Commands in `fork/tests.d/reflection-removal.txt`:

`cargo test -p lightcraft-reflections`, `cargo test -p lightcraft-engine --lib -- reflections`, `cargo test -p lightcraft-ui-egui --lib -- reflection`. None needs a model or a photo (the correction grids are procedural). The real model was run by hand on a CC0 photo (see the PR).

## When upstream merges

Official may merge #1220 as it is, or a newer version of it (the PR is open and conflicts with `main`). Then:
1. Merge `origin/main`. Official's own files for the feature (`crates/reflections/`, `crates/engine/src/reflections.rs`, `crates/ui-egui/src/reflections.rs`, the settings and controls, the locales) take over from ours, except the lines below.
2. Keep our shared folder: `models_dir()` in `crates/engine/src/reflections.rs` must still call `shared_models::models_dir` (official's version reads only `LIGHTCRAFT_REFLECTION_MODELS` and `<config>/reflection-models`), and `crates/engine/src/reflections/shared_models.rs` with its `mod shared_models;` line stays. If official adds its own model download or a different folder, point it at `ArtCraft/models/<model-id>` and ask before downloading.
3. Keep the four extra translations (fi, id, sv, vi) if official's catalogs lack them: `reflections::tests::reflection_messages_exist_in_every_shipped_language` fails otherwise.
4. Drop the Cargo/NOTICE/doc duplicates, re-run `cargo xtask parity --write`, then this feature's tests. When nothing of ours is left, delete this file and its line in `fork/tests.sh`.

## What's New entry

### Remove: Reflections
- The Remove panel has a Reflections section (also Photo ▸ Remove Reflections) that separates a reflection, such as a window's, from the photo with a local DSRNet model (S, L or L Nature) at Preview, Standard or High quality. Nothing leaves the Mac. Amount, Protect Sky, Detail Protection and Limit to Area change the result without running the model again. It is the official experimental feature, taken early.
- The model is not bundled: convert it once as `docs/ai-reflections.md` says, then pick its folder with Install Reflection Model…. It is kept in `~/Library/Application Support/ArtCraft/models/<model-id>` (shared with PhotoCraft).

## Models compared (2026-11-02, handoff)

All kept in `~/Library/Application Support/ArtCraft/models/<id>/`; none is committed. Never delete them: techneissa decides.

| Id | Source | Files, size | Licence of the code / weights | Status |
|---|---|---|---|---|
| `dsrnet-s` | github.com/mingcv/DSRNet `weights/dsrnet_s_epoch14.pt`, converted once with `tools/convert-reflection-model.py` | `model.safetensors` 89 MB | Apache-2.0 code; weights have no licence of their own | ported (PR #1220) |
| `dirs-mugi` | huggingface.co/huqiming513/DIRS-Models `checkpoints/dirs_mugi_lors.ckpt` and `checkpoints/pretrained/swin_large_o365.pth` | 345,268,909 B + 784,900,594 B | Hugging Face card: Apache-2.0; the GitHub repo (mingcv/DIRS) has no licence file | **ported, matches PyTorch** |
| `dirs-pair` | same repo: `checkpoints/dirs_pair_lors.ckpt` (+ the same Swin file) | 561,054,629 B + 784,900,594 B | as above | downloaded, **not ported yet** (see below) |
| `rdnet` | huggingface.co/spaces/lime-j/RDNet: `fp16_check.pt`, `classifier_32.pt` | about 534 MB + 198 MB | not checked yet | downloaded, **not ported yet** |
| `unreflectanything` | huggingface.co/AlbeRota/UnReflectAnything `weights/`: `full_model_weights.pt`, `diffuse_decoder.pt`, `highlight_decoder.pt` | 3,441,741,454 B + 418,170,913 B + 54,845,081 B | MIT (model card) | downloaded, **not ported yet** |

What a port of each still needs:
- **DIRS-PAIR**: reuses `swin.rs` and the dual-stream blocks. New: `DIRSLocalInteractionBlock` (the MuGI block with a plain `x1*y2` gate, no sigmoid), `DualAttentionInteractiveBlock` (window self-attention on each stream plus a "layered" cross attention over the two streams' concatenated windows, relative-position table of `(2w-1)^2*3` rows, window 12, no shift), the five-stage extractor with `enc_blk_nums [12,8,4,2,2]`, `dec_blk_nums [2,2,2,2,2]`, and the backbone wiring in `xreflection/archs/dirs_pair_arch.py` lines 330-547 and `dirs_lors_arch.py` (`DIRS_PAIR_LORS`, output is again `out_l`). Checkpoint keys: `net_g.backbone.*`, Lightning `state_dict`.
- **RDNet**: `models/arch/RDnet_.py` (`FullNet_NLP`, channels `[64,128,256,512]`, layers `[2,2,4,2]`, 4 subnets, run the non-reversible path), a FocalNet-L backbone (`focalnet.py`, `focalnet_L_384_22k_fl4`), NAFBlock/Decoder/UpSampleConvnext (`modules_sig.py`, `decode.py`), and the classifier `PretrainedConvNext("convnext_small_in22k")` (`classifier.py`) whose 6-number output prompts the network. Weights are stored fp16 under key `icnn`. Result: `output_j[-1][:, 3:]`, clamped to [0, 1]. Input is resized to a multiple of 32.
- **UnReflectAnything**: DINOv3 ViT-L/16 encoder (RoPE) + reflection predictor + token inpainter + DPT-style decoder. The code is in the PyPI package `unreflectanything` (not on the Hugging Face repo); `configs/pretrained_config.yaml` there names the layout. It removes specular highlights on shiny objects (endoscopy, glossy products), not window reflections.

## Comparison runs

Test photos and their sources: `$WORKSPACE/assets/lightcraft/design/reflection-tests/` (`sources.md`). Sheets: `$WORKSPACE/assets/lightcraft/shots/reflection-compare/`, also on `pr-assets` in `feat/25-reflection-removal/compare/`. Standard quality (long edge 768), Metal, M4 Pro; time = one inference after the model is loaded.

| Photo | DSRNet-S 1st / 2nd run | DIRS-MuGI 1st / 2nd run |
|---|---|---|
| car-window | 23.7 s / 24.7 s | 19.4 s / 19.0 s |
| display-case | 19.6 s / 19.2 s | 14.5 s / 14.6 s |
| eyeglasses | 19.6 s / 19.7 s | 15.1 s / 15.0 s |
| framed-picture-glass | 20.8 s / 20.8 s | 16.2 s / 16.3 s |
| glass-door | 23.1 s / 22.7 s | 17.8 s / 17.6 s |
| shiny-car-or-skin | 21.7 s / 21.7 s | 16.8 s / 16.3 s |
| shiny-jewellery | 21.5 s / 21.5 s | 16.5 s / 16.7 s |
| shop-window-day | 22.6 s / 22.7 s | 17.1 s / 17.2 s |
| water-surface | 25.1 s / 25.1 s | 19.8 s / 19.8 s |
| window-night | 22.5 s / 28.9 s | 17.3 s / 17.5 s |

Measured on an Apple M4 Pro with other lanes running (load about 45), so expect ±10 %. App memory (`/usr/bin/time -l`, host RAM only, GPU buffers not counted): DSRNet-S about 250 MB, DIRS-MuGI about 1.25 GB. DIRS-PAIR, RDNet and UnReflectAnything are not ported yet, so they have no column.
