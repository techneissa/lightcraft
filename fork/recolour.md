# Recolor on a mask

List: Recolor, a mask effect that gives the masked area a new colour and keeps its shading (red boots to purple boots with the same folds and light), from a colour picker, a hex value or an eyedropper on this or another photo (issue #83). Branch `feat/83-recolour`.

## What it does

- Every mask has a **Recolor** row in the Masking panel, under the effect sliders: a colour swatch (opens the colour picker), the colour as `#rrggbb`, an eyedropper that takes the colour from a point of the photo, and **Recolor Amount** (0 = off). Works on every mask kind (Brush, gradients, ranges, Object, Describe "boots" through SAM 3, and later the clothes parts of #81, which are masks like the others).
- Method (`crates/pipeline/src/recolor.rs`), in OkLab on display-linear colour after the colour stage:
  - the area's own colour (its **reference**: mean lightness, mean chroma, mean hue, alpha-weighted) is measured once per render on a fixed 384 px proxy of the whole frame, through the exposure, tone map and colour stage. The proxy size doesn't depend on the request, so preview, export, a zoomed window and the GPU all use the same numbers;
  - lightness keeps its proportion to the area's mean below the mean (a fold half as light stays half as light: black to white keeps fold contrast), and above the mean the room left up to white is shared out so nothing passes white. A target as light as the area leaves lightness exactly as it was;
  - chroma keeps its proportion to the area's mean chroma, softly limited to 1.75 × above it; a pixel that is grey for its lightness (a specular highlight, a white sole, a grey toe cap) stays grey; a grey area (a black shirt) takes chroma from its shading instead;
  - hue is the target's, plus the pixel's own offset from the area's hue (up to 0.5 rad, only where both are clearly coloured), so a warmer light on one side stays warmer;
  - the result's mean lands on the picked colour (measured on the CC0 shoe: target L 0.497 / C 0.174 / hue −55°, recolored area L 0.495 / C 0.178 / hue −55°).
- Settings: `LocalAdjustments::recolor` (amount 0..100) and `recolor_r/g/b` (sRGB 0..1), not written while 0, so edits without Recolor are saved byte for byte as before. Sync, Copy / Paste (Masks group), develop presets and mask effect presets carry them (an effect preset's "Recolor" choice brings the colour with the amount).
- Commands (and so MCP, `cmd_mask_recolor`, `cmd_mask_recolorPick`): `mask.recolor {id?, color?: "#rrggbb", amount?}`, `mask.recolorPick {id?, x, y, photo?}` (the colour at a point of the active photo or of another photo in the library, as developed); `mask.adjust` takes the four fields too.
- The eyedropper on another photo is the `photo` parameter of `mask.recolorPick` (commands and MCP); the panel's eyedropper picks on the photo being edited.
- Tuning outside the app: `cargo run --release -p lightcraft-pipeline --example recolor -- in.jpg mask.png out.jpg --to "#7a3fb0" --amount 100`.

## Branch

`feat/83-recolour`, from `custom` (the masking code on `custom` differs from upstream `main`).

## Own files

`crates/pipeline/src/recolor.rs`, `crates/pipeline/src/tests_recolor.rs`, `crates/pipeline/src/tests_recolor_render.rs`, `crates/pipeline/examples/recolor.rs`, `crates/engine/src/cmd/mask_recolor.rs`, `crates/engine/src/cmd/mask_recolor_tests.rs`, `crates/ui-egui/src/panels/mask_recolor.rs`, `crates/ui-egui/src/tests/mask_recolor.rs`, `fork/recolour.md`, `fork/tests.d/recolour.txt`

## Hooks in official files

- `crates/develop/src/settings.rs`: four `LocalAdjustments` fields (skipped while 0) and their defaults.
- `crates/pipeline/src/lib.rs`: `pub mod recolor;`, the test `mod` line, `Plan::recolor` (measured in `plan`), `Prepared::recolor` (copied in `render_impl`). `crates/pipeline/src/local.rs`: `frame_proxy` is `pub(crate)`, `Prepared` literal.
- `crates/pipeline/src/finish.rs`: `MASK_TERMS` grows by 8 (`MASK_RECOLOR`), `mask_terms` zeros, `effect_terms_with`, `scaled_terms` uses it, `recolor_pixel` after the colour overlay.
- `crates/gpu/src/masks.rs`: terms through `effect_terms_with` with the plan's references. `crates/gpu/src/ctx.rs`: the Recolor constants. `crates/gpu/src/wgsl/finish.wgsl`: `recolor_masks` after the colour overlay. `crates/gpu/tests/it/equivalence.rs`: a "masks (recolor)" case.
- `crates/engine/src/cmd.rs`: `mod mask_recolor;`, its specs. `crates/engine/src/cmd/masks.rs`: `clamp_local` clamps the fields; `mask.adjust`'s parameter text. `crates/engine/src/cmd/mask_effects.rs`: the fields in `FIELDS`, the colour channels go with the amount (`changed_fields`, `matches`, `normalized`), `set` accepts fields that are not written.
- `crates/ui-egui/src/panels.rs`: `pub mod mask_recolor;`. `panels/masking.rs`: the row after the sliders. `panels/mask_effects.rs`: "Recolor" in the effect lists and the New Effect Preset checklist. `panels/detail.rs`: the eyedropper's click and crosshair. `space_hand.rs`: it counts as a picker. `tests.rs`: `mod` line.

## Tests

`fork/tests.d/recolour.txt`: the maths (red gradient to purple keeps its lightness profile; black to white keeps fold contrast in proportion; grey parts stay grey; Amount 0 and outside the mask unchanged; hostile values), whole renders (outside the mask and Amount 0 byte for byte; preview vs a 4× export scaled down; a window vs the full render; a Describe mask), GPU against CPU, the commands (eyedropper on another photo, presets, paste), and the panel's eyedropper.

## What's New entry

### Recolor
- A mask can now recolor what it selects: pick a colour (picker, hex value or eyedropper) and the area takes it while keeping its own folds, gradients and highlights. Red boots become purple boots with the same light on them; a black shirt can become white with its folds in place.
- Works with every mask, including Describe ("boots", "red dress"). Saved with the mask and carried by Sync, Copy / Paste and presets.

## Parity rows

| LR-MASK-RECOLOR | Recolor a masked area | P2 | ✅ | `cmd:mask.recolor`, `cmd:mask.recolorPick`, `crates/pipeline/src/recolor.rs`, `crates/ui-egui/src/panels/mask_recolor.rs` | beyond Lightroom (no equivalent): the area takes a picked colour (picker, hex, eyedropper on this or another photo) and keeps its shading, in OkLab against the area's mean measured on a fixed proxy, so preview, export and GPU match |
