# Brush live adjustment

## What it does

Brush live adjustment: painting with the Brush shows the mask's adjustment under the brush while the button is down (the loupe's pixels and a second render with the brush covering everything, mixed by the stroke's coverage in each frame's dirty rectangle; erase blends toward the render without the brush; Auto Mask weighs the live dabs by the picture under them; the blend stays until the committed render replaces it).

The live stroke paints with the committed stroke's maths for the photo's process (issue #369, ported from upstream PRs storytold/lightcraft#4047 and #4517): from process V9 Flow is coverage per brush radius travelled (`per_radius`), from V10 a stroke's edge is feathered as one dab's (`by_stroke`, never for Auto Mask strokes).

## Branch

`claude/brush-live-preview`

## Own files

`crates/ui-egui/src/brush_live/` (`composite.rs`, `target.rs`, `auto.rs`), `crates/ui-egui/src/panels/detail/brush_preview.rs`, `crates/ui-egui/src/tests/brush_live.rs`, `crates/ui-egui/examples/brush_live_bench.rs`

## Hooks in official files

`brush_live.rs` (`paint` takes a `Look`, `stamp_with`, `paint_held`, `hold`), `render.rs` (`Slot::BrushFull`, `keep_brush_pixels`, `brush_target`), `panels/detail.rs` (2 calls, a key salt), `panels/detail/masks.rs` (`brush_tool`), `app.rs` (`brush_live_ms`), `tests.rs`

## Tests

`brush_live`, `tests::brush_live`
