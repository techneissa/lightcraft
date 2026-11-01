# ⌥-drag slider previews

## What it does

⌥-drag slider previews (Lightroom's Alt-drag): ⌥ held during a drag of Exposure / Whites / Highlights shows a black image with the clipped highlights, Blacks / Shadows a white image with the crushed shadows (the clipping colours of the toggles above), Sharpening Masking the sharpening mask, Amount / Radius / Detail the photo in greyscale; ends with the drag or ⌥; `view.clippingPreview {mode?, control?}` holds one for agents.

## Branch

`claude/alt-drag-clipping-preview` (on `claude/histogram-clipping-toggles`)

## Own files

`crates/pipeline/src/tests_clip_preview.rs`, `crates/ui-egui/src/tests/clip_preview.rs`, `Preview` in `crates/ui-egui/src/panels/clipping.rs`

## Hooks in official files

`crates/pipeline/src/visualize.rs` (four `Overlay` variants, `clip_view`, `greyscale`, `sharpen_mask_alpha`), `finish.rs` (`sharpen_mask_weight`), `lib.rs` (mask drawn, module), `crates/gpu/src/lib.rs` (`handles`: the sharpening mask renders on the CPU), `panels/detail.rs` + `detail/window.rs`, `widgets.rs` (`slider` reports a drag), `state.rs`, `menus/run.rs` + `menus/ui_commands.rs`, `tests.rs`, `docs/control-protocol.md`, `docs/parity-checklist.md` (LR-EDIT-ALTDRAG), `docs/whats-new.md`

## Tests

`cargo test -p lightcraft-pipeline --lib clip_preview`, `clip_preview` in ui-egui, `cargo test -p lightcraft-gpu --lib handles`
