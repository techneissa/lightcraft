# ⌥-drag slider previews on a mask's sliders

## What it does

Official shipped Lightroom's Alt-drag slider previews (PR #999: `Overlay::for_control`, `ui.dragging_control`; merged 2026-11-01). The fork's own implementation (`view.clippingPreview`, `Preview`, `tests_clip_preview.rs`) was dropped on that merge. What stays: ⌥ held during a drag of a mask's Exposure / Whites / Highlights / Blacks / Shadows (Masking panel, bare ids `exposure`…) shows the same preview as the Edit panel's sliders; the mask tint, outlines and pins step aside while it shows.

## Branch

`claude/alt-drag-clipping-preview` (the original; the mask-slider part is in `custom`)

## Own files

`alt_overlay` and `mask_slider_overlay` in `crates/ui-egui/src/panels/clipping.rs`, `crates/ui-egui/src/tests/clip_preview.rs`

## Hooks in official files

`panels/detail.rs` (`view_overlay` asks `clipping::alt_overlay`), `panels/detail/masks.rs` (outlines and pins hidden under a preview), `docs/parity-checklist.md` (LR-EDIT-ALTDRAG), `docs/whats-new.md`

## Tests

`clip_preview` in ui-egui (a mask's sliders, the clipping overlay while a mask is edited, a hovered triangle hiding the mask tint), `a_masks_sliders_ask_for_their_preview`
