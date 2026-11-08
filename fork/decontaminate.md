# Decontaminate: hair keeps no background colour after a mask edits the background

List: a mask's Decontaminate slider: at its soft edge each pixel's own colour and the colour behind it are edited apart, so a background swap leaves no blue or green fringe between strands of hair; on by default for Background and Entire Person (issue #115). Branch `feat/115-hair-colour`.

## What it does

- **The problem**: a strand pixel the mask half-selects is a mix, `I = a·S + (1 − a)·U` (`S` the hair, `U` the sky). Edited as one colour, it keeps the sky's blue whatever the mask does to the background.
- **Decontaminate** (0..100, a mask setting under Refine Edges; `mask.decontaminate {id?, value}`): at the mask's soft edge (alpha between 0.002 and 0.998) the render estimates `S` and `U` (blur-fusion foreground estimation, Forte and Pitié 2021: a wide and a narrow box window, 4.5 % and 0.3 % of the long side, so a preview and an export agree), renders `S` with the mask wholly on and `U` with it wholly off, and mixes them by the alpha. The amount blends that with the old result. `S` and `U` mix back to the pixel exactly, so an edge the mask leaves alone renders as before (within 1/255).
- **Default**: new Background masks start at 100, and People masks that select the whole person (for Select People, not built yet); every other mask at 0. Documents without the field read 0: they render as before.
- **Where it runs**: the CPU render (`crates/pipeline/src/decontam.rs`, `finish.rs`). The GPU render has no kernel for it yet: a render with a decontaminated mask falls back to the CPU (`crates/gpu/src/lib.rs`), so preview and export always match. It adds about 0.05–0.15 s to a 1600 px render (four CC0 photos; Mac under load).
- **Edge models**: the hair edges (`fork/hair-edges.md`) now take any edge model through one hook: an `Edge` variant with its id, label and `suffix`, an arm in `HairEdges::available` and in `HairEdges::make` that returns a `Matte`. People, Subject, the cache, Settings and `maskEdges.*` take it from there.

How it does (2026-11-09; Background mask from BiRefNet, recoloured to a colour far from the background, with the Current edge and with BiRefNet Matting):
- Blue sky (two CC0 photos): with Decontaminate off, blue stays between and along the strands after the sky turns orange; on, it is gone with both edges.
- Green field and busy foliage (two CC0 photos): off, an olive ring stays round the strands after the green turns magenta; on, the strands stay brown and the new colour shows through where they are thin.
- The edge model matters less than decontamination for the fringe: BiRefNet Matting keeps more thin strands; decontamination removes the old colour from whatever strands the mask keeps.
- Time per photo: the Background mask 10–12 s with the Current edge (BiRefNet), 18–30 s with BiRefNet Matting (both models run; Mac under load); decontamination under 0.2 s per 1600 px render.

BiRefNet Matting on Core ML (not done here): convert the PyTorch checkpoint with coremltools; its decoder's deformable convolutions are not a Core ML layer and must become `grid_sample` (Core ML's resample, macOS 14) or a custom layer; quantize the 885 MB of weights to 8 bits; run it through `crates/coreml` (`Model::predict`), registered in `model_runtime::MODELS` so Settings ▸ Performance picks Core ML or candle; then a queue that mattes photos on import and when the Mac is idle, so the edge is ready when a photo opens.

## Branch

`feat/115-hair-colour` (from `feat/103-hair-edges`)

## Own files

`crates/pipeline/src/decontam.rs`, `crates/pipeline/src/tests_decontam.rs`, `crates/engine/src/cmd/decontaminate.rs`, `crates/engine/src/tests/decontaminate.rs`, `crates/engine/examples/decontam_compare.rs`, `crates/ui-egui/src/panels/decontaminate.rs`, `fork/decontaminate.md`, `fork/tests.d/decontaminate.txt`

## Hooks in official files

- `crates/develop/src/settings.rs`: field `Mask::decontaminate` (serde default 0, not written at 0) and its default.
- `crates/pipeline/src/lib.rs`: `pub mod decontam`, field `Prepared::split`, filled after `prep.recolor`; the `tests_decontam` module line. `crates/pipeline/src/local.rs`: the `Prepared` literal. `crates/pipeline/src/finish.rs`: the per-pixel stage as a `layer` closure run once, or three times at a decontaminated edge; `pixel_local_terms` and `recolor_pixel` take `over` (one mask's alpha replaced). `crates/pipeline/src/recolor/measure.rs`: passes `None`.
- `crates/gpu/src/lib.rs` (`run`): renders with a decontaminated mask fall back to the CPU.
- `crates/engine/src/cmd.rs`: `mod decontaminate`, its specs. `crates/engine/src/cmd/masks.rs` (`mask.add`): the new mask's default. `crates/engine/src/tests.rs`, `crates/ui-egui/src/panels.rs`: `mod` lines. `crates/ui-egui/src/panels/masking.rs`: the slider under Refine Edges. `crates/engine/Cargo.toml`: the example.
- Fork files of `fork/hair-edges.md`: `crates/engine/src/hair_edges.rs` (`matte_for`, `make`, `available`, `Edge::suffix`), `people_parsing.rs`, `segment/birefnet.rs`, `segment/worker/birefnet.rs`, `cmd/hair_edges.rs`.

## Tests

`cargo test -p lightcraft-pipeline --lib -- decontam` · `cargo test -p lightcraft-engine --features sam --lib -- decontaminate hair_edges` (also in `fork/tests.d/decontaminate.txt`). Pictures: `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example decontam_compare -- OUT_DIR PHOTO=RRGGBB...`.

## What's New entry

### Decontaminate Mask Edges
- A mask's Decontaminate slider separates the colour of hair from the background behind it at the mask's edge, so changing the background no longer leaves a blue or green fringe between strands. On by default for Background masks.

## Parity rows

| LR-MASK-DECONTAMINATE | Decontaminate colours at a mask's edge | P2 | 🟡 | `cmd:mask.decontaminate`, `crates/pipeline/src/decontam.rs` | fork: blur-fusion foreground estimation at the mask's soft edge, the selected and unselected colours edited apart; on by default for Background; CPU only (the GPU render falls back); preview and export match; not compared against Lightroom |
