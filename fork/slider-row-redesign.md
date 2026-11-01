# Develop sliders on one row

## What it does

Develop sliders on one row: label left, a recessed groove with eleven ticks and a teardrop thumb, the value right in a fixed-width column (editing never moves the track), long labels shrink to fit, gradient tracks keep their colours, a divider in Noise Reduction.

In the Edit and Masking panels the label column is one fixed width, left-aligned (issue #21): it is the widest label of all Develop controls (all mask sliders in Masking) at the current font size and language, not of the open sections, so opening or closing a section moves no label or track. It keeps the old cap (41 % of the row, shrinking long labels). Other panels (Crop, dialogs) keep the shared right-aligned column.

## Branch

`claude/slider-row-redesign`

## Own files

`crates/ui-egui/src/widgets/tests_slider_columns.rs`, `crates/ui-egui/src/widgets/label_column.rs`, `crates/ui-egui/src/tests/fixed_slider_labels.rs`

## Hooks in official files

`widgets.rs` (`slider` fixed column, `LABEL_FONT`, `mod label_column`, `paint_track`, `paint_thumb`, `slider_columns`, `shared_column_width`, `sliderThumb:` id), `theme.rs` (`slider_row_h` 30), `panels/edit.rs` (noise reduction divider; `show` fixes the label column), `panels/masking.rs` (`show` fixes it, `OTHER_SLIDER_LABELS`), `tests.rs`, `tests/temp_slider.rs` (thumb by id), `tests/dialog_fit.rs` (Export dialog 60 shorter), `docs/parity-checklist.md`

## Tests

`tests_slider_columns`, `fixed_slider_labels` (Exposure's label and track rects equal with Detail open or closed; labels left-aligned; masks), `temp_slider`, `dialog_fit`
