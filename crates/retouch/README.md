# lightcraft-retouch

Portrait retouching for LightCraft (fork feature: `fork/retouch-foundation.md`, issues #79 and #80): the settings,
the face input contract and the retouching step. A leaf crate: it depends on `lightcraft-raster`, `serde` and
`serde_json` only, never on a model, the engine, develop, pipeline or the UI.

## Adding a feature

Each feature owns one module and touches nothing else in this crate:

| Feature | Module | Example | Tests | Panel section |
|---|---|---|---|---|
| Blemishes | `src/blemish.rs` | `examples/blemish.rs` | `tests/blemish.rs` | `crates/ui-egui/src/panels/edit/portrait/blemish.rs` |
| Skin | `src/skin.rs` | `examples/skin.rs` | `tests/skin.rs` | `…/portrait/skin.rs` |
| Wrinkles | `src/wrinkles.rs` | `examples/wrinkles.rs` | `tests/wrinkles.rs` | `…/portrait/wrinkles.rs` |
| Skin Tone | `src/skin_tone.rs` | `examples/skin_tone.rs` | `tests/skin_tone.rs` | `…/portrait/skin_tone.rs` |
| Eyes | `src/eyes.rs` | `examples/eyes.rs` | `tests/eyes.rs` | `…/portrait/eyes.rs` |
| Teeth and Lips | `src/teeth_lips.rs` | `examples/teeth_lips.rs` | `tests/teeth_lips.rs` | `…/portrait/teeth_lips.rs` |
| Glasses Glare | `src/glare.rs` | `examples/glare.rs` | `tests/glare.rs` | `…/portrait/glare.rs` |
| Stray Hair | `src/hair.rs` | `examples/hair.rs` | `tests/hair.rs` | `…/portrait/hair.rs` |
| Clothes Color | `src/clothes_colour.rs` | `examples/clothes_colour.rs` | `tests/clothes_colour.rs` | `…/portrait/clothes_colour.rs` |
| Face Reshape | `src/reshape.rs` | `examples/reshape.rs` | `tests/reshape.rs` | `…/portrait/reshape.rs` |
| Background | `src/background.rs` | `examples/background.rs` | `tests/background.rs` | `…/portrait/background.rs` |

In the module:
- **Settings**: the feature's struct (`#[serde(default)]`, so settings saved without a new field read it as its
  default). Add fields freely; the `Amount` slider of the stub may be kept, renamed or replaced.
- **Sliders**: `CONTROLS`, one `Control` per value: id `retouch.<KEY>.<field>` (camelCase, like every develop
  control), a Title Case label, the range and the default that leaves the picture alone (`Spec::amount` for 0…100,
  `Spec::signed` for −100…+100). Each becomes a develop control (section Portrait): the control channel, MCP,
  presets and copy / paste reach it with no other change. `is_off` defaults to "every slider at its default";
  override it when the feature has flags.
- **`apply`**: retouch the image in place (see the rules below). The panel section draws its sliders with
  `cx.slider("retouch.<KEY>.<field>")`, which writes to every face or to the picked one.

Never edit `lib.rs`, `settings.rs`, `run.rs` or the panel list (`portrait.rs`): they already list every feature.

Try a feature without building the app:

```bash
cargo run -p lightcraft-retouch --example skin -- in.jpg out.jpg --amount 50 --face 0.4,0.2,0.6,0.5
```

`--face x0,y0,x1,y1` gives a face box in fractions of the picture (repeat it for more); the examples have no
landmarks or masks unless a feature's example adds a way to load them.

## The input contract (`src/input.rs`)

The engine builds the input from a provider (`crates/engine/src/retouch_input.rs`, trait `FaceProvider`); this
crate only reads it.

- `Input::faces`: one `Face` per face, in the provider's order (which may change between detections).
- `Face::bounds`: the face box, normalised to the photo **before its crop** (0…1 across width and height, y down,
  after orientation, lens corrections and transform: the frame Remove spots use).
- `Face::landmarks`: normalised points; the first five are always YuNet's (the subject's right eye, left eye,
  nose tip, right and left mouth corner); a provider with more appends them after these.
- `Face::person`: the name of the photo's named face (People) the face overlaps, trimmed and lower case.
- `Face::masks`: `(Part, PartMask)` pairs, at most one per `Part` (Entire Person, Facial Skin, Body Skin, Hair,
  Eyebrows, Eye Whites, Iris, Lips, Teeth, Clothes Top, Clothes Bottom, Dress, Shoes, Hat, Bag, Others). A
  `PartMask` is a float `Plane` (0…1) at any resolution, covering `area` (a normalised rectangle) of the photo;
  0 outside it. Empty today: the YuNet provider gives boxes and landmarks only, and the people parser (#81)
  installs its own provider with masks (`FaceStore::set_provider`).
- `Input::fingerprint`: a hash of all of it, in the render cache keys.

A feature reads the faces through `Scene`: each `FaceCtx` has the box and landmarks in the image's pixels and
`size`, the face's size in pixels; `Scene::mask_at` / `Scene::mask_plane` sample a part mask in the image's pixels.

## Rules every feature follows

- **Sizes are fractions of the face** (`FaceCtx::px(fraction)`), never pixel counts: the preview, the loupe and
  the export then match.
- **Off is exact**: an off feature never runs, and a render with every feature off is byte for byte the render
  without the block (tested in `crates/pipeline/src/retouch_step.rs`).
- **Pixels outside the feature's mask never change**; a face without the masks a feature needs is left alone.
- **No panic** on any input: no faces, 100 faces, a face past the edge, a box that is not a box, a 1 × 1 or empty
  image, a malformed mask. Each feature's test file runs these through `tests/common`.
- **Models run once per photo**, on a worker thread with an activity row, cached; a slider drag never runs a model.
  `apply` itself runs no model.
- The image is scene-linear RGB (linear Rec.2020 primaries in the app; the examples use linear sRGB).
- A windowed render (the zoomed loupe) gives `apply` a part of the frame: `Scene` maps faces into it, and a feature
  that reads far around a face reads only what the window holds.

## Settings

`Retouch` is the develop settings' `fork_retouch` block (a namespace of this fork's own: a build without it, or a
malformed block, reads it as off; it is not written while it is the default):

- `global`: a `Features` struct, one field per feature, for every face.
- `faces`: faces with their own settings (`FaceOverride`): the face's key (`FaceKey`: its person when named, and
  its box) and a whole `Features`, copied from `global` when the face first gets its own.
- Matching (`overrides::resolve`, independent of detection order): a named face takes its person's settings;
  another face takes the unclaimed settings whose box centre is nearest, within half that box; settings made for a
  person never go to a face named as someone else.
- Control ids: `retouch.<feature>.<field>` (every face), `retouchFace.<i>.<feature>.<field>` (the `i`-th face with
  its own settings).

## Render order

One step, in the pipeline's scene-linear stage (`crates/pipeline/src/retouch_step.rs`): after white balance, spots
and red eye, before noise reduction; global tone and colour, local masks, detail and effects follow in the finish
pass (they are one per-pixel pass there, so the step sits before all of them). Stages, in order:

1. **Retouch** (tone and colour), features in this order: Blemishes, Skin, Wrinkles, Skin Tone, Eyes, Teeth and
   Lips, Glasses Glare, Stray Hair, Clothes Color.
2. **Geometry**: Face Reshape, a geometry step next to Transform. Its slot is reserved and not run yet; the reshape
   feature wires it into the pipeline's geometry.
3. **Background**: after every retouch, once for the whole picture.

The step is skipped, adding nothing to any cache key, while the block is off or the Portrait section's eye is off.
