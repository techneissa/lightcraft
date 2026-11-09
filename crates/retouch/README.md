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
- **Reach**: `REACH`, how far past its face's box and the areas of its `PARTS` masks `apply` reads, a fraction of
  the face's size (`DEFAULT_REACH`, half the face, when it declares none). See the zoom rule below.
- **Where it writes**: a face feature changes only pixels inside its face's read box (the face's box, its parts'
  masks, its spots and strands, grown by `REACH`), so a slider tick redoes only those boxes. One that writes elsewhere
  (strokes the user paints, the whole picture) overrides `writes_near_face` to return `false` while it does.
- **`apply`**: retouch the image in place (see the rules below). The panel section draws its sliders with
  `cx.slider("retouch.<KEY>.<field>")`, which writes to every face or to the picked one.

- **Refine**: nothing to do. Every feature gets the Refine brush (Reduce, Restore, Add; `src/refine.rs`,
  `fork/refine-brush.md`) from the step and the tab's section loop. Add widens `Feature::ADD_PARTS`, by default the
  first of `PARTS`; set it when another part is the one a missed spot belongs to (one line).

Never edit `lib.rs`, `settings.rs`, `run.rs` or the tab's list of sections (`crates/ui-egui/src/panels/portrait.rs`):
they already list every feature. The tab's Portrait Presets section (`panels/portrait/presets.rs`) has no module here:
a portrait preset carries, per feature, the fields that are numbers, switches or short lists of numbers (a colour), so
a new slider or switch is in presets with no change; lists of points or shapes (Refine strokes, spots) and fields
named like them never are (`carry` in `crates/engine/src/portrait_presets.rs`, `fork/portrait-presets.md`).
Clothes recolouring is not one of them: it is a local-mask effect (issue #83), so it works on any mask; the clothes
parts of `Part` are there for its masks.

Try a feature without building the app:

```bash
cargo run -p lightcraft-retouch --example skin -- in.jpg out.jpg --amount 50 --face 0.4,0.2,0.6,0.5
```

`--face x0,y0,x1,y1` gives a face box in fractions of the picture (repeat it for more); the examples have no
landmarks or masks unless a feature's example adds a way to load them. Masks for them come from the people parser's
example, which writes each person's part masks as grey pictures (`outdir/masks/<part>-<person>.png`):

```bash
cargo run -p lightcraft-parsing --release --example parts -- in.jpg outdir/
cargo run -p lightcraft-retouch --release --example skin -- in.jpg out.jpg --smoothness 60 --shine 50 \
  --mask outdir/masks/facialSkin-0.png
cargo run -p lightcraft-retouch --release --example skin_tone -- in.jpg out.jpg --even 60 --redness 50 \
  --masks outdir/masks --person 0
```

For what the app's step gets for each face (box, YuNet's five then the 478 mesh landmarks, the model's part masks,
merged as the app merges them), the parser's `face_input` example writes `dir/faces.json`; the eyes and wrinkles
examples read it:

```bash
cargo run -p lightcraft-parsing --release --example face_input -- in.jpg dir/
cargo run -p lightcraft-retouch --release --example eyes -- in.jpg out.jpg --faces dir/faces.json --dark 60
cargo run -p lightcraft-retouch --release --example wrinkles -- in.jpg out.png --faces dir/faces.json --amount 60
```

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
- `Face::spots`: the spots on the face's skin (Blemishes: centre, radius as a fraction of the face, kind, strength
  and a donor patch), found once per photo by the provider on the picture its masks come from
  (`blemish::with_spots`; the engine does it with the people parse, so they are kept with the masks). `None`: not
  looked for, and Blemishes looks on the image it retouches (the examples and tests).
- `Face::strands`: the stray strands around and across the face (Stray Hair: a grid over an area of the photo, kept
  only for the tiles holding a strand, each cell a weight, a direction across the strand and a polarity), found once
  on the picture the masks come from (`hair::with_strands`; the engine does it with the people parse). `None`: not
  looked for yet; Stray Hair then changes nothing (a render never searches).
