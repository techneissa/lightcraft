# Develop sliders on one row

## What it does

Develop sliders on one row: label left, a recessed groove with eleven ticks and a teardrop thumb, the value right in a fixed-width column (editing never moves the track), long labels shrink to fit, gradient tracks keep their colours, a divider in Noise Reduction.

## Branch

`claude/slider-row-redesign`

## Own files

`crates/ui-egui/src/widgets/tests_slider_columns.rs`

## Hooks in official files

`widgets.rs` (`slider`, `paint_track`, `paint_thumb`, `slider_columns`, `shared_column_width`, `sliderThumb:` id), `theme.rs` (`slider_row_h` 30), `panels/edit.rs` (noise reduction divider), `tests/temp_slider.rs` (thumb by id), `tests/dialog_fit.rs` (Export dialog 60 shorter), `docs/parity-checklist.md`

## Tests

`tests_slider_columns`, `temp_slider`, `dialog_fit`
