# Reflection removal

List: Remove ▸ Reflections separates a window's reflection with a local model (DIRS-MuGI, DIRS-PAIR, RDNet, or DSRNet from official PR #1220), kept in the folder shared with PhotoCraft. Branch `feat/25-reflection-removal`.

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

## Models compared (2026-11-02)

All kept in `~/Library/Application Support/ArtCraft/models/<id>/`; none is committed. Never delete them: techneissa decides.
Every model is read as published (no conversion), by candle's non-executing pickle reader, and matches PyTorch on a 192×160 picture with the real weights.

| Id | Source | Files, size | Licence | Status | Max difference to PyTorch (CPU / Metal) |
|---|---|---|---|---|---|
| `dsrnet-s` | github.com/mingcv/DSRNet, converted once with `tools/convert-reflection-model.py` | 89 MB | Apache-2.0 code; weights have none of their own | ported (PR #1220) | n/a |
| `dirs-mugi` | huggingface.co/huqiming513/DIRS-Models | 345 MB + 785 MB | card: Apache-2.0; GitHub repo has no licence file | ported | 2.5e-6 / 4.3e-6 |
| `dirs-pair` | same repo, `dirs_pair_lors.ckpt` + the same Swin file | 561 MB + 785 MB | as above | ported (`dirs/pair.rs`) | 1.8e-6 / 2.6e-6 |
| `rdnet` | huggingface.co/spaces/lime-j/RDNet `fp16_check.pt`, `classifier_32.pt` | 533 MB + 198 MB | none stated (no licence file in the Space or in github.com/lime-j/RDNet) | ported (`rdnet.rs`, `focal.rs`) | 2.0e-6 / 2.3e-6 |
| `unreflectanything` | huggingface.co/AlbeRota/UnReflectAnything `weights/` | 3.4 GB + 418 MB + 55 MB | MIT (model card) | downloaded, **not ported** | n/a |

Download… in the panel works for DIRS-MuGI, DIRS-PAIR and RDNet (asks first, resume, SHA-256 pinned from Hugging Face's own records). The Model dropdown lists installed models, then "Download <name>…" for the downloadable ones that are missing.

RDNet notes: the Space runs inputs resized down to a multiple of 32; this port pads by edge replication instead (the same in the PyTorch reference used for the check). Only the decoder branch that gives the reflection is run, once, after the last sub-network. The Space's own `app.py` feeds 0-255 values; the port feeds 0-1, as the authors' dataset code does.

### UnReflectAnything: not ported yet (stopped for context, not because it cannot be done)
- Needs: DINOv3 ViT-L/16 encoder with RoPE (24 layers at 896 px, from `facebook/dinov3-vitl16-pretrain-lvd1689m`), two DPT decoders (diffuse and highlight), the `TokenInpainter_Prior` (6 blocks, 16 heads, local prior blending) and the mask logic of `inference.py` (brightness threshold 0.7, mask dilation 11). Code: github.com/alberto-rota/UnReflectAnything (`models.py` 1660 lines, `token_inpainters.py`, `utilities/inference.py`); layout in `config/pretrained_config.yaml`.
- `full_model_weights.pt` is a pickle holding a `dotmap.DotMap` object next to the tensors: PyTorch's `weights_only` loader refuses it. Candle's reader must be tested on it; if it refuses too, a small restricted unpickler is needed (never run the pickle).
- It removes highlights on shiny objects (endoscopy, glossy products), so it needs its own "Highlights" mode in the panel.

## Comparison runs

Test photos and their sources: `$WORKSPACE/assets/lightcraft/design/reflection-tests/` (`sources.md`): 8 window and glass reflections and 3 shiny objects, each looked at before it was kept. Sheets: `$WORKSPACE/assets/lightcraft/shots/reflection-compare/`, also on `pr-assets` in `feat/25-reflection-removal/compare/`. Standard quality (long edge 768), Metal, M4 Pro, other lanes running (load about 25); time = one inference after the model is loaded. 
| Photo | DSRNet-S 1st / 2nd run | DIRS-MuGI 1st / 2nd run | DIRS-PAIR 1st / 2nd run | RDNet 1st / 2nd run |
|---|---|---|---|---|
| amsterdam-glass-wall | 22.4 s / 22.3 s | 17.4 s / 17.6 s | 25.3 s / 25.3 s | 42.7 s / 43.1 s |
| capri-car-paint | 17.7 s / 17.9 s | 13.6 s / 13.6 s | 20.1 s / 20.1 s | 33.5 s / 33.6 s |
| car-window | 23.8 s / 23.7 s | 18.4 s / 18.4 s | 27.3 s / 27.6 s | 45.2 s / 45.0 s |
| designer-apparel-window | 23.7 s / 23.6 s | 18.3 s / 18.3 s | 27.2 s / 27.2 s | 45.2 s / 46.0 s |
| eyeglasses | 19.6 s / 20.6 s | 15.5 s / 15.6 s | 22.6 s / 22.5 s | 37.5 s / 37.5 s |
| framed-picture-glass | 21.0 s / 21.1 s | 16.2 s / 16.2 s | 24.1 s / 24.2 s | 40.0 s / 40.0 s |
| glass-door | 22.3 s / 22.2 s | 17.1 s / 17.1 s | 25.1 s / 27.4 s | 54.4 s / 55.2 s |
| red-car-body | 25.8 s / 19.6 s | 15.3 s / 19.5 s | 33.6 s / 31.0 s | 43.6 s / 43.9 s |
| shop-window-day | 26.7 s / 24.1 s | 24.2 s / 21.5 s | 33.9 s / 33.2 s | 43.4 s / 43.0 s |
| willys-coupe | 22.4 s / 22.3 s | 17.1 s / 17.1 s | 25.1 s / 25.1 s | 42.6 s / 42.5 s |
| window-night | 22.6 s / 22.6 s | 17.4 s / 17.4 s | 35.3 s / 25.4 s | 43.1 s / 43.2 s |

App memory (`/usr/bin/time -l`, host RAM only, GPU buffers not counted), mean over the photos: DSRNet-S 247 MB, DIRS-MuGI 1262 MB, DIRS-PAIR 1452 MB, RDNet 1347 MB. A few runs show a first/second difference from other work on the Mac (red-car-body, shop-window-day, window-night), not from the model.