- `Input::fingerprint`: a hash of all of it, in the render cache keys (the engine hashes it on the search's worker).

A feature reads the faces through `Scene`: each `FaceCtx` has the box and landmarks in the image's pixels and
`size`, the face's size in pixels; `Scene::mask_at` / `Scene::mask_plane` sample a part mask in the image's pixels,
and `Scene::mask_box` gives the pixels a mask can touch (read and write only those).

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
- **Zoom never changes a result.** A feature declares `Feature::REACH`: how far past its face's box and the areas
  of its `Feature::PARTS` masks it reads pixels, as a fraction of the face's size (`DEFAULT_REACH`, half the face,
  when it declares none). A zoomed render whose window touches what a face's features read renders all of it and
  cuts back to the window (`window_for_reads` in `crates/pipeline/src/retouch_step.rs`, as the joint colour noise
  reduction's halo does), so `apply` sees the same pixels around the face as in a whole-frame render. A feature
  keeps its reads within its `REACH` (clamp what landmarks or masks could throw far away), and works on grids and
  boxes anchored to the face (its box, an eye, a mask's area), never to the image's corner, which a window moves.
  The window also holds the reads of every face whose reads touch it, in a chain: features run one after another
  over every face, so a face's later feature reads what an earlier one changed for its neighbour. Within one face
  the largest `REACH` is enough: the window holds every pixel any of its features reads.
- **A big face is never skipped.** A feature that caps the pixels it works on at full size (Skin and Wrinkles:
  `MAX_BOX`, about 67 M) works on a bigger box reduced by a whole factor, as a draft of it, and carries the change
  back as a gain (`src/reduced.rs`), so a very large export makes the change its draft shows.
- **Reference stats come from the whole face**, never from the pixels a render happens to hold: a level, a target
  colour or the skin's colour (Eyes: the whites' level, the cheek's colour, the skin's colour; Teeth and Lips: the
  lips' main colour) is measured on a fixed grid over the face's box or a mask's area, in photo coordinates, so a
  draft, the loupe and an export measure the same samples.

## Done means

A feature lane is checked against this list, and its test file (`tests/<feature>.rs`) proves each line:

- **Off is the original**: the feature at its default, and every slider at its default (0), gives back the original
  pixels bit for bit.
- **Nothing outside the mask changes**: pixels where the feature's masks are 0 keep their bits, and a face without
  the masks it needs is left alone.
- **Preview equals export**: a draft and an export several times its size make the same change once the export is
  scaled down (sizes are fractions of the face).
- **Zoom equals the whole frame**: a render of a window holding one eye or one face part (the zoomed loupe) equals
  the whole-frame render inside it (tested in `crates/pipeline/src/retouch_step.rs`, within one 8-bit level).
- **Texture is kept**: a retouch that smooths keeps the fine detail (the high-frequency energy inside the mask stays
  within a set ratio of the original's).
- **No halo**: pixels next to the mask's edge do not take the colour of what is outside it.
- **Zoom**: a render of a window holding part of the face, grown by the reach, equals the whole render inside it.
- **Edge cases**: no faces, 50 faces, a face cut by the photo's edge, a 1 × 1 photo, a flat grey photo (stays flat
  when nothing in it calls for a change) and a missing model (faces without masks) run without a panic and give the
  expected result.
- **Speed**: the PR reports the slider time (see the speed target below); a slider update on cached masks stays
  within the draft's 16 ms.

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
2. **Geometry**: Face Reshape, a geometry step next to Transform. Not run by the step: `reshape::Field` is a warp
   of the photo before its lens corrections (`crates/pipeline/src/reshape_step.rs` puts it into the frame's warp),
   so the one resample, masks drawn on the photo, the faces and their part masks all follow the reshaped face.
3. **Background**: after every retouch, once for the whole picture.

The step is skipped while the block is off or the Portrait tab's eye is off; the scene-linear stage's key then
hashes a 0 for it (in-memory stage caches only).
