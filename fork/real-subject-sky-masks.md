# Real Subject, Sky and Background masks

List: Subject, Sky and Background are real SAM 3 selections stored with the mask (official PR #3109, trimmed), a rough mask says so, and a new mask is named after its first part. Branch `feat/37-real-subject-sky-masks`.

## What it does

Subject, Sky and Background (Masking ▸ Create New Mask, and the part buttons under a mask) use SAM 3 when it is installed (or a remote worker is configured), as Object and Describe do (issue #37):

- Subject is what SAM 3 finds for `person | animal | vehicle | subject, main object, foreground` (people first), Sky is `sky`, Background is the subject turned around (everything else).
- The segmentation is stored with the mask (as Object and Describe store theirs), so previews, exports and reopening never need the model. Edge works on all three; Subject and Sky get the zoomed-in detail pass.
- Like Describe, the mask is added when SAM 3 has found it, with a row "Selecting with SAM 3" in the activity stack (✕ cancels it) and the "Selecting…" line under the tiles; no rough mask shows meanwhile. Nothing found is a message, never a rough mask.
- A photo with an embedded matte for the kind (a DNG's sky or person matte) keeps the matte: the model is not run and the matte wins when drawing.
- Without SAM 3 the mask is made the old way (classical heuristic) at once, and the part shows one line, "Rough mask. Click to install SAM 3.", that opens the download. A mask saved before keeps its look; with SAM 3 installed its line reads "Rough mask. Add it again to use SAM 3."
- A new mask is named "<Kind> Mask <N>" after its first part ("Subject Mask 1", "Sky Mask 2", "Describe Mask 1", "Brush Mask 1"…): N is the lowest number no mask of the photo has for that kind, so deleting "Subject Mask 1" and adding a Subject gives "Subject Mask 1" again. A name the user typed, and the names of saved masks, are never changed. In the Masks list and the part rows a name too long for its row takes a smaller font (down to 10.5 pt), then ends with "…", the full name on hover.
- Cancelling: deleting the mask, undo, or a photo change cancels the job (from the official PR).

## Branch

`feat/37-real-subject-sky-masks`

## Official PR taken

storytold/lightcraft #3109 ("AI masking with SAM 3, startup model pre-loading & Sony a6400 lens support"), merged with its commits (`git log --oneline origin/pr-3109`). It is the only open PR that does all three kinds on SAM 3 and keeps the result with the mask (its shapes `Sky`, `Subject`, `Background` carry `seg`, `detail`, `edge`); #1219 does Sky only, on top of unmerged #1079; #1227 needs a BLIP caption model nobody has; #1322 uses no model; #2706 brings five other models (one of 885 MB, two with an undecided licence) and 15,000 lines. Left out of the merge, as not part of this feature: the Sony lens support (`lens_profiles.rs`, `raw/`, `meta/focus*`, the Optics panel text), the `Cargo.toml` profile changes, the denoise and SAM start-up warm-up (`segment_warmup` is in the code but nothing calls it), the locale arrow change. Changed from it: a stored segmentation does not outrank an embedded matte; the mask is added when found, not first (its `target` code is gone); nothing found is an error; the Activity row.

## Own files

`crates/engine/src/cmd/concepts.rs` (what the model is asked for, whether a mask uses it, how it is stored; `Session::is_rough_ai_mask`), `crates/engine/src/cmd/mask_names.rs`, `crates/ui-egui/src/panels/rough_mask.rs`, `crates/ui-egui/src/panels/mask_name.rs`, `crates/ui-egui/src/tests/rough_mask.rs`, `crates/ui-egui/src/tests/mask_name.rs`

## Hooks in official files

Where an official update should look:

- `crates/engine/src/cmd.rs`: `pub(crate) mod concepts;`, `mod mask_names;`. `crates/engine/src/cmd/masks.rs`: the generated names in `mask.add` and in `mask.brushStroke` (a stroke that starts a mask) come from `mask_names::next`.
- `crates/engine/src/cmd/masks.rs`: `resolve_ai` has one arm for Subject, Sky and Background without a stored selection (`concepts::wanted`, `concepts::store`); `mask.add` and `mask.addComponent` queue a `Later::Concept` like a `Later::Text`; `queue_later` does nothing for it.
- `crates/engine/src/segment.rs`: `InFlight` holds the job's `TaskGuard` (activity kind `segment`, `activity_label`); `Tag::NewConcept` has no `target`; `segment_concept_later`; `apply_outcome` for it; the two concept tests.
- `crates/engine/src/media/cache.rs`: `has_matte`. `crates/engine/src/activity.rs`: the `segment` kind in a doc line.
- `crates/pipeline/src/masks.rs` (`shape_alpha`): mattes first, then a stored segmentation, then the heuristic; the test `embedded_mattes_win_over_a_stored_segmentation` in `masks/tests.rs`.
- `crates/ui-egui/src/panels/masking.rs` (`range_controls`): an arm for the three kinds without a selection calls `rough_mask::line`. `panels/masking.rs` (the Masks list row) and `panels/mask_parts.rs` (the part row) set the name with `mask_name::fit` instead of `one_line`; the latter's test helper uses `MaskShape::subject()`. `crates/ui-egui/src/panels.rs`, `tests.rs`: `mod` lines.
- `crates/ui-egui/locales/*.json`: three strings.
- `docs/ai-masks.md`, `docs/parity-checklist.md` (LR-MASK-SUBJECT, -SKY, -BACKGROUND, edited in place).

## Tests

`cargo test -p lightcraft-engine --features sam --lib -- segment:: concepts:: mask_names` · `cargo test -p lightcraft-pipeline --lib -- masks::` · `cargo test -p lightcraft-ui-egui --lib -- rough_mask mask_name` (also listed in `fork/tests.d/real-subject-sky-masks.txt`).

## When upstream merges

If #3109 is merged upstream: the update script merges it over this merge; expect conflicts in `segment.rs`, `cmd/masks.rs`, `pipeline/src/masks.rs` and `tests_window.rs`. Keep upstream's code except the changes listed under "Changed from it" (above) and `concepts.rs`. If it merges whole (with the Sony lens support and warm-up), take those too, and drop what this note says was left out. If upstream merges something else that makes Subject, Sky and Background model-based (#1219, #2706), keep whichever stores its selection with the mask and re-apply the matte-first order, the activity row and the rough-mask line.

## What's New entry

### Real Subject, Sky and Background Masks
- With SAM 3 installed, Subject, Sky and Background are real selections: Subject finds the people (else an animal, a vehicle or the main object), Sky the sky, Background everything else. The mask appears when SAM 3 has found it, with a "Selecting with SAM 3" row in the activity stack that you can cancel; the selection is stored with the mask, so previews, exports and reopening need no model, and Edge works on them.
- A photo with an embedded sky or person matte (a DNG from an iPhone) keeps using it.
- Without SAM 3 the mask is the same rough one as before, and the part says "Rough mask. Click to install SAM 3." Masks saved earlier look the same.
- A new mask is named after its first part: "Subject Mask 1", "Sky Mask 1", "Brush Mask 2"; a number is never used twice for a kind. A name too long for its row gets a smaller font, then "…".
