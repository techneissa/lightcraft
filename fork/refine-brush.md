# Portrait retouching: the Refine brush

List: a Refine brush under every Portrait feature's sliders: Reduce weakens the feature where painted (none at Strength 100), Restore brings back its automatic result, Add paints it where its mask missed; one undo step per stroke and Show Refine Strokes. Branch `feat/99-refine-brush`.

## What it does

Issue #99, on the base of portrait retouching (`fork/retouch-foundation.md`). Under each feature's sliders in the
Portrait tab, a **Refine** row:

- **Reduce**: paint to weaken the feature there. At Strength 100 the pixels there are what they were before the
  feature ran, bit for bit; at 50, half way.
- **Restore**: paint to take Reduce and Add back: the automatic result again.
- **Add**: paint to apply the feature where its automatic mask missed (a cheek the skin mask skipped, a tooth the
  teeth mask missed). Offered for features that read a mask.
- **Clear** removes the feature's strokes (for every face, or the picked face's own).
- While a brush is in hand: Size, Feather and Flow (the Masking brush's own, shared with it) and Strength.
- **Show Refine Strokes**, under the line that says which faces the sliders change, draws the strokes on the photo
  (Reduce red, Restore blue, Add green; another feature's fainter while a brush is in hand).

The brush is the Masking brush: pen pressure scales each dab's size and flow, ⌥ held swaps Reduce and Restore (Add
with ⌥ restores), ⌃⌥-drag resizes it, a right-click on the photo opens its Size, Feather and Flow. Clicking the
mode again puts the brush down. With a face picked on the photo, strokes go to that face's own settings (made from
those for every face, strokes included); otherwise to every face.

## How

- **One shared component.** `crates/retouch/src/refine.rs` keeps the strokes (`Refine`: per feature, in the order
  painted) and `run::run_one` wraps every feature's run: a feature without strokes runs as before, with no extra
  work; one with strokes runs through `refine::run_refined`. The panel draws the row from the tab's section loop
  (`panels/portrait.rs` → `panels/portrait/refine.rs`). A feature declares nothing; `Feature::ADD_PARTS` (the part
  masks Add widens) defaults to the first of its `PARTS`, so a feature lane changes it with one line when that is
  not the right one (Eyes: Eye Whites; Teeth and Lips: Teeth).
- **Reduce** saves the pixels under the strokes before the feature runs and mixes the result back toward them by
  the cut there (1 at a full Reduce: the saved bits). **Restore** lowers the cut and the added mask. **Add** widens
  each mask in `ADD_PARTS` (the larger of the mask and the painted coverage) for the face nearest the stroke, so
  the feature works there as on its own mask; a face without that mask gets one from Add.
- **Coordinates.** Points are normalised on the photo before its crop, after orientation, lens corrections and
  transform (the frame of Remove spots); sizes are fractions of its long edge. `View::to_edit` / `edit_to_px` map
  them; faces from a people parse (before the warp) are mapped through the warp as before.
- **Rasterised once per change.** Strokes become grids over the photo (a sixth of the smallest brush radius per
  cell, at most 1024 per side) cached by the strokes' hash; widened masks are cached too. A draft, the zoomed loupe
  and an export sample the same grids, so preview equals export, and a zoomed window grows to where Add strokes
  widen masks (`Reads::add`, `window_for_reads`).
- **Saved** in the retouch block: `fork_retouch.refine` and `fork_retouch.faces[].refine`, left out while empty;
  fork catalog format 8. Sidecar (`lc:settings`), catalog and History carry them; Copy / Paste and Sync with
  Portrait ticked replace them (copying a photo without strokes takes them away). Portrait Presets (#145) carry a
  face's sliders only, never strokes. Reset Portrait for a feature clears its strokes too.
- **Strokes alone do nothing**: a feature that is off stays off.

Commands: `retouch.refine.stroke {feature, mode, points, size?, feather?, flow?, strength?, pressure?, face?}`,
`retouch.refine.clear {feature?, face?}`, `retouch.refine.list {face?}`; UI: `portrait.refine {feature?, mode?,
strength?, show?}`; widgets `portrait:refine.<feature>.<mode>`, `portrait:refine.<feature>.clear`,
`slider:portrait.refineStrength`, `portrait:refineStrokes`.

## Limits

- Face Reshape has no Refine row yet: it moves pixels in the geometry step (next to Transform), outside the run the
  brush wraps; `retouch.refine.stroke` refuses it. Its warp would need to read the cut as a weight.
- Strokes don't turn with Rotate Left / Right or a flip (as Remove spots don't).
- Add widens masks; it can't make Blemishes find spots its once-per-photo search did not find, and it does nothing
  for a feature that reads no mask.
- A hard brush edge is softened by one grid cell (a sixth of the radius).

## Speed

`cargo run -p lightcraft-retouch --release --example refine_speed` (a 1936 × 1290 draft, Skin on one face, fastest
of 20 runs, on a test Mac under heavy load from other builds): Skin alone 7.45 ms; with a Reduce and an Add stroke
8.16 ms once the grids are made, 14.17 ms for the run that rasterises them. No model runs.

## Tests

`fork/tests.d/refine-brush.txt`: `crates/retouch/tests/refine.rs` (full Reduce gives back the original bits, a
partial one mixes half way and Restore takes it back, Add smooths past the mask, every feature gets Reduce, preview
equals export, a face's own strokes stay with it, strokes alone change nothing, JSON round trip and malformed
strokes, hostile strokes), `crates/pipeline/src/tests_retouch_refine.rs` (zoom equals the whole frame; a draft and a
render four times its size agree under a crop and a lens warp with faces before the warp),
`crates/develop/src/retouch.rs` (the Portrait group carries and clears strokes), `crates/catalog` (fork format 8),
`crates/engine/src/retouch_input/tests.rs` (commands, one undo step, copy / paste, sync, sidecar and catalog), and
`crates/ui-egui/src/tests/portrait.rs` (the row in every section, painting on the photo, the picked face, Clear).
