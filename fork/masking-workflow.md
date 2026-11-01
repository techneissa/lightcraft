# Masking workflow

## What it does

Masking workflow (issue #26): under the selected mask one Add · Subtract · Intersect control sets the mode of the next part, and a button for each kind of part (Brush, Linear, Radial, Color, Luminance, Subject, Sky, Background, Object, Describe) adds it: no menus, no help lines. A click on a part's row selects it (a brush part starts painting into it again); the selected or hovered row has its own + − ∩, Invert and Delete buttons. Brush Add and Erase are named by what a stroke does to the mask, also in a Subtract part (a Subtract part starts on Erase); the cursor shows + or −, swapping while ⌥ is held. Esc steps back one level at a time (the brush, the part, the photo); Delete removes the selected part, else the mask. The first part of a mask is its base and always acts as Add; deleting it makes the next part the base. The Create New Mask buttons fill the side bar's width (at least 48 pt, at most 104 pt a tile). Overlay mode and Effect stay dropdowns: five long names and a list of the user's own presets do not fit as buttons.

## Branch

`fix/26-masking-rework`

## Own files

`crates/ui-egui/src/panels/mask_parts.rs`, `crates/ui-egui/src/tests/mask_parts.rs`

## Hooks in official files

`crates/engine/src/cmd/masks.rs` (`mask.select` `component`, `mask.brushStroke` `component`, `mask.component` rebases on delete, `Session::active_part`), `crates/engine/src/lib.rs` (`active_component`), `crates/engine/src/cmd/query.rs` and `crates/ui-egui/src/control.rs` (`activeComponent`), `crates/pipeline/src/masks.rs` and `crates/gpu/src/render.rs` (the first part is the base), `crates/ui-egui/src/panels/masking.rs` (the parts section, tile layout, icons for Object / Describe / Background), `crates/ui-egui/src/panels/detail/masks.rs` and `brush_preview.rs` and `brush_live/target.rs` (the part strokes go into, `+` / `−` cursor), `crates/ui-egui/src/menus/run.rs` (`tool.brush` calls `mask_parts::start_brush`, Esc leaves the part), `crates/ui-egui/src/shortcuts.rs` (Delete), `crates/ui-egui/src/icons.rs` (`ObjectSelect`, `Describe`, `Background`), `crates/ui-egui/src/state.rs` (`mask_op`), `panels.rs`, `tests.rs`, `docs/whats-new.md`, `docs/control-protocol.md`, `docs/parity-checklist.md`

## Tests

`mask_parts`, `a_first_part_is_the_base`, `deleting_the_first_part`, `a_selected_brush_part`, `the_selected_part_follows`, `tiles_fill`, `create_new_mask_tiles`
