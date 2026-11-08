# Hair edges: BiRefNet Matting for people and Subject masks

List: hair-accurate edges for Entire Person / Background and Subject masks: BiRefNet Matting redraws the mask's edge so flyaway hair stays in, chosen per mask kind in Settings ▸ Faces ▸ Hair Edges (default unchanged) (issue #103). Branch `feat/103-hair-edges`.

## What it does

- **Two settings**, in Settings ▸ Faces ▸ Hair Edges (and `maskEdges.edge`), each **Current** (the default: masks stay exactly as before) or **BiRefNet Matting**:
  - **Entire Person / Background** (`people`): the people parse's Entire Person parts, and so its Background (everything that is not a person).
  - **Subject** (`subject`): new Subject parts and the Background made from them, whatever model makes them (SAM 3, BiRefNet, SAM 3 + BiRefNet Edges).
  The choice is kept in `<config>/mask-edges.json`. A chosen BiRefNet Matting whose model has gone counts as Current until it is back.
- **The model**: BiRefNet-matting (`ZhengPeng7/BiRefNet-matting`, MIT, 885 MB), the same network as BiRefNet (`crates/birefnet`) with weights trained for soft alpha mattes. It lives in the shared models folder (`~/Library/Application Support/ArtCraft/models/birefnet-matting/model.safetensors`; `LIGHTCRAFT_BIREFNET_DIR` overrides). Without it the setting reads "BiRefNet Matting (Download…)": choosing it asks first (size and folder), then downloads with a row in the activity stack (resume, size and SHA-256 checked) and is chosen once the download has finished. `maskEdges.download` needs `confirm: true`.
- **How the edge is drawn**: the mask's own model decides who or what is selected; the matte decides only within a band of 3 % of the photo's side around the mask's edge (strands the mask cut off come in, background between strands goes out), with a smooth hand-over. Things the matte also cuts out far from the mask (a lamp, a second subject) never come in. For people, a person never grows into another person, and strands a person gains beside their Hair are added to their Hair part.
- **Runs**: once per picture, on the worker thread that asked (the Subject worker, or the people-parsing thread), with a row "Refining hair edges" whose ✕ stops it. One model, shared by both, loaded on the GPU (CPU when the GPU cannot take it) and unloaded after two idle minutes; the newest four mattes are kept by a hash of the pixels. A people parse with matting edges is cached on disk as its own version (`+matting1`), so switching the setting back and forth never runs a model twice. Preview = export: Subject parts store the new 1024² grid in the mask (`SegMask::model` ends `+matting`; the part's hover says "…, BiRefNet Matting Edges"); people masks come from the cached parse.
- **Never crashes**: a missing model refuses the choice with the size and how to get it; a short, long, zero-filled or damaged file, a panic in the run and an invalid matte are messages ("Hair edges: …"), and a people parse that cannot get its matte fails with that message rather than quietly using the old edge.

How it does (three CC0 portraits with flyaway hair from Wikimedia Commons, 2026-11-09; pictures in the PR):
- Entire Person / Background: the clearest gain. The parse's edge keeps a blur of background between loose strands and cuts off thin strands; with BiRefNet Matting the background between strands goes and thin strands stay (2.6 to 5.7 % of the photo's pixels change).
- Subject: when BiRefNet already makes the part (the default with it installed), its edge was fine already: matting changes 0.1 to 0.4 % of the pixels, slightly softer strand tips. The gain is larger over SAM 3's coarse edge.
- Time per photo (M4 Pro, Metal, load average 45 to 60 from other work): the matting step adds about 7 to 9 s (Subject with BiRefNet: 8.1 to 10.5 s, with matting 15.0 to 18.9 s; people parse: 4.3 to 5.4 s, with matting 11.6 to 13.6 s, the first run including the model's load). A second request on the same photo reuses the matte.

## Branch

`feat/103-hair-edges`

## Own files

`crates/engine/src/hair_edges.rs`, `crates/engine/src/hair_edges/{model,people}.rs`, `crates/engine/src/cmd/hair_edges.rs`, `crates/engine/src/tests/hair_edges.rs`, `crates/engine/examples/hair_edges.rs`, `crates/ui-egui/src/dialogs/hair_edges.rs`, `crates/ui-egui/src/tests/hair_edges.rs`, `fork/hair-edges.md`, `fork/tests.d/hair-edges.txt`

## Hooks in official files

- `crates/engine/src/lib.rs`: `pub mod hair_edges`, field `Session::hair_edges`, made in `Session::new` and handed to people parsing (`with_hair_edges`).
- `crates/engine/src/cmd.rs`: `mod hair_edges;`, `hair_edges::specs()`. `crates/engine/src/tests.rs`, `crates/ui-egui/src/tests.rs`, `crates/ui-egui/src/dialogs.rs`: `mod` lines. `crates/engine/Cargo.toml`: the `hair_edges` example.
- `crates/engine/src/segment.rs`: `download` is `pub(crate)` (the matting download reuses it); `segment_job` reads the BiRefNet job's `model`.
- `crates/engine/src/segment/worker.rs`: `Kind::BiRefNet` carries `model` (was `combined`) and `matting`.
- Fork files of other features: `crates/engine/src/segment/birefnet.rs` (`concept_kind` routes a matting-edged request, `subject_matting`, `label_of` reads `+matting`), `crates/engine/src/segment/worker/birefnet.rs` (SAM 3 alone as a base, the matting step), `crates/engine/src/people_parsing.rs` (`with_hair_edges`, the cache version, the matting step after a parse, the models folder in `with_default_parsing_models`), `crates/ui-egui/src/dialogs/faces.rs` (the section after People Parts).

## Tests

`cargo test -p lightcraft-engine --features sam --lib -- hair_edges people_parsing birefnet` · `cargo test -p lightcraft-ui-egui --features lightcraft-engine/sam --lib -- hair_edges` (also in `fork/tests.d/hair-edges.txt`). The test that runs the real model reads it from the shared models folder and skips without it; the download test uses a local fake server. Pictures: `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example hair_edges -- OUT_DIR PHOTO...`.

## What's New entry

### Hair-Accurate Edges for People and Subject Masks
- Settings ▸ Faces ▸ Hair Edges picks the edge of Entire Person / Background and of Subject masks: Current, or BiRefNet Matting, which keeps flyaway hair and clears the background between strands.
- BiRefNet Matting is an 885 MB download (MIT licence), offered only after you agree. It runs once per photo; switching back to Current gives the masks you had.

## Parity rows

| LR-MASK-HAIR-EDGES | Hair-accurate edges for people and subject masks | P2 | 🟡 | `cmd:maskEdges.edge`, `cmd:maskEdges.status`, `cmd:maskEdges.download`, `cmd:maskEdges.cancel`, `crates/engine/src/hair_edges.rs` | fork: BiRefNet-matting redraws the edge of Entire Person / Background (people parsing) and Subject masks within a band around the mask's edge, per mask kind (default Current, unchanged); once per photo, cached by pixels, preview = export; not compared against Lightroom; Select People UI not built yet, so people masks are reached through `parsing.mask` |
