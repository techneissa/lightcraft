# AI models on the Neural Engine (Core ML)

List: Learn My Edits' image model runs through Apple's Core ML on the Neural Engine, about 3–5× faster than candle on the GPU, once its Core ML file is downloaded (asked first, 43 MB); candle stays the fallback, and Settings ▸ Performance ▸ AI Models picks Automatic, Core ML or Candle (issue #105). Branch `perf/105-coreml`.

## What it does

- **`crates/coreml`** (`lightcraft-coreml`, layer 0): Core ML through objc2, like `crates/macraw` reaches Core Image. `load_package` compiles an `.mlpackage` into a `.mlmodelc` next to it, keyed by the package's `Manifest.json` (`<name>.mlmodelc.key`: a new model compiles again), and compiles again once when a cached copy no longer loads; `Model::load` (compute units: all, CPU + Neural Engine, CPU + GPU, CPU) and `Model::predict` run `f32` tensors. All `unsafe` is in `src/macos.rs`, with `// SAFETY:` on every block; every failure is an `Error`, never a panic; off the Mac a stub returns "unavailable". No Objective-C exceptions are caught (pure Rust), so every selector is checked first and every nil handled (its README lists the calls).
- **The first model: DINOv2-small**, Learn My Edits' image encoder. It is merged, small (88 MB) and a plain ViT, the kind the Neural Engine runs best, so it is the cheapest proof. The people-parsing SegFormer (PR #92), BiRefNet, LaMa, the face models and SAM 3 are follow-ups.
- **Conversion, once, by hand** (`tools/convert-coreml-dinov2.py`, versions in `tools/requirements-coreml-convert.txt`): the same network written again in PyTorch from the published tensor names, with candle's exact 224 × 224 input and 37 × 37 → 16 × 16 position resample, converted to an fp16 ML program. It reads the `model.safetensors` already in `~/Library/Application Support/ArtCraft/models/dinov2-small/` (never a second download) and can install `coreml/dinov2-small.mlpackage` next to it (43 MB). The app compiles it there once, into `coreml/dinov2-small.mlmodelc` (about 2 s, at the first job).
- **Download** (`crates/engine/src/model_runtime/download.rs`): converted files of small models (under 2 GB) whose licence allows redistribution are published once, zipped (stored), on the `models-coreml-1` release of techneissa/lightcraft. Settings ▸ Performance ▸ AI Models shows "Download Core ML Model (43 MB)…" on a Mac without it, asks "Download the Core ML model (43 MB) from github.com?", and downloads with a row in the activity stack (`models.downloadCoreml {model, confirm: true}` for agents; refused without `confirm`). The zip's size and SHA-256 are checked like LaMa's, it is unpacked into the model's `coreml/` folder in the shared models folder (an entry outside the package, a link, too many files or bytes: nothing is installed) and deleted. Without the release the download fails with a message and the model stays on candle; the app never converts anything.
- **Runtime per model** (`crates/engine/src/model_runtime.rs`, kept in `<config>/model-runtimes.json`): Automatic (default: Core ML when its model is there and loads, else candle; candle also takes over for good when Core ML fails on a photo), Core ML (an error when it can't run), Candle. Settings ▸ Performance ▸ AI Models has one row per model; `models.runtime {model, runtime}` sets it, `models.runtime {}` lists it with `coremlAvailable`, `coremlInstalled` and the `backend` in use; `learn.status` carries `runtime`, `coremlInstalled`, `backend`.

## Models and licences

| Model | Core ML file | Licence | Bytes | SHA-256 |
| --- | --- | --- | ---: | --- |
| DINOv2-small (`facebook/dinov2-small`, revision `ed25f3a`), Learn My Edits | `models-coreml-1/dinov2-small.mlpackage.zip` | Apache-2.0 | 43,397,268 | `c69bd095b956fd60e74b726e7d0ec85417ef0c1f0ced52ee9a6d162091e4eaed` |

Measured on the M4 Pro (macOS 27.0.1), five 384 × 256 photos × 4 rounds after a warm-up, other work running (load 57–83), `cargo run --release -p lightcraft-learn --example coreml_speed -- <shared dinov2-small folder>`:

| Runtime | Load (ms) | Per photo (ms) |
| --- | ---: | ---: |
| candle, GPU (Metal), today's path | 59–101 | 12.4–20.2 |
| candle, CPU | 11–26 | 237–1146 |
| Core ML, all units | 37 (1911 the first time: compile) | 4.0–4.9 |
| Core ML, CPU + Neural Engine | 17–42 | 4.0–4.3 |

Core ML against candle on the test photo: cosine 0.99989, largest difference 0.11 on values up to 14 (fp16 against fp32).

## Branch

`perf/105-coreml`

## Own files

`crates/coreml/` (all), `crates/engine/src/model_runtime/download.rs`, `crates/engine/src/model_runtime/download_tests.rs`, `crates/learn/src/image_encoder.rs`, `crates/learn/src/image_encoder_tests.rs`, `crates/learn/examples/coreml_speed.rs`, `crates/engine/src/model_runtime.rs`, `crates/engine/src/cmd/model_runtime.rs`, `crates/ui-egui/src/dialogs/settings/model_runtime.rs`, `tools/convert-coreml-dinov2.py`, `tools/requirements-coreml-convert.txt`, `fork/tests.d/coreml.txt`

## Hooks in official files

`Cargo.toml` (`lightcraft-coreml` path), `Cargo.lock` (`objc2-core-ml` 0.3.2), `xtask/src/layers.rs` (one `TABLE` row), the `unsafe` bullet in `AGENTS.md` and `CLAUDE.md`. Fork files from Learn My Edits: `crates/learn/Cargo.toml` (dependency), `crates/learn/src/lib.rs` (`mod image_encoder`, the `ImageFeatures` trait; `photo_features` takes any encoder), `crates/learn/src/encoder.rs` (`pixels` and `run` split out of `features`), `crates/engine/src/learn.rs` (the encoder is an `ImageEncoder`; `runtime`, `set_runtime`, `coreml_installed`, `backend`; `with_default_learn` reads the runtime), `crates/engine/src/cmd/learn.rs` (three `learn.status` fields). `crates/engine/Cargo.toml` (`lightcraft-coreml`, `zip` 7.4 without default features, `sha2` for a test), `crates/engine/src/lib.rs` (`mod model_runtime`, `Session::model_runtimes`), `crates/engine/src/cmd.rs` (`mod` + `specs` line), `crates/ui-egui/src/dialogs/settings.rs` (`mod` line), `crates/ui-egui/src/dialogs/settings/performance.rs` (one call at the end).

## Tests

```bash
cargo test -p lightcraft-coreml
cargo test -p lightcraft-learn --lib -- image_encoder
cargo test -p lightcraft-engine --lib -- model_runtime
```

The model tests read `model.safetensors` and `coreml/dinov2-small.mlpackage` from the shared folder through links in a temporary folder (nothing is written there) and skip with `skip!(Model, …)` without them: Core ML equals candle within tolerance on one photo; a missing or damaged Core ML model falls back to candle with the same numbers. Without any model, loading is an error, not a panic (runs everywhere). The Core ML crate's own tests feed it a damaged compiled model and package (an error from Core ML, no crash), an output shape whose size overflows, and a changed manifest (a new cache key). The download tests run against a local HTTP server and a tiny zip made in the test, never a real download: a checked zip unpacks and replaces the old package; a wrong hash or an entry outside the package installs nothing. A runtime choice that can't be saved is not applied; the download is refused without `confirm`.

## What's New entry

### Learn My Edits Runs on the Neural Engine
On a Mac, Learn My Edits' image model can run through Core ML on the Neural Engine, about 3–5× faster per photo: Settings ▸ Performance ▸ AI Models downloads its Core ML file (43 MB, asked first) and picks Automatic, Core ML or Candle.
