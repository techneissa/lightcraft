# Portrait retouching: part masks from the people parse

List: portrait retouching gets each face's part masks (skin, hair, brows, eyes, lips, teeth, clothes) and face mesh from the cached people parse, mapped through rotation, lens corrections and transform; Skin Tone's Face Light showed them first. Branch `feat/retouch-masks`.

## What it does

The link between people parsing (`fork/people-parsing.md`) and the retouch step (`fork/retouch-foundation.md`).

- **Faces come from the parse** while the chosen people-parts set is downloaded: per face its box, YuNet's five
  landmarks then the 478 of the face mesh (the mesh's irises, nose tip and mouth corners stand in for YuNet's five
  when YuNet is not installed), and every part mask the parser found that a feature reads (`Feature::PARTS`; by
  default Facial Skin, Body Skin, Hair, Eyebrows, Eye Whites, Iris, Lips, Teeth and Others; Entire Person for
  Background; never the clothes). People without a face (seen from behind) are
  not faces. Without the set, faces come from YuNet as before (boxes, no masks).
- **One parse per photo, shared**: the parse is the one `parsing.run` and `parsing.mask` read (same picture, same
  faces, same cache on disk). A photo parsed before (ahead of time or by a request) is read through the parse-ahead
  index with no model run, and a parse the search makes is filed there (`fork/parse-ahead.md`). Faces found from it are kept under the parse's version, so another model set or
  hair-edge choice finds them anew.
- **Never in a render**: a preview of a photo without its faces renders on no faces (the original pixels with Face
  Light on) and starts the face search; the search's worker renders the parse picture, runs YuNet and waits for the
  parse job ("Finding people parts" row). Once the faces are there, the next preview has the masks. Exports and
  synchronous renders wait for the parse. A slider drag only samples the cached masks.
- **Resolution independent**: masks keep the resolution the models made them at, in fractions of the photo; every
  render samples them at its own pixels (bilinear), so a draft, the loupe and a 24 MP export agree. Several planes of
  one part are merged once, at the finest plane's resolution (at most 1 536 samples a side and 1 M in all, so a
  damaged parse can't size it past that).
- **The frame**: the parse is made on the photo before rotation, lens corrections and transform. Faces are turned by
  the photo's orientation (samples moved, not resampled) and marked `Input::on_photo`; the step maps them through the
  lens and transform warp per pixel, as it maps local masks drawn on the photo.
- **Kept by the file, the orientation and the parse's version** (not lens, transform or crop): the parse is made on
  the photo with default settings, so a Geometry, Lens or Straighten drag, or a preset's thumbnail, finds the same
  faces and starts no search. One search runs per photo (a render asking meanwhile is redrawn when it ends); the
  faces kept take at most 256 MB, one entry per photo; their fingerprint is hashed on the search's worker. A search
  parses with the model set and hair edges chosen when it started, the version its key names.
- **Per-face settings** stay keyed as before: a face named after the photo's face regions (People) by its person,
  another by its box before the user's rotation (now also before lens corrections and transform). Each key keeps
  its frame (`FaceKey::frame`: after lens and transform for YuNet's faces, before them for the parse's); a key and a
  face of different frames are compared through the photo's lens and transform, so settings made under YuNet stay
  with their face once the parse finds it, and never go to the neighbour standing where the YuNet box lay.
- **Proof feature**: Skin Tone's Amount brightened Facial Skin to show the masks arrive; Skin Tone
  (`fork/retouch-skin-tone.md`) replaced it with Face Light, and the tests here use Face Light.
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

## Fixes from the review of #122

- Faces from the parse are kept by the file, the orientation and the parse's version, not by geometry, lens and crop
  angle: those drags no longer render without faces and start a search per tick, and preset thumbnails keep the
  retouch. One background search per photo; a render that asked meanwhile is redrawn when it ends.
- The Portrait panel and its face picker read the faces through the store's `Arc` instead of copying every mask
  plane each frame.
- A model-set switch during a search no longer files the new set's masks under the old set's key.
- The face store trims by bytes (256 MB), one entry per photo, keeps only the parts a feature reads, and hashes the
  masks on the search's worker.
- A sliver plane can't size a merged mask to gigabytes.
- Per-face keys keep their frame and are compared through the lens and transform.

## Branch

`feat/retouch-masks`

## Own files

`crates/engine/src/retouch_input/parts.rs`, `crates/engine/src/retouch_input/parts/tests.rs`, `fork/retouch-masks.md`,
`fork/tests.d/retouch-masks.txt`, `crates/retouch/tests/frames.rs`

## Hooks in official files

- `crates/pipeline/src/spots_keep.rs` (the step after red eye), `crates/engine/src/cmd/face_models.rs` (install and
  remove forget the kept reasons), `crates/ui-egui/src/panels/strip.rs` (`fit`), and the doc of
  `crates/ui-egui/src/panels/tool_strip_order.rs`. In fork files:
- `crates/retouch/src/input.rs` (`Input::on_photo`, `Input::new_on_photo`, in the fingerprint; `KeyFrame`,
  `Face::key_frame`, `KeyMap`), `src/feature.rs` (`Scene::mask_box`, `Feature::PARTS`, `FACE_PARTS`,
  `View::key_map`), `src/settings.rs` (`FaceKey::frame`, the `_in` matchers, `FeatureInfo::parts`),
  `src/overrides.rs` (`resolve_in`), `src/run.rs`, `src/lib.rs` (`parts_read`), `src/skin_tone.rs` (Face Light,
  its parts), `src/background.rs` and `src/reshape.rs` (their parts), `tests/skin_tone.rs`, `tests/common/mod.rs`,
  `README.md` (the contract).
- `crates/pipeline/src/retouch_step.rs` (`PhotoView`: on-photo faces through the warp; `FrameKeyMap`).
- `crates/engine/src/retouch_input.rs` (`mod parts`, `Source` and `Finder`: the parse as the face source while its set
  is downloaded; the store key with the parse's version; `arrange`; the byte budget, one search per photo,
  `retouch_key_map`), `crates/engine/src/cmd/retouch.rs` (matching through the key map),
  `crates/ui-egui/src/panels/portrait.rs` and `portrait/overlay.rs` (faces through the `Arc`, the key map).
- `crates/engine/src/people_parsing.rs` (`PeopleParsing::version`, `ready`, `choice`, `parse_as`); `crates/engine/src/cmd/parsing.rs`
  (`prepare` split into `prepare_job` and `Prepare::run`, so a worker makes the very picture `parsing.run` does; the
  search uses `prepare_ahead`, which loads YuNet on its worker).

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
installed or removed; the strip's top group clear of the bottom icons from 440 to 700 pt; Geometry, Lens Distortion
and Straighten drags keeping the parse's faces with no search started, and a preset thumbnail retouched; one search
per photo, with the preview that asked meanwhile redrawn; a model-set switch during a search keeping the first set's
faces; the store's byte budget and one entry per photo; a sliver plane within the merged mask's cap; no clothes masks
kept; a face's own settings made under YuNet staying with it under the parse across a strong lens distortion, not
going to the neighbour at its YuNet place (engine and crate level); a crop and a lens distortion, draft against a
render four times its size. A fake runner stands in for the models.
