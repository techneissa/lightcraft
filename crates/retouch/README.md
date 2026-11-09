# lightcraft-retouch

Portrait retouching for LightCraft (fork feature: `fork/retouch-foundation.md`, issues #79 and #80): the settings,
the face input contract and the retouching step. A leaf crate: it depends on `lightcraft-raster`, `serde` and
`serde_json` only, never on a model, the engine, develop, pipeline or the UI.

## Adding a feature

Each feature owns one module and touches nothing else in this crate:

| Feature | Module | Example | Tests | Section of the Portrait tab |
|---|---|---|---|---|
| Blemishes | `src/blemish.rs` | `examples/blemish.rs` | `tests/blemish.rs` | `crates/ui-egui/src/panels/portrait/blemish.rs` |
| Skin | `src/skin.rs` | `examples/skin.rs` | `tests/skin.rs` | `…/portrait/skin.rs` |
| Wrinkles | `src/wrinkles.rs` | `examples/wrinkles.rs` | `tests/wrinkles.rs` | `…/portrait/wrinkles.rs` |
| Skin Tone | `src/skin_tone.rs` | `examples/skin_tone.rs` | `tests/skin_tone.rs` | `…/portrait/skin_tone.rs` |
| Eyes | `src/eyes.rs` | `examples/eyes.rs` | `tests/eyes.rs` | `…/portrait/eyes.rs` |
| Teeth and Lips | `src/teeth_lips.rs` | `examples/teeth_lips.rs` | `tests/teeth_lips.rs` | `…/portrait/teeth_lips.rs` |
| Glasses Glare | `src/glare.rs` | `examples/glare.rs` | `tests/glare.rs` | `…/portrait/glare.rs` |
| Stray Hair | `src/hair.rs` | `examples/hair.rs` | `tests/hair.rs` | `…/portrait/hair.rs` |
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
- **Parts**: `PARTS`, the part masks `apply` reads. The face search keeps only the parts some feature lists
  (`parts_read`), so list every part you read. The default, `FACE_PARTS`, is a face's parts and its skin, without
  Entire Person and the clothes.
- **`apply`**: retouch the image in place (see the rules below). The panel section draws its sliders with
  `cx.slider("retouch.<KEY>.<field>")`, which writes to every face or to the picked one.

Never edit `lib.rs`, `settings.rs`, `run.rs` or the tab's list of sections (`crates/ui-egui/src/panels/portrait.rs`):
they already list every feature. The tab's Portrait Presets section (`panels/portrait/presets.rs`) has no module here.
Clothes recolouring is not one of them: it is a local-mask effect (issue #83), so it works on any mask; the clothes
parts of `Part` are there for its masks.

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
  after orientation, lens corrections and transform: the frame Remove spots use). With `Input::on_photo` (faces from
  the people parse) every coordinate is before lens corrections and transform instead (after the orientation), and
  the step maps it through their warp, per pixel, as local masks drawn on the photo are.
- `Face::upright`: the same box on the photo before the user's rotation (Rotate Left / Right, flips), when the
  provider knows it; per-face settings are keyed and matched by it (`Face::key_bounds`). Lens corrections and
  transform are not undone: `Face::key_frame` says whether the box is after them (`KeyFrame::Edit`, YuNet) or before
  them (`KeyFrame::Photo`, the people parse).
- `Face::landmarks`: normalised points; the first five are always YuNet's (the subject's right eye, left eye,
  nose tip, right and left mouth corner); a provider with more appends them after these.
- `Face::person`: the name of the photo's named face (People) the face overlaps, trimmed and lower case.
- `Face::masks`: `(Part, PartMask)` pairs, at most one per `Part` (Entire Person, Facial Skin, Body Skin, Hair,
  Eyebrows, Eye Whites, Iris, Lips, Teeth, Clothes Top, Clothes Bottom, Dress, Shoes, Hat, Bag, Others). A
  `PartMask` is a float `Plane` (0…1) at any resolution, covering `area` (a normalised rectangle) of the photo;
  0 outside it. While the chosen people-parts set is downloaded, faces come from the photo's people parse with every
  part the parser found that some feature reads (`Feature::PARTS`), at the resolution the models made them
  (`crates/engine/src/retouch_input/parts.rs`, `fork/retouch-masks.md`); without it, YuNet gives boxes and five
  landmarks and no masks.
