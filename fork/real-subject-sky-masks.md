# Real Subject, Sky and Background masks

## What it does

Subject, Sky and Background (Masking ▸ Create New Mask, and the part buttons under a mask) use SAM 3 when it is installed (or a remote worker is configured), as Object and Describe do (issue #37):

- Subject is what SAM 3 finds for `person | animal | vehicle | subject, main object, foreground` (people first), Sky is `sky`, Background is the subject turned around (everything else).
- The segmentation is stored with the mask (as Object and Describe store theirs), so previews, exports and reopening never need the model. Edge works on all three; Subject and Sky get the zoomed-in detail pass.
- Like Describe, the mask is added when SAM 3 has found it, with a row "Selecting with SAM 3" in the activity stack (✕ cancels it) and the "Selecting…" line under the tiles; no rough mask shows meanwhile. Nothing found is a message, never a rough mask.
- A photo with an embedded matte for the kind (a DNG's sky or person matte) keeps the matte: the model is not run and the matte wins when drawing.
- Without SAM 3 the mask is made the old way (classical heuristic) at once, and the part shows one line, "Rough mask. Click to install SAM 3.", that opens the download. A mask saved before keeps its look; with SAM 3 installed its line reads "Rough mask. Add it again to use SAM 3."
- Cancelling: deleting the mask, undo, or a photo change cancels the job (from the official PR).

## Branch

`feat/37-real-subject-sky-masks`

## Official PR taken

storytold/lightcraft #3109 ("AI masking with SAM 3, startup model pre-loading & Sony a6400 lens support"), merged with its commits (`git log --oneline origin/pr-3109`). It is the only open PR that does all three kinds on SAM 3 and keeps the result with the mask (its shapes `Sky`, `Subject`, `Background` carry `seg`, `detail`, `edge`); #1219 does Sky only, on top of unmerged #1079; #1227 needs a BLIP caption model nobody has; #1322 uses no model; #2706 brings five other models (one of 885 MB, two with an undecided licence) and 15,000 lines. Left out of the merge, as not part of this feature: the Sony lens support (`lens_profiles.rs`, `raw/`, `meta/focus*`, the Optics panel text), the `Cargo.toml` profile changes, the denoise and SAM start-up warm-up (`segment_warmup` is in the code but nothing calls it), the locale arrow change. Changed from it: a stored segmentation does not outrank an embedded matte; the mask is added when found, not first (its `target` code is gone); nothing found is an error; the Activity row.

## Own files

`crates/engine/src/cmd/concepts.rs` (what the model is asked for, whether a mask uses it, how it is stored; `Session::is_rough_ai_mask`), `crates/ui-egui/src/panels/rough_mask.rs`, `crates/ui-egui/src/tests/rough_mask.rs`

## Hooks in official files

Where an official update should look:

- `crates/engine/src/cmd.rs`: `pub(crate) mod concepts;`.
- `crates/engine/src/cmd/masks.rs`: `resolve_ai` has one arm for Subject, Sky and Background without a stored selection (`concepts::wanted`, `concepts::store`); `mask.add` and `mask.addComponent` queue a `Later::Concept` like a `Later::Text`; `queue_later` does nothing for it.
- `crates/engine/src/segment.rs`: `InFlight` holds the job's `TaskGuard` (activity kind `segment`, `activity_label`); `Tag::NewConcept` has no `target`; `segment_concept_later`; `apply_outcome` for it; the two concept tests.
- `crates/engine/src/media/cache.rs`: `has_matte`. `crates/engine/src/activity.rs`: the `segment` kind in a doc line.
- `crates/pipeline/src/masks.rs` (`shape_alpha`): mattes first, then a stored segmentation, then the heuristic; the test `embedded_mattes_win_over_a_stored_segmentation` in `masks/tests.rs`.
- `crates/ui-egui/src/panels/masking.rs` (`range_controls`): an arm for the three kinds without a selection calls `rough_mask::line`. `crates/ui-egui/src/panels.rs`, `tests.rs`: `mod` lines. `panels/mask_parts.rs`: one test helper uses `MaskShape::subject()`.
- `crates/ui-egui/locales/*.json`: three strings.
- `docs/ai-masks.md`, `docs/parity-checklist.md` (LR-MASK-SUBJECT, -SKY, -BACKGROUND), `docs/whats-new.md`.

## Tests

`cargo test -p lightcraft-engine --features sam --lib -- segment:: concepts::` · `cargo test -p lightcraft-pipeline --lib -- masks::` · `cargo test -p lightcraft-ui-egui --lib -- rough_mask`

## When upstream merges

If #3109 is merged upstream: the update script merges it over this merge; expect conflicts in `segment.rs`, `cmd/masks.rs`, `pipeline/src/masks.rs` and `tests_window.rs`. Keep upstream's code except the changes listed under "Changed from it" (above) and `concepts.rs`. If it merges whole (with the Sony lens support and warm-up), take those too, and drop what this note says was left out. If upstream merges something else that makes Subject, Sky and Background model-based (#1219, #2706), keep whichever stores its selection with the mask and re-apply the matte-first order, the activity row and the rough-mask line.
