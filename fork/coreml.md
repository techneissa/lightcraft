# AI models on the Neural Engine (Core ML)

List: Learn My Edits' image model runs through Apple's Core ML on the Neural Engine, about 3–5× faster than candle on the GPU, once its Core ML file is downloaded (asked first, 43 MB); candle stays the fallback, and Settings ▸ Performance ▸ AI Models picks Automatic, Core ML or Candle (issue #105); the people-parts SegFormer face and clothes parsers run through Core ML on the GPU too, about 5–8× faster per photo (converted locally, not published). Branches `perf/105-coreml`, `perf/coreml-parsing`.

## What it does

- **`crates/coreml`** (`lightcraft-coreml`, layer 0): Core ML through objc2, like `crates/macraw` reaches Core Image. `load_package` compiles an `.mlpackage` into a `.mlmodelc` next to it, keyed by the package's `Manifest.json` (`<name>.mlmodelc.key`: a new model compiles again), and compiles again once when a cached copy no longer loads; `Model::load` (compute units: all, CPU + Neural Engine, CPU + GPU, CPU) and `Model::predict` run `f32` tensors. All `unsafe` is in `src/macos.rs`, with `// SAFETY:` on every block; every failure is an `Error`, never a panic; off the Mac a stub returns "unavailable". No Objective-C exceptions are caught (pure Rust), so every selector is checked first and every nil handled (its README lists the calls).
- **People parts: SegFormer-B0 (faces) and SegFormer-B3 (clothes)** (`crates/parsing/src/coreml.rs`, `tools/convert-coreml-parsing.py`), used by the SegFormer and Sapiens2 sets. The converter writes the same networks again in PyTorch from the published tensor names (it matches Hugging Face's `SegformerForSemanticSegmentation` to 4 decimals) and converts them to fp16 ML programs with fp16 outputs; packages go in `<models>/<folder>/coreml/<folder>.mlpackage` in the shared models folder, compiled once by the app beside them. The face parser takes 1 to 8 crops of 512² per call and gives the ten face groups of `face.rs` at 512² (resize, softmax and group sums inside). The clothes parser's input size changes with the photo (long side 768 to 1536), so it takes the photo's grid in the top-left of one of 28 canvases (each long side 768, 1024, 1280, 1536; short sides 1, ¾, ⅔ and ½ of it; both ways round) with a fourth channel marking the photo, and every layer ignores the padding: convolutions see zeros past the photo, attention gives the padding's keys −10⁴, and the decoder's resizes divide the resized masked map by the resized mask (the edge clamp of a resize of the photo alone). In float32 the canvas run equals the run alone to 1.2·10⁻⁶. Both run on the **GPU** (`CpuAndGpu`): the Neural Engine compiler ran over 15 minutes on one 1536 × 1024 clothes canvas without finishing (its first attention is 98,304 × 1,536), and for the face parser the GPU is as fast (11 against 12 ms a face) and about ten times closer to float32 (a made-up crop's person mask moved 0.4–1.4 % on the Neural Engine, 0.03–0.09 % on the GPU). Runtime rows: `face-parsing-segformer-b0` ("People Parts: Faces") and `human-parsing-segformer-b3` ("People Parts: Bodies"); `PeopleParsing::set_runtime` drops the loaded set. BiSeNet, SegFormer-B2, the face mesh and Sapiens2 itself stay on candle.
- **The first model: DINOv2-small**, Learn My Edits' image encoder. It is merged, small (88 MB) and a plain ViT, the kind the Neural Engine runs best, so it is the cheapest proof. The people-parsing SegFormer (PR #92), BiRefNet, LaMa, the face models and SAM 3 are follow-ups.
- **Conversion, once, by hand** (`tools/convert-coreml-dinov2.py`, versions in `tools/requirements-coreml-convert.txt`): the same network written again in PyTorch from the published tensor names, with candle's exact 224 × 224 input and 37 × 37 → 16 × 16 position resample, converted to an fp16 ML program. It reads the `model.safetensors` already in `~/Library/Application Support/ArtCraft/models/dinov2-small/` (never a second download) and can install `coreml/dinov2-small.mlpackage` next to it (43 MB). The app compiles it there once, into `coreml/dinov2-small.mlmodelc` (about 2 s, at the first job).
- **Download** (`crates/engine/src/model_runtime/download.rs`): converted files of small models (under 2 GB) whose licence allows redistribution are published once, zipped (stored), on the `models-coreml-1` release of techneissa/lightcraft. Settings ▸ Performance ▸ AI Models shows "Download Core ML Model (43 MB)…" on a Mac without it, asks "Download the Core ML model (43 MB) from github.com?", and downloads with a row in the activity stack (`models.downloadCoreml {model, confirm: true}` for agents; refused without `confirm`). The zip's size and SHA-256 are checked like LaMa's, it is unpacked into the model's `coreml/` folder in the shared models folder (an entry outside the package, a link, too many files or bytes: nothing is installed) and deleted. Without the release the download fails with a message and the model stays on candle; the app never converts anything.
- **Runtime per model** (`crates/engine/src/model_runtime.rs`, kept in `<config>/model-runtimes.json`): Automatic (default: Core ML when its model is there and loads, else candle; candle also takes over for good when Core ML fails on a photo), Core ML (an error when it can't run), Candle. Settings ▸ Performance ▸ AI Models has one row per model; `models.runtime {model, runtime}` sets it, `models.runtime {}` lists it with `coremlAvailable`, `coremlInstalled` and the `backend` in use; `learn.status` carries `runtime`, `coremlInstalled`, `backend`.

## Models and licences

| Model | Core ML file | Licence | Bytes | SHA-256 |
| --- | --- | --- | ---: | --- |
| DINOv2-small (`facebook/dinov2-small`, revision `ed25f3a`), Learn My Edits | `models-coreml-1/dinov2-small.mlpackage.zip` | Apache-2.0 | 43,397,268 | `c69bd095b956fd60e74b726e7d0ec85417ef0c1f0ced52ee9a6d162091e4eaed` |
| SegFormer-B0 face parser (`alexkrz/segformer-b0-celebamask_hq` @ `e1e6109`), People Parts | not published: local only (`weight.bin` 7,451,132 bytes) | "other": NVIDIA Source Code License for SegFormer (non-commercial use; redistribution only with the licence); trained on CelebAMask-HQ (non-commercial research) | — | — |
| SegFormer-B3 clothes parser (`sayeed99/segformer_b3_clothes` @ `6c12f0e`), People Parts | not published: local only (`weight.bin` 94,512,100 bytes) | MIT on the model card; its MiT-b3 encoder is under the NVIDIA Source Code License for SegFormer (non-commercial) | — | — |

Neither licence is on the house allowlist, so their converted files are not on the release: make them on the Mac with `tools/convert-coreml-parsing.py` (a minute each). Publishing them is techneissa's call; it would take uploading the zips and adding a `RELEASES` row each, and `models.downloadCoreml` learning their folders.

Measured on the M4 Pro (macOS 27.0.1), five 384 × 256 photos × 4 rounds after a warm-up, other work running (load 57–83), `cargo run --release -p lightcraft-learn --example coreml_speed -- <shared dinov2-small folder>`:

| Runtime | Load (ms) | Per photo (ms) |
| --- | ---: | ---: |
| candle, GPU (Metal), today's path | 59–101 | 12.4–20.2 |
| candle, CPU | 11–26 | 237–1146 |
| Core ML, all units | 37 (1911 the first time: compile) | 4.0–4.9 |
| Core ML, CPU + Neural Engine | 17–42 | 4.0–4.3 |

Core ML against candle on the test photo: cosine 0.99989, largest difference 0.11 on values up to 14 (fp16 against fp32).

People parts, SegFormer set, M4 Pro (macOS 27), other work running, one run each, `cargo run --release -p lightcraft-parsing --example coreml_compare -- <photo>` (photo at 2048 px; faces from YuNet). "First" is the first parse after loading (it holds Core ML's first call per input size, about 1 s each); "then" is the next parse of the same photo.

| Photo | Faces | candle, as the app ran it | candle, one face a call | Core ML, first | Core ML, then |
| --- | ---: | ---: | ---: | ---: | ---: |
| CC0 portrait (ChidoMpembaYoungAfricanWomanInPolitics, 1638 × 2048) | 1 | 2.4 s / 2.1 s | 1.8 s / 2.0 s | 1.6 s | 0.26 s |
| CC0 group (Miss World Canada 2012 group photograph, 960 × 720) | 26 | 9.0 s / 7.6 s | 14.2 s / 8.8 s | 3.3 s | 1.7 s |
| techneissa's A7 V (10261023/A7V04950, 2048 × 1365) | 1 | 1.9 s / 2.2 s | 1.6 s / 2.5 s | 1.0 s | 0.36 s |

On the group, Core ML's 1.5 s is 0.2 s clothes parser and 1.3 s faces: per 8 faces about 45 ms cutting the crops, 75–240 ms face parser and 170 ms face mesh (still candle), then the planes. The mesh is the next model worth converting.

Agreement of Core ML with candle (one face a call, the reference: see below), per part over every person, on the masks as the app draws them (guided filter, at 2048 px, on at 0.5): share of pixels the same / intersection over union.

| Part | CC0 portrait | CC0 group | A7 V |
| --- | --- | --- | --- |
| Entire Person | 100.00 % / 1.000 | 99.99 % / 0.999 | 100.00 % / 1.000 |
| Facial Skin | 100.00 % / 1.000 | 100.00 % / 0.999 | 100.00 % / 1.000 |
| Body Skin | 100.00 % / 0.999 | 100.00 % / 0.999 | 100.00 % / 1.000 |
| Hair | 100.00 % / 1.000 | 99.99 % / 0.997 | 100.00 % / 1.000 |
| Eyebrows | 100.00 % / 0.999 | 100.00 % / 1.000 | 100.00 % / 0.999 |
| Eye Whites | 100.00 % / 1.000 | — | 100.00 % / 1.000 |
| Iris | 100.00 % / 1.000 | 100.00 % / 1.000 | 100.00 % / 1.000 |
| Lips | 100.00 % / 0.999 | 100.00 % / 1.000 | 100.00 % / 0.999 |
| Clothes Top | 100.00 % / 1.000 | 99.99 % / 0.998 | 99.99 % / 0.999 |
| Clothes Bottom | — | 100.00 % / 1.000 | — |
| Shoes | — | 100.00 % / 0.999 | — |
| Hat | 100.00 % / 0.996 | — | — |
| Others | 100.00 % / 0.996 | — | — |

(— : no such part in that photo.)

Two candle 0.9.2 faults found on the way (not fixed here):
- On **Metal**, a batch of SegFormer-B0 face crops gives every crop after the first other numbers than the same crop alone (person share of a made-up crop: 0.327 in a batch of 3 against 0.339 alone; another 0.711 against 0.630; alone equals PyTorch). The app runs candle faces 4 at a time, so on candle 3 of every 4 faces get slightly different masks. Core ML has no such fault, and the reference above runs candle one face a call.
- On the **CPU**, candle's 4 × 4 stride-4 convolution of the face parser's stage 2 gives other numbers than PyTorch (its output's spread 1.96 against 6.46); Metal's matches. Only the CPU fallback (not a Mac) is hit.

**BiSeNet and Sapiens2.** BiSeNet (ResNet-18, fixed 512² crops) is a plain CNN the Neural Engine and GPU run well; converting it needs its network written in PyTorch from `crates/parsing/src/bisenet.rs` (MLX weights, kernels stored out × h × w × in) and a face-groups head in its class order, about a day with checks; its set's SegFormer-B2 clothes parser converts with the same script as B3 (a `MODELS` entry). Its weights are MIT, trained on CelebAMask-HQ (non-commercial research). Not done here. Sapiens2 0.4B is a ViT (813 MB in bf16) run on crops of each person (`crops.rs`): it needs `sapiens.rs` written in PyTorch at its fixed crop size, likely on the GPU (a long token sequence, as with B3); fp16 is about 800 MB, under the release's 2 GB. `models.rs` records its licence as allowing redistribution with the licence while forbidding surveillance, biometric processing and identifying people; it is not on the allowlist, so publishing a converted file would be techneissa's call.

## Branch

`perf/105-coreml`

## Own files

`crates/coreml/` (all), `crates/parsing/src/coreml.rs`, `crates/parsing/src/tests/coreml.rs`, `crates/parsing/examples/coreml_compare.rs`, `tools/convert-coreml-parsing.py`, `crates/engine/src/model_runtime/download.rs`, `crates/engine/src/model_runtime/download_tests.rs`, `crates/learn/src/image_encoder.rs`, `crates/learn/src/image_encoder_tests.rs`, `crates/learn/examples/coreml_speed.rs`, `crates/engine/src/model_runtime.rs`, `crates/engine/src/cmd/model_runtime.rs`, `crates/ui-egui/src/dialogs/settings/model_runtime.rs`, `tools/convert-coreml-dinov2.py`, `tools/requirements-coreml-convert.txt`, `fork/tests.d/coreml.txt`

## Hooks in official files

`Cargo.toml` (`lightcraft-coreml` path), `Cargo.lock` (`objc2-core-ml` 0.3.2), `xtask/src/layers.rs` (one `TABLE` row), the `unsafe` bullet in `AGENTS.md` and `CLAUDE.md`. Fork files from Learn My Edits: `crates/learn/Cargo.toml` (dependency), `crates/learn/src/lib.rs` (`mod image_encoder`, the `ImageFeatures` trait; `photo_features` takes any encoder), `crates/learn/src/encoder.rs` (`pixels` and `run` split out of `features`), `crates/engine/src/learn.rs` (the encoder is an `ImageEncoder`; `runtime`, `set_runtime`, `coreml_installed`, `backend`; `with_default_learn` reads the runtime), `crates/engine/src/cmd/learn.rs` (three `learn.status` fields). `crates/engine/Cargo.toml` (`lightcraft-coreml`, `zip` 7.4 without default features, `sha2` for a test), `crates/engine/src/lib.rs` (`mod model_runtime`, `Session::model_runtimes`), `crates/engine/src/cmd.rs` (`mod` + `specs` line), `crates/ui-egui/src/dialogs/settings.rs` (`mod` line), `crates/ui-egui/src/dialogs/settings/performance.rs` (one call at the end). People parts (fork files of `fork/people-parsing.md`): `crates/parsing/Cargo.toml` (`lightcraft-coreml`, the example), `crates/parsing/src/lib.rs` (`pub mod coreml`, `Backend`, `Runtimes`), `crates/parsing/src/parser.rs` (each parser an `Engine`: Core ML or candle; `load_with`, `backends`), `crates/parsing/src/face.rs` (`batch_pixels`, `parse_batch` public; `Nets` takes the parse function), `crates/parsing/src/body.rs` (`pixels` split out of `run`), `crates/parsing/src/tests.rs` (`mod coreml`), `crates/engine/src/people_parsing.rs` (`COREML_MODELS`, `runtimes`, `set_runtime`, `coreml_installed`, `backend`; the runtimes read in `with_default_parsing_models`), `crates/engine/src/model_runtime.rs` (two `MODELS` rows), `crates/engine/src/cmd/model_runtime.rs` (installed and backend per model), `crates/ui-egui/src/dialogs/settings/model_runtime.rs` (installed per model).

## Tests

```bash
cargo test -p lightcraft-coreml
cargo test -p lightcraft-learn --lib -- image_encoder
cargo test -p lightcraft-engine --lib -- model_runtime
cargo test -p lightcraft-parsing --lib -- coreml
```

The model tests read `model.safetensors` and `coreml/dinov2-small.mlpackage` from the shared folder through links in a temporary folder (nothing is written there) and skip with `skip!(Model, …)` without them: Core ML equals candle within tolerance on one photo; a missing or damaged Core ML model falls back to candle with the same numbers. Without any model, loading is an error, not a panic (runs everywhere). The Core ML crate's own tests feed it a damaged compiled model and package (an error from Core ML, no crash), an output shape whose size overflows, and a changed manifest (a new cache key). The people-parts tests read the SegFormer set and both packages from the shared folder through links: on a made-up picture, Core ML's clothes map picks the same class as candle's (Metal) on at least 99 % of pixels at two grid sizes (one inside a larger canvas), and each face group agrees on at least 99 % of pixels with a mean difference under 0.005; with no package or a damaged compiled model, Automatic is candle and Core ML is an error; the BiSeNet set refuses Core ML. A grid of every size the parser makes has a canvas. The download tests run against a local HTTP server and a tiny zip made in the test, never a real download: a checked zip unpacks and replaces the old package; a wrong hash or an entry outside the package installs nothing. A runtime choice that can't be saved is not applied; the download is refused without `confirm`.

## What's New entry

### Learn My Edits Runs on the Neural Engine
On a Mac, Learn My Edits' image model can run through Core ML on the Neural Engine, about 3–5× faster per photo: Settings ▸ Performance ▸ AI Models downloads its Core ML file (43 MB, asked first) and picks Automatic, Core ML or Candle.
