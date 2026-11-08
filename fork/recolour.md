# Recolor on a mask

List: Recolor, a mask effect that gives the masked area a new colour and keeps its shading (red boots to purple boots with the same folds and light), from a colour picker, a hex value or an eyedropper on this or another photo (issue #83). Branch `feat/83-recolour`.

## What it does

- Every mask has **Recolor** rows in the Masking panel, under the effect sliders: the new colour (swatch with the colour picker, `#rrggbb`, an eyedropper on the photo), **Color to Replace** (Automatic, or picked with its own eyedropper; Automatic clears it), **Recolor Amount** (0 = off) and **Range**. Works on every mask kind (Brush, gradients, ranges, Object, Describe "boots" through SAM 3, and later the clothes parts of #81, which are masks like the others).
- Only the area's **main colour** changes; other colours inside the mask (dust, a white lining, black rubber, a blue logo) stay. Method (`crates/pipeline/src/recolor/`), in OkLab on display-linear colour right after the colour stage:
  - the main colour (`recolor/measure.rs`) is the peak of a hue histogram of the area's coloured pixels weighted by chroma × alpha, refined to their circular mean, with that cluster's lightness, chroma and spread. It is measured on a fixed 384 px proxy of the whole frame through the same per-pixel steps the finish stage runs before Recolor (exposure, dehaze, local tone such as Shadows, Contrast and Clarity, calibration, tone map, colour stage, colour overlay), so preview, export, zoomed windows and the GPU use the same numbers and the main colour lands on the pick. A mask too small for that proxy (under 64 proxy pixels) is measured on a 1024 px one. Color to Replace sets the hue instead of the histogram;
  - each pixel's change is weighted by how much it belongs to the main colour: its hue distance from the main hue (smooth falloff, window from the cluster's spread) times its saturation (chroma over lightness) relative to the main colour's, and a pixel under about 0.01–0.025 chroma has too weak a hue to count. Range widens or narrows the window (each 50 doubles or halves it). Weakly coloured pixels near the main hue (a glossy object's greyish sheen) take the new hue and lightness at their own chroma;
  - lightness keeps its proportion to the main colour's below it (black to white keeps fold contrast), and above it the room left up to white is shared out. A target as light as the main colour leaves lightness exactly as it was;
  - chroma is the target's × the pixel's chroma over the main chroma, softly limited to 1.75 × above it, and faded out at very low and very high lightness, so dark edges and highlights don't take saturated colour. A grey area (a black shirt) takes chroma from its shading, never above the target's, fading out above its own lightness;
  - hue is the target's plus the pixel's own offset from the main hue (up to 0.5 rad, only where both are clearly coloured).
