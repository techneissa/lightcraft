# Masking workflow

## What it does

Masking workflow (issue #26): under the selected mask one Add · Subtract · Intersect control sets the mode of the next part (it follows the selected part's mode), and a button for each kind of part (Brush, Linear, Radial, Color, Luminance, Subject, Sky, Background, Object, Describe) adds it: no menus, no help lines. Each part is a row that is a button: a click selects it and brings its tool back (a brush part the Brush painting into it, a gradient its handles on the photo, blue, ready to drag, a colour range the picker, an Object its clicks, the others their own controls). At the right of every row: a mode dropdown showing the mode's icon (+, −, ∩; none on the first part), an Invert toggle and Delete. The whole mask's Invert is an icon on its row in the Masks list. Brush Add and Erase are named by what a stroke does to the mask, also in a Subtract part and in a part with Invert on (a Subtract part starts on Erase); the cursor shows + or −, swapping while ⌥ is held. Esc steps back one level at a time (the tool, the part, the photo); Delete removes the selected part, else the mask. The first part of a mask is its base and always acts as Add; deleting it makes the next part the base. The Create New Mask buttons fill the side bar's width in 5, 4 or 2 columns (never a tile alone on the last row; labels shrink down to 8 pt before a column is dropped). Overlay mode and Effect stay dropdowns: five long names and a list of the user's own presets do not fit as buttons.

## Branch

`fix/26-masking-rework`

## Own files

`crates/engine/src/cmd/mask_parts.rs`, `crates/ui-egui/src/panels/mask_parts.rs`, `crates/ui-egui/src/panels/mask_tiles.rs`, `crates/ui-egui/src/tests/mask_parts.rs`

## Hooks in official files

Where an official update should look (function, what changed, why):

Engine
- `crates/engine/src/cmd.rs`: `mod mask_parts;`.
- `crates/engine/src/lib.rs`: field `Session::active_component` (the selected part), set to `None` where the photo changes.
- `crates/engine/src/cmd/masks.rs`:
  - `masks_edit`: forgets the selected part when the selected mask changes.
  - `mask.brushStroke`: the part to paint into comes from `mask_parts::brush_target` (was: the last brush part), painting selects it; the doc string names `component`.
  - `mask.select`: the body is `mask_parts::select` (takes `component`); doc string.
  - `mask.component`: `op` goes through `mode_for` (the first part stays Add), `delete` through `rebase_after_delete`, the selected part follows through `follow`.
- `crates/engine/src/cmd/query.rs` (`engine.state`) and `crates/ui-egui/src/control.rs` (`ui.inspect`): `activeComponent`.
- `crates/pipeline/src/masks.rs` (`evaluate_one`) and `crates/gpu/src/render.rs` (mask ops): the first part is the base whatever its mode (a first Intersect emptied the mask). Both must change together.

UI
- `crates/ui-egui/src/icons.rs`: `Icon::ObjectSelect`, `Describe`, `Background` and their paint arms.
- `crates/ui-egui/src/state.rs`: `UiState::mask_op`, and the doc of `brush_erase` (now by effect on the mask).
- `crates/ui-egui/src/menus/run.rs`: `tool.brush` calls `mask_parts::start_brush`; `view_back_command` has a branch `mask_parts::leave_part` (Esc leaves the part after the tool).
- `crates/ui-egui/src/shortcuts.rs` (`delete_key`): Delete in Masking calls `mask_parts::delete_selected`.
- `crates/ui-egui/src/brush_live/target.rs`: `settings` is now a call of `settings_in`, which takes the part and turns a part with Invert on around.
- `crates/ui-egui/src/panels/detail/brush_preview.rs` (`request`): the part and whether a stroke goes the other way (`painting_into`).
- `crates/ui-egui/src/panels/detail/masks.rs`: `mask_overlay` (marks the selected part), `component_outline` (`at` carries the selected flag; the selected part's handles are blue), `grip_gestures` (a drag takes the selected part first), `brush_tool` (the + / − cursor; the stroke's `erase` from `stroke_erase`; `component` sent).
- `crates/ui-egui/src/panels/masking.rs`:
  - `kind_label`, `menu_label`, `range_controls`: `pub(super)`; `kind_label` has the new icons; `range_controls` lost the "harder border · softer border" help line.
  - `show_mask_panel`: tiles laid out by `mask_tiles::layout` (grid gap and label room from `mask_tiles`), new tile icons, the empty-list help line removed, the selected mask's row has `mask_invert_button` (and a shorter name), `add_controls` replaces the `+` / `−` pair, `part_rows` replaces the parts loop and the Add / Subtract / Intersect / Invert / trash row, the brush bar shows while a brush part is selected too.
  - `brush_settings`: a click on Add or Erase puts the Brush on.
  - `tile_layout`, `component_row_menu`, `component_menu`: left as they were, now unused (`#[allow(dead_code)]`); delete them when official drops them.
- `crates/ui-egui/src/panels.rs`, `crates/ui-egui/src/tests.rs`: `mod` lines. `crates/ui-egui/src/tests/masking.rs`: `subtract_brush_removes_sky_coverage_and_add_restores_it` uses the new controls and `luminance_range_controls_and_map` runs in a taller window (the panel is taller now). Official `mask_adjustments_hide_overlay_without_disabling_the_mask` fails on its "needs scrolling" precondition (the panel is shorter now): a known failure, covered by the fork test `a_mask_slider_hides_the_overlay_and_keeps_the_mask_enabled`.
- `docs/whats-new.md`, `docs/control-protocol.md`, `docs/parity-checklist.md`.

## Tests

`mask_parts`, `a_first_part_is_the_base`, `deleting_the_first_part`, `a_selected_brush_part`, `the_selected_part_follows`, `a_mask_slider_hides_the_overlay`, `mask_tiles`, `create_new_mask_tiles`, `brush_live::target`
