# People parts: per-person masks for skin, eyes, lips, hair and clothes

List: per-person part masks (skin, hair, eyebrows, eye whites, iris, lips, teeth, clothes) and 478 face landmarks from two downloadable model sets, cached per photo, for the portrait features and Select People (issue #81). Branch `feat/81-people-parsing`.

## What it does

The base of the portrait work (#79): it finds each person in a photo and where their parts are. Nothing in the
Edit view uses it yet; the Select People masks and the Portrait features build on it.

- **Parts per person**: Entire Person, Facial Skin (face, nose, ears), Body Skin (neck, arms, hands, legs), Hair,
  Eyebrows, Eye Whites, Iris, Lips, Teeth, Clothes Top, Clothes Bottom, Dress, Shoes, Hat, Bag, Others (glasses,
  sunglasses, earrings, necklaces, belts, scarves). Background is everything that is not a person. Each person also
  has a face box, 478 face landmarks (MediaPipe's order, irises included) and the person id of the face region it
  matches in the photo's faces.
- **Two model sets**, side by side, picked in Settings ▸ Faces ▸ People Parts (or `parsing.modelSet`):
  | Set | Face parser | Body parser | Landmarks | Download | Licence |
  | --- | --- | --- | --- | --- | --- |
  | SegFormer (default) | SegFormer-B0, 19 face classes ([alexkrz/segformer-b0-celebamask_hq](https://huggingface.co/alexkrz/segformer-b0-celebamask_hq) @ `e1e6109`, 14.9 MB) | SegFormer-B3, 18 clothes classes ([sayeed99/segformer_b3_clothes](https://huggingface.co/sayeed99/segformer_b3_clothes) @ `6c12f0e`, 189.0 MB) | MediaPipe face mesh v2 ([SceneWorks/mp-facemesh-v2](https://huggingface.co/SceneWorks/mp-facemesh-v2) @ `4e65de9`, 5.0 MB) | 209 MB | B0: "other" (NVIDIA MiT, non-commercial; CelebAMask-HQ, non-commercial research); B3: MIT on the card, NVIDIA MiT underneath; mesh: Apache-2.0 |
  | BiSeNet | BiSeNet ResNet-18, 19 face classes ([mlx-community/face-parsing-bisenet-bf16](https://huggingface.co/mlx-community/face-parsing-bisenet-bf16) @ `5f2aa21`, 26.6 MB) | SegFormer-B2, 18 clothes classes ([mattmdjaga/segformer_b2_clothes](https://huggingface.co/mattmdjaga/segformer_b2_clothes) @ `584abc1`, 109.5 MB) | the same face mesh | 141 MB | BiSeNet: MIT (CelebAMask-HQ, non-commercial research); B2: MIT on the card, NVIDIA MiT underneath |

  Sizes and SHA-256 are pinned in `crates/parsing/src/models.rs`. No file is over 300 MB. Not wired: the
  SegFormer-B5 face parser (`jonathandinu/face-parsing`, 339 MB, over the 300 MB limit) and Sapiens (body-part
  segmentation with teeth and lips, 0.3B is about 600 MB).
- **Download**: asked first (the section's "(Download…)" entry opens a dialog with the size and folder;
  `parsing.download` needs `confirm: true`), into `~/Library/Application Support/ArtCraft/models/<folder>/` (shared
  with PhotoCraft; `LIGHTCRAFT_PARSING_MODELS` overrides), one row "Downloading model" in the activity stack whose ✕
  stops it, resumable, each file's size and SHA-256 checked. Files a set shares (the face mesh) download once.
- **Parse**: once per photo and model version, on a worker thread with a row "Finding people parts" (✕ stops it). The
  photo is rendered upright, uncropped, with default settings at 2048 px (the frame face regions use); faces come
  from YuNet when it is installed (Settings ▸ Faces), else from the body model. The result is cached on disk by the
  pixels and the set's version (`~/Library/Caches/LightCraft/people-parsing`, 512 MB, oldest first) and the newest
  four in memory. A caller that needs the masks now (an export) waits for the run already going instead of starting a
  second one (`PeopleParsing::parse(.., wait: true)`). The models stay loaded for a minute after their last use.
- **Resolution independent**: masks are planes in fractions of the photo at the model's resolution (face crops at
  512², the body at 768 to 1536 px); `lightcraft_parsing::render` draws them at any size and snaps their edges to the
  photo with a guided filter whose radius is 1.5 model cells, so a preview and a 24 MP export agree.
- **Commands**: `parsing.status`, `parsing.modelSet {set}`, `parsing.download {set?, confirm}`, `parsing.cancel`,
  `parsing.run {ids?, wait?}` (people, faces, landmark counts and each part's area), `parsing.mask {id?, part,
  person?, path, edge?}` (one mask as a grey PNG).
- **Never crashes**: a missing or damaged model file is an error naming the file; a damaged cache file is parsed again;
  a panic in the run is a message; no people, 50 faces, faces cut by the edge and a 1 × 1 photo are tested.

How it does on photos (techneissa's A7 V photos and CC0 photos, 2026-11-08):
- Both sets: faces, eyes (cut to the mesh's eye openings, iris from the mesh), lips, brows, hands and arms are right on
  portraits and groups; a group of 29 with 30-px faces gets tops, trousers, hands and shoes per person.
- SegFormer set: better clothes (a thobe is a top, patterned scarves and dark shirts are clothes), finds glasses more
  often; calls beards facial skin; about 1.5 to 2 times slower.
- BiSeNet set: calls beards hair (better for skin retouching); the B2 body model misses dark shirts and calls a thobe a
  dress; on one occluded face it called the whole face crop hair.
- Both: people on a screen or in a poster are people; a coat or a dark car with no hair, face or limbs is not.

## Branch

`feat/81-people-parsing` (from `custom`: it uses the shared models folder and the LaMa-style download).

## Own files

`crates/parsing/` (`src/{lib,types,models,image,weights,nn,segformer,bisenet,facemesh,eyes,body,people,face,parser,render,cache,tests}.rs`, `src/tests/models.rs`, `examples/parts.rs`, `README.md`, `testdata/README.md`), `crates/engine/src/people_parsing.rs`, `crates/engine/src/people_parsing/download.rs`, `crates/engine/src/cmd/parsing.rs`, `crates/engine/src/tests/people_parsing.rs`, `crates/ui-egui/src/dialogs/people_parts.rs`, `crates/ui-egui/src/tests/people_parts.rs`, `fork/people-parsing.md`, `fork/tests.d/people-parsing.txt`

## Hooks in official files

- `Cargo.toml` (workspace dependency `lightcraft-parsing`), `Cargo.lock`, `xtask/src/layers.rs` (`parsing` is layer 3), `NOTICE` (the models' entry).
- `crates/engine/Cargo.toml` (the dependency), `crates/engine/src/lib.rs` (`pub mod people_parsing`, field `Session::parsing`), `crates/engine/src/cmd.rs` (`mod parsing`, `parsing::specs()`), the `mod` line of `crates/engine/src/tests.rs`.
- `apps/lightcraft/src/main.rs` and `crates/mcp/src/headless.rs` (`with_default_parsing_models()`).
- `crates/ui-egui/src/dialogs.rs` (`pub mod people_parts`), `crates/ui-egui/src/dialogs/faces.rs` (the section at the end of the Faces tab), the `mod` line of `crates/ui-egui/src/tests.rs`.
- `docs/parity-checklist.md`: the LR-MASK-PEOPLE row, changed in place.

## Tests

```bash
cargo test -p lightcraft-parsing --lib
cargo test -p lightcraft-engine --lib people_parsing
cargo test -p lightcraft-ui-egui --lib people_parts
```

With the models: `LIGHTCRAFT_PARSING_MODELS=<models folder> cargo test -p lightcraft-parsing --lib` also parses a 1 × 1
photo, a photo with no people and 50 faces. Over the network, opt-in:
`LIGHTCRAFT_PARSING_DOWNLOAD_TEST=<empty folder> cargo test -p lightcraft-engine --lib both_sets_download` downloads
both sets and checks them. Pictures: `cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/`.