- Speed: the measurement costs about 30–260 ms on a 24 MP photo (most of it the proxy's planes; the Mac was loaded while measured). It is kept per edit: changing a Recolor amount, colour or Range, or moving any slider during a drag (draft renders take the last answer for the same photo, frame and masks), measures nothing again; the full render after a drag measures once.
- Settings: `LocalAdjustments::recolor` (amount 0..100), `recolor_r/g/b` (sRGB 0..1), `recolor_range` (−100..100), `recolor_from_r/g/b` (Color to Replace, sRGB 0..1; black = Automatic), not written while 0, so edits without Recolor are saved byte for byte as before. Sync, Copy / Paste (Masks group), develop presets and mask effect presets carry them (an effect preset's "Recolor" choice brings all of them).
- Commands (and so MCP, `cmd_mask_recolor`, `cmd_mask_recolorPick`): `mask.recolor {id?, color?, amount?, range?, from?: "#rrggbb"|null}`, `mask.recolorPick {id?, x, y, photo?, target?: "to"|"from"}` (the colour at a point, 0..1 each, of the active photo or of another photo in the library, as developed; a point off the photo is an error); `mask.adjust` takes the fields too.
- Not changed: the area measured is the cropped frame, so changing the crop can move the main colour a little. A mask effect preset saved with "All" sliders before Recolor existed sets Recolor to off when applied (it lists every slider, and Recolor's was 0).
- Tuning outside the app: `cargo run --release -p lightcraft-pipeline --example recolor -- in.jpg mask.png out.jpg --to "#7a3fb0" --amount 100 [--range 0] [--from "#rrggbb"]`.

## Branch

`feat/83-recolour`, from `custom` (the masking code on `custom` differs from upstream `main`).

## Own files

`crates/pipeline/src/recolor/` (`mod.rs`, `measure.rs`, `tests.rs`), `crates/pipeline/src/tests_recolor_render.rs`, `crates/pipeline/examples/recolor.rs`, `crates/engine/src/cmd/mask_recolor.rs`, `crates/engine/src/cmd/mask_recolor_tests.rs`, `crates/ui-egui/src/panels/mask_recolor.rs`, `crates/ui-egui/src/tests/mask_recolor.rs`, `fork/recolour.md`, `fork/tests.d/recolour.txt`

## Hooks in official files

- `crates/develop/src/settings.rs`: four `LocalAdjustments` fields (skipped while 0) and their defaults.
- `crates/pipeline/src/lib.rs`: `pub mod recolor;`, the test `mod` line, `Plan::recolor` (measured in `plan`), `Prepared::recolor` (copied in `render_impl`). `crates/pipeline/src/local.rs`: `frame_proxy` is `pub(crate)`, `Prepared` literal.
- `crates/pipeline/src/finish.rs`: `MASK_TERMS` grows by 10 (`MASK_RECOLOR`), `mask_terms` zeros, `effect_terms_with`, `scaled_terms` uses it, `recolor_pixel` after the colour overlay; `apply_light`, `PixelLocalTerms`, `scaled_terms`, `pixel_local_terms`, `adjust_scene_pixel` and `local_tone_delta` are `pub(crate)` (the measurement runs them). `lib.rs`: `hash_of` is `pub(crate)`.
- `crates/gpu/src/masks.rs`: terms through `effect_terms_with` with the plan's references. `crates/gpu/src/ctx.rs`: the Recolor constants. `crates/gpu/src/wgsl/finish.wgsl`: `recolor_masks` after the colour overlay. `crates/gpu/tests/it/equivalence.rs`: a "masks (recolor)" case.
- `crates/engine/src/cmd.rs`: `mod mask_recolor;`, its specs. `crates/engine/src/cmd/masks.rs`: `clamp_local` clamps the fields (Range −100..100); `mask.adjust`'s parameter text. `crates/engine/src/cmd/mask_effects.rs`: the fields in `FIELDS`, the colour channels go with the amount (`changed_fields`, `matches`, `normalized`), `set` accepts fields that are not written.
- `crates/ui-egui/src/panels.rs`: `pub mod mask_recolor;`. `panels/masking.rs`: the row after the sliders. `panels/mask_effects.rs`: "Recolor" in the effect lists and the New Effect Preset checklist. `panels/detail.rs`: the eyedropper's click and crosshair. `space_hand.rs`: it counts as a picker. `tests.rs`: `mod` line.

## Tests

`fork/tests.d/recolour.txt`: the maths (red gradient to purple keeps its lightness profile; black to white keeps fold contrast in proportion; in a mask holding red, a blue star, grey dust, an off-white lining and black rubber only the red changes; Range; a dark edge doesn't take saturated colour; a grey area never takes more chroma than the target; Color to Replace; Amount 0 unchanged; hostile values), whole renders (outside the mask and Amount 0 byte for byte; preview vs a 4× export scaled down; a window vs the full render; a Describe mask; the pick lands with Shadows / Contrast / Blacks set; a tiny mask), GPU against CPU, the commands (eyedropper on another photo, Color to Replace, points off the photo, presets, paste), and the panel's two eyedroppers.

## What's New entry

### Recolor
- A mask can now recolor what it selects: pick a colour (picker, hex value or eyedropper) and the area's main colour takes it while keeping its own folds, gradients and highlights. Other colours inside the mask (a logo, dust, a white lining) stay; Range and Color to Replace choose which colour changes. Red boots become purple boots with the same light on them; a black shirt can become white with its folds in place.
- Works with every mask, including Describe ("boots", "red dress"). Saved with the mask and carried by Sync, Copy / Paste and presets.

## Parity rows

| LR-MASK-RECOLOR | Recolor a masked area | P2 | ✅ | `cmd:mask.recolor`, `cmd:mask.recolorPick`, `crates/pipeline/src/recolor.rs`, `crates/ui-egui/src/panels/mask_recolor.rs` | beyond Lightroom (no equivalent): the area takes a picked colour (picker, hex, eyedropper on this or another photo) and keeps its shading, in OkLab against the area's mean measured on a fixed proxy, so preview, export and GPU match |
