# Portrait retouching: part masks from the people parse

List: portrait retouching gets each face's part masks (skin, hair, brows, eyes, lips, teeth, clothes) and face mesh from the cached people parse, mapped through rotation, lens corrections and transform; Skin Tone's Amount lights Facial Skin to show them. Branch `feat/retouch-masks`.

## What it does

The link between people parsing (`fork/people-parsing.md`) and the retouch step (`fork/retouch-foundation.md`).

- **Faces come from the parse** while the chosen people-parts set is downloaded: per face its box, YuNet's five
  landmarks then the 478 of the face mesh (the mesh's irises, nose tip and mouth corners stand in for YuNet's five
  when YuNet is not installed), and every part mask the parser found (Entire Person, Facial Skin, Body Skin, Hair,
  Eyebrows, Eye Whites, Iris, Lips, Teeth, the clothes parts, Others). People without a face (seen from behind) are
  not faces. Without the set, faces come from YuNet as before (boxes, no masks).
- **One parse per photo, shared**: the parse is the one `parsing.run` and `parsing.mask` read (same picture, same
  faces, same cache on disk). Faces found from it are kept under the parse's version, so another model set or
  hair-edge choice finds them anew.
- **Never in a render**: a preview of a photo without its faces renders on no faces (the original pixels with Face
  Light on) and starts the face search; the search's worker renders the parse picture, runs YuNet and waits for the
  parse job ("Finding people parts" row). Once the faces are there, the next preview has the masks. Exports and
  synchronous renders wait for the parse. A slider drag only samples the cached masks.
- **Resolution independent**: masks keep the resolution the models made them at, in fractions of the photo; every
  render samples them at its own pixels (bilinear), so a draft, the loupe and a 24 MP export agree. Several planes of
  one part are merged once, at the finest plane's resolution (at most 1 M samples).
- **The frame**: the parse is made on the photo before rotation, lens corrections and transform. Faces are turned by
  the photo's orientation (samples moved, not resampled) and marked `Input::on_photo`; the step maps them through the
  lens and transform warp per pixel, as it maps local masks drawn on the photo.
- **Per-face settings** stay keyed as before: a face named after the photo's face regions (People) by its person,
  another by its box before the user's rotation (now also before lens corrections and transform).
- **Proof feature**: Skin Tone's Amount brightens Facial Skin by up to half again (Face Light); pixels outside the
  mask never change. The Skin Tone lane replaces it.
- `Scene::mask_box` (new): the pixels a face's mask can touch, so a feature reads and writes only those.

Mask sampling per render (Apple M4 Pro, release, load ~20; Facial Skin over 30 % × 40 % of the photo, a 512² mask):
Face Light at draft 1936 × 1291 3.4 ms, loupe 2560 × 1707 5.9 ms, 24 MP 38.6 ms; a full-frame `mask_plane` 8.2 / 13.9
/ 157 ms.

Not done: edges are not snapped to the photo with the guided filter `parsing.mask` uses (the step samples the model's
mask as it is); the face picker's thumbnails and `retouch.faces` boxes ignore lens corrections and transform.

## Fixes to the base (review of #85)

- A photo with a Remove spot is retouched in the cached render and the GPU's scene-linear stage
  (`crates/pipeline/src/spots_keep.rs` runs the step after red eye, as `crate::lin_cpu` does).
- Without a usable face model, the reason is kept per photo and shown, instead of the Portrait panel reading (and, for
  an unreadable file, parsing) the model on the UI thread every frame; installing or removing a face model
  (`crates/engine/src/cmd/face_models.rs`) forgets it.
- The tool strip's buttons and gaps shrink alike when the strip is too short for eleven icons (a window near its
  560 pt minimum), so the top group no longer runs into the two pinned to the bottom (`crates/ui-egui/src/panels/strip.rs`).
- `tool_strip_order.rs`'s doc: eleven places, five between the dividers, About after the ninth.

## Branch

`feat/retouch-masks`

## Own files

`crates/engine/src/retouch_input/parts.rs`, `crates/engine/src/retouch_input/parts/tests.rs`, `fork/retouch-masks.md`,
`fork/tests.d/retouch-masks.txt`

## Hooks in official files

- `crates/pipeline/src/spots_keep.rs` (the step after red eye), `crates/engine/src/cmd/face_models.rs` (install and
  remove forget the kept reasons), `crates/ui-egui/src/panels/strip.rs` (`fit`), and the doc of
  `crates/ui-egui/src/panels/tool_strip_order.rs`. In fork files:
- `crates/retouch/src/input.rs` (`Input::on_photo`, `Input::new_on_photo`, in the fingerprint), `src/feature.rs`
  (`Scene::mask_box`), `src/skin_tone.rs` (Face Light), `tests/skin_tone.rs`, `README.md` (the contract).
- `crates/pipeline/src/retouch_step.rs` (`PhotoView`: on-photo faces through the warp).
- `crates/engine/src/retouch_input.rs` (`mod parts`, `Source` and `Finder`: the parse as the face source while its set
  is downloaded; the store key with the parse's version; `arrange`).
- `crates/engine/src/people_parsing.rs` (`PeopleParsing::version`, `ready`); `crates/engine/src/cmd/parsing.rs`
  (`prepare` split into `prepare_job` and `Prepare::run`, so a worker makes the very picture `parsing.run` does).

## Tests

`fork/tests.d/retouch-masks.txt`. They cover: masks reaching the step for 1 and 3 faces, and `parsing.run` then
reading the same cached parse; no faces (original pixels), 50 faces and a face cut by the edge; a 1 × 1 and a flat
grey photo; masks turning with Rotate Right; a preview without a parse rendering the original pixels and asking for
the parse, then getting the masks; an export waiting for the parse; a draft and a render four times its size lighting
the same share by the same amount; pixels away from the mask unchanged (engine, pipeline and crate level, the last
bit for bit); a face's own settings staying with its person and an unnamed face's with its place when the parse comes
back in another order, and never going to someone else; another model set finding faces anew; merged and turned
planes; on-photo faces following a lens distortion (fails without the warp mapping) and matching faces after the warp
when there is none; a spot not skipping the retouch in the cached render; a missing face model kept until a model is
installed or removed; the strip's top group clear of the bottom icons from 440 to 700 pt. A fake runner stands in for
the models.
