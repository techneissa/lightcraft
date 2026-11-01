# Reflection removal

## What it does

The Remove panel gets a **Reflections** section (also Photo ▸ Remove Reflections) that separates a reflection, such as a shop window's, from the photo with a local DSRNet model: DSRNet-S, DSRNet-L or DSRNet-L Nature, at Preview (384 px), Standard (768 px) or High (1024 px). Nothing leaves the Mac. Amount (default 35 %), Protect Sky, Detail Protection (default 60 %) and Limit to Area change the result without running the model again; changing the model or the quality needs Recalculate. The correction is a small grid saved beside the library settings (`reflection-cache`), never in the photo. Mac only (Candle on Metal, CPU as the fallback). Issue #25.

This is official PR storytold/lightcraft #1220 ("Add experimental local DSRNet reflection removal", d4nnyp73), taken early with its commits kept. Its own notes: [docs/ai-reflections.md](../docs/ai-reflections.md).

Commands: `photo.removeReflections`, `reflection.apply {model?, quality?}` (both return at once; the work runs on a thread with a "Reflection Removal" row in the activity stack), `reflection.toggle`, `reflection.reset` (undoable), `reflection.status`, `reflection.install {path?}`, `reflection.models`, `reflection.panel`, `reflection.instructions`; controls `reflection.amount`, `reflection.detailProtection`, `reflection.left` / `top` / `right` / `bottom`.

The model is not bundled and the app does not download it. It is converted once from the author's checkpoint (`tools/convert-reflection-model.py`, needs Python with torch), then **Install Reflection Model…** copies the folder (`manifest.json` + `model.safetensors`, checked against its SHA-256) into the models folder. The fork keeps the models in the folder shared with PhotoCraft, `~/Library/Application Support/ArtCraft/models/<model-id>/` (`dsrnet-s`, `dsrnet-l`, `dsrnet-l-nature`), like SAM 3 ([shared-sam3.md](shared-sam3.md)); `LIGHTCRAFT_REFLECTION_MODELS` overrides it, and off the Mac the official `<config>/reflection-models` is used. The installer only creates `<model-id>` there and refuses when it exists: never move a model back, delete the ArtCraft folder or replace a model folder in it.

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
- `README.md`, `docs/gaps.md`, `docs/parity-checklist.md` (LR-REM-REFLECT 🟡), `docs/whats-new.md`.
- When official splits or moves one of these files again (it did before this merge: `media.rs`, `menus.rs`, `lib.rs` and `panels/right.rs` became folders), move the hook with the code.

## Tests

`cargo test -p lightcraft-reflections`, `cargo test -p lightcraft-engine --lib -- reflections`, `cargo test -p lightcraft-ui-egui --lib -- reflection`. None needs a model or a photo (the correction grids are procedural). The real model was run by hand on a CC0 photo (see the PR).

## When upstream merges

Official may merge #1220 as it is, or a newer version of it (the PR is open and conflicts with `main`). Then:
1. Merge `origin/main`. Official's own files for the feature (`crates/reflections/`, `crates/engine/src/reflections.rs`, `crates/ui-egui/src/reflections.rs`, the settings and controls, the locales) take over from ours, except the lines below.
2. Keep our shared folder: `models_dir()` in `crates/engine/src/reflections.rs` must still call `shared_models::models_dir` (official's version reads only `LIGHTCRAFT_REFLECTION_MODELS` and `<config>/reflection-models`), and `crates/engine/src/reflections/shared_models.rs` with its `mod shared_models;` line stays. If official adds its own model download or a different folder, point it at `ArtCraft/models/<model-id>` and ask before downloading.
3. Keep the four extra translations (fi, id, sv, vi) if official's catalogs lack them: `reflections::tests::reflection_messages_exist_in_every_shipped_language` fails otherwise.
4. Drop the Cargo/NOTICE/doc duplicates, re-run `cargo xtask parity --write`, then this feature's tests. When nothing of ours is left, delete this file and its line in `fork/tests.sh`.
