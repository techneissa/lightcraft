# Adaptive AI masks

List: Presets, Paste, Sync and Auto Sync carry what an AI mask selects (Subject, Sky, Background, Object clicks, a description, Select People parts and whose), and each photo finds its own selection in the background; Photo ▸ Update AI Masks finds a photo's own again (issue #239). Branch `feat/239-adaptive-masks`.

## What it does

- **What a part selects, apart from what was found.** An AI mask part (Subject, Sky, Background, Object, Describe,
  Select People) keeps its kind and parameters (the description, the clicks, the parts and whose face) apart from
  the selection a model found on one photo (`seg`, `detail`). A preset stores only what it selects. Paste, Paste
  from Previous, Sync, Auto Sync and applying a preset carry it to each photo, where the part waits to be found.
- **Found on each photo.** A waiting part is found on its photo with the photo's own models: Subject, Sky and
  Background as when they are added (SAM 3 and the selection-model dropdown, or the photo's own matte, or the
  classical way without SAM 3), an Object from its clicks at the same places, a Describe part from its words, a
  Select People part from the photo's people (its face by name, else by place; everyone's parts are everyone's).
  Brush, gradient and range parts come as they were, and combine (add, subtract, intersect) with the found parts.
- **The edit lands at once.** In the app the photos' other settings change at once; the AI parts are found in the
  background, one photo at a time, with one row in the ring ("Finding AI masks", ✕ stops it). Until a part is found,
  its mask shows nothing (neither the part nor its inverse), never the first photo's selection. The models' caches
  are used: the SAM 3 worker's encoded photo, the people parse and its disk cache (which parse-ahead fills). The CLI,
  MCP and tests wait for the parts instead.
- **A photo keeps its own.** A photo that has the same part found already (the same mask, place and recipe, turned
  the same way) keeps its own selection, so pasting a look again, or moving a mask slider with Auto Sync on, finds
  nothing again.
- **Undo.** The paste, sync or preset is one undo step, also when the parts are found later: what is found goes into
  the photo's settings without an undo step of its own, and the step's redo brings back the found parts. A part that
  no longer waits is left alone. A part that changed while its photo was being found (another paste, a new
  description), or a photo turned meanwhile, is found again as it is now once that search ends. Opening another
  library stops the search: photo ids are per library, so no answer for the old library lands on the new one.
- **Nothing found** selects nothing on that photo (the face is not there, no subject, no SAM 3 for an Object or
  Describe part). The ring's summary lists the photos where a model found nothing, with why.
- **Update AI Masks** (Photo menu, `mask.updateAi {ids?}`) finds each AI part of the photos (the selection) again on
  that photo, after a crop, a retouch or an edit that moved the subject; one undo step. A part no model on this
  computer can find keeps what it has, and a part whose new search fails or finds nothing gets its selection back
  (the ring still names the photo): a selection the user had is never lost.
- **Old catalogs and presets** that carry a selection read as before. Applied to another photo, their parts are found
  there. A waiting part is saved as a selection of nothing marked `waiting`: another build draws it as nothing.
  Parts that still wait when the app quits are found when their photo is opened, or by Update AI Masks.
- **Export waits for them.** An export that includes a photo whose AI parts are still being found is refused with
  "AI masks are still being found on N of these photos", and those photos are queued (also after ✕ stopped them).

Mac only, like every fork feature.

## Branch

`feat/239-adaptive-masks`

## Own files

`crates/develop/src/adaptive.rs`, `crates/develop/src/adaptive/tests.rs`, `crates/engine/src/adaptive.rs`,
`crates/engine/src/adaptive/tests.rs`, `crates/engine/src/segment/adaptive_jobs.rs`, `crates/engine/src/cmd/adaptive.rs`,
`fork/adaptive-masks.md`, `fork/tests.d/adaptive-masks.txt`

## Hooks in official files

- `crates/develop/src/lib.rs`: `pub mod adaptive`. `crates/develop/src/presets.rs`: `Preset::from_settings` strips
  the found selections (and `preset.update` in `crates/engine/src/cmd/develop.rs`); `add_masks` marks a preset's AI parts waiting and compares parts with `same_parts`.
- `crates/pipeline/src/masks.rs`, `local.rs`, `lib.rs`, `recolor/measure.rs`, `decontam/mod.rs`, `masks/planes.rs`
  and `crates/gpu/src/render.rs`, `masks.rs`, `decontam.rs`, `dirty.rs`: the masks a render draws are
  `Mask::renders()` (visible, with parts, none waiting) instead of `visible && !components.is_empty()`. CPU and GPU
  must change together.
- `crates/engine/src/lib.rs`: `pub mod adaptive`, field `Session::adaptive`; `run_command` calls `adaptive_after` at
  depth 0 (the photos the command changed, and the photo it opened); `auto_sync_ops` calls `adapted`.
- `crates/engine/src/cmd/develop.rs` (`pasted`): `adapted` for Paste, Paste from Previous and Sync.
- `crates/engine/src/segment.rs`: `mod adaptive_jobs`; `segment_poll` calls `adaptive_poll`.
- `crates/engine/src/activity.rs`: `Activity::start_background_batch`. `crates/engine/src/library.rs`: opening a library
  calls `adaptive.stop()`.
- `crates/engine/src/cmd.rs`: `mod adaptive`, its specs.
- `crates/ui-egui/src/app.rs` (`ai_mask_detail`): repaints while `adaptive_busy`. `crates/ui-egui/src/headless.rs`
  (`busy`): `ui.settle` waits for it too. `crates/ui-egui/src/export_task.rs` (`start`): refuses while photos wait. `crates/ui-egui/locales/*.json`: two
  strings.
- Fork files of other features: `crates/engine/src/select_people.rs` (`people_find_under`, the row under the batch;
  `rebase.rs` is gone: its drawing is `adaptive::people_found`), `fork/select-people.md`.
- `docs/parity-checklist.md` (LR-MASK-UPDATE, LR-MASK-ADAPTIVE, LR-PRE-ADAPTIVE), `docs/target-app-parity.md`.

## Tests

`fork/tests.d/adaptive-masks.txt`. A finder stands in for SAM 3 (`Adaptive::set_finder`); Select People uses the
parser's test runner. They cover a preset with a Subject mask applied to a second photo (its own subject and
background, the brush part and the edits as they were), an old preset that carries a selection, Paste to two photos
in the background (the masks waiting and not drawn, the ring's row, one undo step, redo with the found parts), Update
AI Masks, a failed Update keeping the selections, nothing found and a photo keeping its own part, another library
opened mid-search, a part changed mid-search, Update Preset saving no selection; in the develop crate the recipe in a preset, waiting parts,
`adapt` and a saved waiting part.

## What's New entry

### Adaptive AI Masks
- Presets, Paste Settings and Sync now carry what an AI mask selects, not the first photo's selection. Copy a look with a subject mask and a background mask from one portrait, paste it to 200 others, and each photo gets its own subject and background.
- The edit shows at once; the masks are found in the background, with a ring in the top bar. Brush and gradient parts of a mask stay as they were.
- Photo ▸ Update AI Masks finds a photo's AI masks again, after a crop or an edit that moved the subject.
