# Decontaminate: hair keeps no background colour after a mask edits the background

List: a mask's Decontaminate slider: at its soft edge each pixel's own colour and the colour behind it are edited apart, so a background swap leaves no blue or green fringe between strands of hair; on by default for Background and Entire Person (issue #115). Branch `feat/115-hair-colour`.

## What it does

- **The problem**: a strand pixel the mask half-selects is a mix, `I = a·S + (1 − a)·U` (`S` the hair, `U` the sky). Edited as one colour, it keeps the sky's blue whatever the mask does to the background.
- **Decontaminate** (0..100, a mask setting under Refine Edges; `mask.decontaminate {id?, value}`): at the mask's soft edge (alpha between 0.002 and 0.998) the render estimates `S` and `U` (blur-fusion foreground estimation, Forte and Pitié 2021: a wide and a narrow box window, 4.5 % and 0.3 % of the long side, so a preview and an export agree). `S` and `U` mix back to the pixel exactly.
- **How a pixel is drawn** (`crates/pipeline/src/decontam/pixel.rs`): once, as the composite of its two colours. The mask's scene edits (Exposure, Temp, Tint, Dehaze, Noise, Moiré, Defringe) are made to `S` with the mask wholly on and to `U` with it wholly off, and mixed by the alpha in scene linear. The tone stage (Highlights, Shadows, Whites, Blacks, Contrast, Clarity, Texture, the tone map) then runs once on the composite, as on any pixel: no bright rim, no colour shift on a raw's tone map. The colour edits after it (Saturation, Hue, Color, Recolor) are made to each colour's display colour apart (mask at 1 and 0) and mixed by the alpha. Each change is added as a difference, so a mask with nothing to edit apart changes no pixel (within 1/255). The amount scales the differences. Every decontaminated mask whose edge holds a pixel is split, in mask order.
- **Which masks**: visible, Decontaminate above 0, an Amount, and an edit the split changes (scene or colour edits; `decontam::applies`). A mask with only tone edits draws as without it.
- **Default**: new Background masks start at 100, and People masks that select the whole person (for Select People, not built yet); every other mask at 0. The slider resets to the mask's start value. Documents without the field read 0: they render as before. The field is fork catalog format 4 (`fork/catalog-format.md`).
- **Where it runs**: the CPU render (`crates/pipeline/src/decontam/`, `finish.rs`). Only the edge is estimated and kept (its pixels' two colours), in bands of 512 rows that each read the windows' reach around them. The edge is kept in the stage cache, keyed by the image and the mask's alpha: a slider drag (the mask's sliders, its Amount, the global sliders) doesn't estimate it again. A zoomed (windowed) render reads 4.8 % of the long side around itself, so its edge matches the whole render. The GPU render has no kernel for it yet: a render with a decontaminated mask falls back to the CPU (`crates/gpu/src/lib.rs`), so preview and export always match.
- **A GPU version** needs: the edge's colours uploaded as a texture (or estimated there: seven box means of two radii, in bands), the scene stage run three times at edge pixels with the mask's alpha at its own value, 1 and 0 (the per-mask terms are already in the shader), the tone stage once, and the colour stage twice more where the mask has colour edits; then the CPU fallback in `gpu::run` goes.
- **Edge models**: the hair edges (`fork/hair-edges.md`) now take any edge model through one hook: an `Edge` variant with its id, label and `suffix`, an arm in `HairEdges::available` and in `HairEdges::make` that returns a `Matte`. People, Subject, the cache, Settings and `maskEdges.*` take it from there.

How it does (2026-11-09; Background mask from BiRefNet, recoloured to a colour far from the background, with the Current edge and with BiRefNet Matting):
- Blue sky (two CC0 photos): with Decontaminate off, blue stays between and along the strands after the sky turns orange; on, it is gone with both edges.
- Green field and busy foliage (two CC0 photos): off, an olive ring stays round the strands after the green turns magenta; on, the strands stay brown and the new colour shows through where they are thin.
- The edge model matters less than decontamination for the fringe: BiRefNet Matting keeps more thin strands; decontamination removes the old colour from whatever strands the mask keeps.
- Time per photo: the Background mask 10–12 s with the Current edge (BiRefNet), 18–30 s with BiRefNet Matting (both models run; Mac under load).
- A slider drag (2026-11-09, `LIGHTCRAFT_PROFILE=1`, one run, Mac under load; a 1920 × 1280 CC0 photo at 1600 px, the Background mask without a model, soft over 48 % of the frame, a worst case; Recolor on, the mask's Exposure dragged): the first render estimates the edge once (prepare 205 ms, 79 ms without decontamination); every draft after it reuses it (prepare 0–15 ms) and draws in 75–100 ms, 35–70 ms without. A model's matte has a much thinner edge.

BiRefNet Matting on Core ML (not done here): convert the PyTorch checkpoint with coremltools; its decoder's deformable convolutions are not a Core ML layer and must become `grid_sample` (Core ML's resample, macOS 14) or a custom layer; quantize the 885 MB of weights to 8 bits; run it through `crates/coreml` (`Model::predict`), registered in `model_runtime::MODELS` so Settings ▸ Performance picks Core ML or candle; then a queue that mattes photos on import and when the Mac is idle, so the edge is ready when a photo opens.

## Branch

`feat/115-hair-colour` (from `feat/103-hair-edges`)

## Own files

`crates/pipeline/src/decontam/` (`mod.rs`, `pixel.rs`, `tests.rs`), `crates/pipeline/src/tests_decontam.rs`, `crates/engine/src/cmd/decontaminate.rs`, `crates/engine/src/tests/decontaminate.rs`, `crates/engine/examples/decontam_compare.rs`, `crates/ui-egui/src/panels/decontaminate.rs`, `fork/decontaminate.md`, `fork/tests.d/decontaminate.txt`

## Hooks in official files

- `crates/develop/src/settings.rs`: field `Mask::decontaminate` (serde default 0, not written at 0) and its default.
- `crates/pipeline/src/lib.rs`: `pub mod decontam`, field `Prepared::split`, filled after `prep.recolor` from the kept edges; `CacheEntry::split` (and its bytes); in `plan`, a windowed render grows by `decontam::reach`; the `tests_decontam` module line. `crates/pipeline/src/local.rs`: the `Prepared` literal. `crates/pipeline/src/finish.rs`: the tone map as a `to_display` closure and the colour edits as a `colour` closure, so `decontam::pixel` runs them on each colour; the hooks after the scene stage and the colour stage; `pixel_local_terms` and `recolor_pixel` take `over` (one mask's alpha replaced). `crates/pipeline/src/recolor/measure.rs`: passes `None`. `crates/pipeline/src/tests_window.rs`: a window-independence test.
- `crates/gpu/src/lib.rs` (`run`): renders with a decontaminated mask (`decontam::applies`) fall back to the CPU.
- `crates/catalog`: `fork_format.rs` (`VERSION` 4), `journal.rs` (the fork 4 row), `tests_format_version.rs`, `tests_fork_format.rs`.
- `crates/engine/src/cmd.rs`: `mod decontaminate`, its specs, `decontaminate_start` (the slider's reset value). `crates/engine/src/cmd/masks.rs` (`mask.add`): the new mask's default. `crates/engine/src/tests.rs`, `crates/ui-egui/src/panels.rs`: `mod` lines. `crates/ui-egui/src/panels/masking.rs`: the slider under Refine Edges. `crates/engine/Cargo.toml`: the example.
- Fork files of `fork/hair-edges.md`: `crates/engine/src/hair_edges.rs` (`matte_for`, `make`, `available`, `Edge::suffix`), `people_parsing.rs`, `segment/birefnet.rs`, `segment/worker/birefnet.rs`, `cmd/hair_edges.rs`.

## Tests

`cargo test -p lightcraft-pipeline --lib -- decontam tests_window::a_decontaminated` · `cargo test -p lightcraft-catalog --lib -- fork_format` · `cargo test -p lightcraft-engine --features sam --lib -- decontaminate hair_edges` · `cargo test -p lightcraft-ui-egui --lib -- decontaminate` (also in `fork/tests.d/decontaminate.txt`). Pictures: `LIGHTCRAFT_CONFIG_DIR=<scratch> cargo run --release -p lightcraft-engine --features sam --example decontam_compare -- OUT_DIR PHOTO=RRGGBB...`.

## What's New entry

### Decontaminate Mask Edges
- A mask's Decontaminate slider separates the colour of hair from the background behind it at the mask's edge, so changing the background no longer leaves a blue or green fringe between strands. On by default for Background masks.

## Parity rows

| LR-MASK-DECONTAMINATE | Decontaminate colours at a mask's edge | P2 | 🟡 | `cmd:mask.decontaminate`, `crates/pipeline/src/decontam.rs` | fork: blur-fusion foreground estimation at the mask's soft edge, the selected and unselected colours edited apart, the tone stage once on their composite; on by default for Background; CPU only (the GPU render falls back); preview, export and zoomed windows match; not compared against Lightroom |
