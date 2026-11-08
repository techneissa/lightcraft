# AI models on the Neural Engine (Core ML)

List: Learn My Edits' image model runs through Apple's Core ML on the Neural Engine when its converted model is in the shared models folder, about 3–5× faster than candle on the GPU; candle stays the fallback, and Settings ▸ Performance ▸ AI Models picks Automatic, Core ML or Candle (issue #105). Branch `perf/105-coreml`.

## What it does

- **`crates/coreml`** (`lightcraft-coreml`, layer 0): Core ML through objc2, like `crates/macraw` reaches Core Image. `compile` turns an `.mlpackage` into a compiled `.mlmodelc` once; `Model::load` (compute units: all, CPU + Neural Engine, CPU + GPU, CPU) and `Model::predict` run `f32` tensors. All `unsafe` is in `src/macos.rs`, with `// SAFETY:` on every block; every failure is an `Error`, never a panic; off the Mac a stub returns "unavailable". No Objective-C exceptions are caught (pure Rust), so every selector is checked first and every nil handled (its README lists the calls).
- **The first model: DINOv2-small**, Learn My Edits' image encoder. It is merged, small (88 MB) and a plain ViT, the kind the Neural Engine runs best, so it is the cheapest proof. The people-parsing SegFormer (PR #92) is not merged yet; it and the bigger models (BiRefNet, LaMa, the face models, SAM 3) are follow-ups.
- **Conversion, once, by hand** (`tools/convert-coreml-dinov2.py`, versions in `tools/requirements-coreml-convert.txt`): the same network written again in PyTorch from the published tensor names, with candle's exact 224 × 224 input and 37 × 37 → 16 × 16 position resample, converted to an fp16 ML program. It reads the `model.safetensors` already in `~/Library/Application Support/ArtCraft/models/dinov2-small/` (never a second download) and installs `coreml/dinov2-small.mlpackage` next to it (43 MB). The app compiles it there once, into `coreml/dinov2-small.mlmodelc` (about 2 s, at the first job), and recompiles when the package is newer.
- **Runtime per model** (`crates/engine/src/model_runtime.rs`, kept in `<config>/model-runtimes.json`): Automatic (default: Core ML when its model is there and loads, else candle; candle also takes over for good when Core ML fails on a photo), Core ML (an error when it can't run), Candle. Settings ▸ Performance ▸ AI Models has one row per model; `models.runtime {model, runtime}` sets it, `models.runtime {}` lists it with `coremlAvailable`, `coremlInstalled` and the `backend` in use; `learn.status` carries `runtime`, `coremlInstalled`, `backend`.
- **Not yet: the download.** The app does not download the Core ML file: it has no home on GitHub or Hugging Face yet. Until it has, the script installs it on this Mac.

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

`crates/coreml/` (all), `crates/learn/src/image_encoder.rs`, `crates/learn/src/image_encoder_tests.rs`, `crates/learn/examples/coreml_speed.rs`, `crates/engine/src/model_runtime.rs`, `crates/engine/src/cmd/model_runtime.rs`, `crates/ui-egui/src/dialogs/settings/model_runtime.rs`, `tools/convert-coreml-dinov2.py`, `tools/requirements-coreml-convert.txt`, `fork/tests.d/coreml.txt`

## Hooks in official files

`Cargo.toml` (`lightcraft-coreml` path), `Cargo.lock` (`objc2-core-ml` 0.3.2), `xtask/src/layers.rs` (one `TABLE` row), the `unsafe` bullet in `AGENTS.md` and `CLAUDE.md`. Fork files from Learn My Edits: `crates/learn/Cargo.toml` (dependency), `crates/learn/src/lib.rs` (`mod image_encoder`, the `ImageFeatures` trait; `photo_features` takes any encoder), `crates/learn/src/encoder.rs` (`pixels` and `run` split out of `features`), `crates/engine/src/learn.rs` (the encoder is an `ImageEncoder`; `runtime`, `set_runtime`, `coreml_installed`, `backend`; `with_default_learn` reads the runtime), `crates/engine/src/cmd/learn.rs` (three `learn.status` fields). `crates/engine/Cargo.toml` (dependency), `crates/engine/src/lib.rs` (`mod model_runtime`, `Session::model_runtimes`), `crates/engine/src/cmd.rs` (`mod` + `specs` line), `crates/ui-egui/src/dialogs/settings.rs` (`mod` line), `crates/ui-egui/src/dialogs/settings/performance.rs` (one call at the end).

## Tests

```bash
cargo test -p lightcraft-coreml
cargo test -p lightcraft-learn --lib -- image_encoder
cargo test -p lightcraft-engine --lib -- cmd::model_runtime
```

The model tests read `model.safetensors` and `coreml/dinov2-small.mlpackage` from the shared folder through links in a temporary folder (nothing is written there) and skip with `skip!(Model, …)` without them: Core ML equals candle within tolerance on one photo; a missing or damaged Core ML model falls back to candle with the same numbers. Without any model, loading is an error, not a panic (runs everywhere). The Core ML crate's own tests feed it a damaged compiled model and package (an error from Core ML, no crash).

## What's New entry

### Learn My Edits Runs on the Neural Engine
On a Mac with the converted model installed, Learn My Edits' image model runs through Core ML on the Neural Engine, about 3–5× faster per photo. Settings ▸ Performance ▸ AI Models picks Automatic, Core ML or Candle.