- `Input::fingerprint`: a hash of all of it, in the render cache keys (the engine hashes it on the search's worker).

A feature reads the faces through `Scene`: each `FaceCtx` has the box and landmarks in the image's pixels and
`size`, the face's size in pixels; `Scene::mask_at` / `Scene::mask_plane` sample a part mask in the image's pixels,
and `Scene::mask_box` gives the pixels a mask can touch (read and write only those). Skin Tone's Amount is a stand-in
Face Light (Facial Skin brightened by up to half again) that shows the masks arrive; its feature lane replaces it.

## Rules every feature follows

- **Sizes are fractions of the face** (`FaceCtx::px(fraction)`), never pixel counts: the preview, the loupe and
  the export then match.
- **Off is exact**: an off feature never runs, and a render with every feature off is byte for byte the render
  without the block (tested in `crates/pipeline/src/retouch_step.rs`).
- **Pixels outside the feature's mask never change**; a face without the masks a feature needs is left alone.
- **No panic** on any input: no faces, 100 faces, a face past the edge, a box that is not a box, a 1 × 1 or empty
  image, a malformed mask. Each feature's test file runs these through `tests/common`.
- **Models run once per photo**, on a worker thread with an activity row, cached; a slider drag never runs a model.
  `apply` itself runs no model. See the speed target below.
- The image is scene-linear RGB (linear Rec.2020 primaries in the app; the examples use linear sRGB).
- A windowed render (the zoomed loupe) gives `apply` a part of the frame: `Scene` maps faces into it, and a feature
  that reads far around a face reads only what the window holds.

## Speed target

Every feature lane builds to this, and its PR reports both numbers below:

- **A slider is a GPU step on cached masks.** Moving a slider re-runs only the feature's per-pixel work on masks
  that are already there: no model runs while dragging, and the draft render keeps the loupe's slider budget.
- **Models run once per photo, ahead of time.** People parsing (#81) and face matching run once per photo, before
  the photo is opened: on import and while the app is idle, on Core ML where the model has a Core ML file (see
  `fork/coreml.md`), else candle. Opening a photo in the Portrait tab, or applying a portrait preset, then finds the
  masks ready.
- **Report**: the model time per photo (the parse and the match, on the Mac it was measured on, with the model set
  and Core ML or candle) and the slider time (one draft render with the feature on, against the same render with
  it off). Measure with `LIGHTCRAFT_PROFILE=1` or a bench, never in a test.

Running ahead of time is not wired yet. Today faces are found when the Portrait tab opens on a photo and kept in
memory, one entry per photo, for the photos used last within 256 MB. Faces from the people parse are already in
step 1's frame: they are kept by the file, the orientation and the parse's version, so lens, transform and crop
drags reuse them. What it takes:

1. **One frame for faces.** Find faces (and their masks) on the photo as the background face scan sees it
   (upright, uncropped, default settings, 2048 px: `crates/engine/src/faces_worker.rs`, the frame people parsing
   uses too), and map them into the edit's frame (orientation, lens corrections, transform) when a render needs
   them. A result then no longer depends on the edit, and per-face settings can be keyed by that frame.
2. **A disk cache** of each photo's faces, keyed by the file's pixels and the provider's version, like people
   parsing's (`~/Library/Caches/LightCraft/people-parsing`), so the work survives a restart and the memory cache
   only holds what is open.
3. **A queue in the background face scan** (`faces.pump`, paced by what the user is doing): photos imported or not
   yet parsed get their faces and masks while the pace allows, after the scan's own work for the photo, reusing its
   render.

## Settings

`Retouch` is the develop settings' `fork_retouch` block (a namespace of this fork's own: a build without it, or a
malformed block, reads it as off; it is not written while it is the default):

- `global`: a `Features` struct, one field per feature, for every face.
- `faces`: faces with their own settings (`FaceOverride`): the face's key (`FaceKey`: its person when named, its box
  on the photo before the user's rotation, and the frame of that box, `frame`, written only when it is `photo`) and
  a whole `Features`, copied from `global` when the face first gets its own.
- Matching (`overrides::resolve_in`, independent of detection order): a named face takes its person's settings;
  another face takes the unclaimed settings whose box centre is nearest, within half that box; settings made for a
  person never go to a face named as someone else. A key and a face of different frames are compared after the
  photo's `KeyMap` (its lens corrections and transform; the step's `View::key_map`, the engine's
  `Session::retouch_key_map`) takes the one after them to before them; without a map they pair by person only.
- Control ids: `retouch.<feature>.<field>` (every face), `retouchFace.<i>.<feature>.<field>` (the `i`-th face with
  its own settings).

## Render order

One step, in the pipeline's scene-linear stage (`crates/pipeline/src/retouch_step.rs`): after white balance, spots
and red eye, before noise reduction; global tone and colour, local masks, detail and effects follow in the finish
pass (they are one per-pixel pass there, so the step sits before all of them). Stages, in order:

1. **Retouch** (tone and colour), features in this order: Blemishes, Skin, Wrinkles, Skin Tone, Eyes, Teeth and
   Lips, Glasses Glare, Stray Hair.
2. **Geometry**: Face Reshape, a geometry step next to Transform. Its slot is reserved and not run yet; the reshape
   feature wires it into the pipeline's geometry.
3. **Background**: after every retouch, once for the whole picture.

The step is skipped while the block is off or the Portrait tab's eye is off; the scene-linear stage's key then
hashes a 0 for it (in-memory stage caches only).
