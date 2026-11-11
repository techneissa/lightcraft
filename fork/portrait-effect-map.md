# Portrait: ⌥-drag shows what a slider changes

List: ⌥-dragging a Portrait slider shows what that slider changes on this face and how much, at its value: the photo dark and grey, the change glowing amber, nothing at 0 (issue #208). Branch `feat/208-portrait-effect-map`.

## What it does

Before, ⌥-drag on a Portrait slider drew a grey / white shape per feature from the faces' masks (`Feature::area`):
the same blob whatever the value, and the same for every slider of a feature. Now the view is measured
(`crates/pipeline/src/retouch_effect.rs`, `Overlay::RetouchEffect { feature, control, face }`):

- **Tone, colour and texture sliders** (every feature but Face Reshape): the retouch step runs on the image before
  it twice, with the settings as they are and with the dragged slider at its default, and each pixel's value is how
  far apart the two look: OkLab ΔE at the photo's exposure, averaged over 0.12 % of the photo's long edge (so
  smoothing reads as the skin it covers, not as speckle), on a fixed curve `1 − e^(−ΔE / 0.03)` (a just-visible
  change shows faintly, a plain retouch brightly). The scale is fixed, never normalised per photo, so a higher value
  reads brighter. Eyes light the eyes, Dark Circles the crescents under them, Teeth Whitening the teeth, Blemishes
  each spot it removes, Wrinkles the lines it takes away, Stray Hair each strand, Background the backdrop it changes.
- **Face Reshape**: how far the warp moves each point between the slider as it is and at 0, as a share of its face's
  size, on `1 − e^(−(d / 0.025)²)` (a flat start, so the warp's sub-pixel tails stay dark). Face by face
  (`reshape_step::field_of`).
- **Which face**: with All Faces picked, the slider's change in the settings for every face (a face with its own
  settings, which the slider does not move, stays dark); with a face picked, that face's own change. Background (a
  picture feature) always shows the settings for every face.
- **The look** (`visualize::retouch_effect`): the finished photo in grey at 36 % brightness, mixed toward amber
  (`EFFECT_GLOW`, 255 172 48) by the map, the amber shaded by the photo's lightness so the face stays readable. A
  brighter grey photo with a red tint (Lightroom's mask colour) was tried: small changes on light skin hardly showed.
- **Follows the features as they are**: nothing per feature. A feature whose algorithm changes keeps a right view.

## Speed

Measured on the CPU in both renderers, from the image before the step the render keeps (`StageCache`'s
`pre_retouch`, the GPU's host copy; made and kept here when there is none, as on the GPU with a raw's local tone map
on). The image with the slider at its default is kept between ticks (one, under the host budget), so a tick retouches
once more. Only the box round the changed pixels is blurred and curved. On a test
Mac under load, a 1920 × 1280 draft: the view's own work 5–14 ms for tone sliders, about 1 ms for Face Reshape; a
whole ⌥ tick 16–25 ms against 7–11 ms plain on the GPU (12–24 ms against 10–16 ms on the CPU), so the view updates live. `LIGHTCRAFT_PROFILE=1` prints
`retouch effect map: <ms>`.

Zoomed windows show the same as the whole frame (the window holds a face's retouch reads), to within a level.

`cargo run --release -p lightcraft-engine --example effect_map_sheet -- OUT_DIR PHOTO...` writes a sheet per slider
(the photo at 100, the view at 25, 50 and 100, a row per photo) and times ⌥ ticks against plain ones.

## Own files

`crates/pipeline/src/retouch_effect.rs`, `crates/pipeline/src/retouch_effect/tests.rs`,
`crates/engine/examples/effect_map_sheet.rs`, `fork/portrait-effect-map.md`, `fork/tests.d/portrait-effect-map.txt`

## Hooks in official and fork files

- `crates/pipeline/src/visualize.rs`: `Overlay::RetouchArea { feature, face }` is now `RetouchEffect { feature,
  control, face }` (parts, key), drawn by `retouch_effect`.
- `crates/pipeline/src/lib.rs`: `pub mod retouch_effect`; the CPU render keeps the image before the step for the view
  (`overlay_alpha` takes the source and it).
- `crates/pipeline/src/retouch_step.rs`: `apply_with` (the step with other settings); `area` removed.
- `crates/pipeline/src/reshape_step.rs`: `field_of` (one face's warp).
- `crates/gpu/src/render.rs`: `effect_pre`, `retouch_effect` (the host copy of the image before the step to the view).
- `crates/ui-egui/src/panels/clipping.rs` (`retouch_effect_overlay`: the dragged control's place), `panels/detail.rs`,
  `state.rs`.
- `crates/retouch/src/feature.rs`, `settings.rs`, `README.md`: docs only. `Feature::area` and `crates/retouch/src/area.rs`
  are no longer drawn; they stay until the Blemishes and Eyes lanes that edit those files have landed.

## Tests

`fork/tests.d/portrait-effect-map.txt`: on a synthetic face, Teeth Whitening's map is 0 on the skin and the
background, lights the teeth, grows from 25 to 50 to 100 and is 0 everywhere at 0, and stays its own with another
feature on; a picked face's own change; Face Reshape's map is 0 at 0 and far from the face and grows with the value;
the overlay's parts and key; the drawn view (grey where nothing changes, the glow on the teeth) and a zoomed window
equal to the whole frame; the UI's ⌥-drag on Skin Smoothness (`tests::portrait_alt`); the GPU's view equal to the
CPU's to within 4 levels.
