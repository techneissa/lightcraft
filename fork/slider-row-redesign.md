# Develop sliders on one row

## What it does

Develop sliders on one row: label left, a recessed groove with eleven ticks and a teardrop thumb, the value right in a fixed-width column (editing never moves the track), long labels shrink to fit, gradient tracks keep their colours, a divider in Noise Reduction.

In the Edit and Masking panels the label column is one fixed width, left-aligned (issues #21, #29): it is the widest of the common labels at the normal font size and current language (Edit: the Light, Color and Effects sliders without the HDR ones; Masking: the local adjustment sliders), not of the open sections or of the longest label, so opening or closing a section moves no label or track and short labels leave no wide gap. It keeps the old cap (41 % of the row). A label that does not fit uses a slightly smaller font (down to 11 pt); one that still does not fit wraps onto two lines at that size (down to 9.5 pt if a word needs it) and its row grows taller, the track and value keeping their place relative to the row's middle. Other panels (Crop, dialogs) keep the shared right-aligned column.

## Branch

`claude/slider-row-redesign`

## Own files

`crates/ui-egui/src/widgets/tests_slider_columns.rs`, `crates/ui-egui/src/widgets/label_column.rs`, `crates/ui-egui/src/tests/fixed_slider_labels.rs`

## Hooks in official files

`widgets.rs` (`slider` fixed column, `label_width`, `LABEL_FONT` (now `pub(crate)`), `mod label_column`, `paint_track`, `paint_thumb`, `slider_columns`, `shared_column_width`, `sliderThumb:` id), `theme.rs` (`slider_row_h` 30), `panels/edit.rs` (noise reduction divider; `show` fixes the label column from the common labels), `panels/masking.rs` (`show` fixes it from `LOCAL`), `tests.rs`, `tests/temp_slider.rs` (thumb by id), `tests/dialog_fit.rs` (Export dialog 60 shorter), `docs/parity-checklist.md`

## Tests

`tests_slider_columns`, `fixed_slider_labels` (Exposure's label and track rects equal with Detail open or closed; labels left-aligned; the column fits the common labels, not Color Noise Reduction; long labels shrink or wrap inside it and the track stays centred; masks), `temp_slider`, `dialog_fit`
