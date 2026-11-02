# Mask menu, submask selection and submask names

List: a mask's right-click menu adds, subtracts and intersects every kind of submask and deletes empty or all masks; several submasks are selected and changed together; submasks are named "<Kind> Submask <n>" (issue #61). Branch `feat/61-mask-menu-submasks`.

## What it does

- **Mask menu** (`mask_menus::mask_menu`): Add to Mask, Subtract from Mask and Intersect Mask with, each a submenu of every kind of submask in the panel's groups (Subject, Sky, Background, Object, Describe… / Brush, Linear Gradient, Radial Gradient / Color Range, Luminance Range). A pick selects the mask and runs what the panel's button runs (`mask.addComponent`, `tool.brush`, the Object and Describe starts), so the new submask is selected and its tool starts. The Duplicate, Duplicate and Invert, Invert and Delete items carry the mask's name. Delete Empty Masks (`mask.deleteEmpty`) and Delete All Masks (`mask.deleteAll`, asks first).
- **Intersect with Brush** works: the engine already took a brush part in any mode, and painting adds to an Intersect brush part like an Add one, so the painted area is what stays. The panel's Brush button is no longer greyed out in Intersect mode.
- **Empty mask** (`mask_multi::is_empty`): no submask, or the submasks folded in order (the base sets it, Add joins, Intersect with nothing clears it, Subtract leaves it) select nothing: a brush with no stroke, an Object with no click and no selection, a colour range with no sample. An inverted mask, an inverted submask and a submask that waits for its AI selection count as selecting something.
- **Several submasks** (⌘-click toggles, ⇧-click selects the run from the selected one, Esc goes back to the last clicked one, then as before): the submask menu acts on all of them: Duplicate N Submasks (the copies go right after the last selected one, keep their order and modes, and become the selection), Duplicate and Invert, Invert, Mode ▸, Duplicate to New Mask, Delete N Submasks; the Delete key deletes them all. The panel remembers the set (`UiState::mask_ui`); the engine still has one selected submask, the last clicked, which the Brush paints into.
- **Commands**: `mask.component {id?, component | components: [..], action: invert|duplicate|delete|rename|op|toMask, op?, invert?}` takes a list (a single `component` works as before); each call is one undo step; duplicate answers the indexes of the copies as `components`. `toMask` makes a new mask of the chosen submasks (the first becomes its base) with the adjustments, refine amount and invert of the source, named like any new mask.
- **Names** (`mask_names::submask_name`): "<Kind> Submask <n>", n counted per kind in the mask ("Subject Submask 1", "Brush Submask 2"; Linear, Radial, Luminance and Color for the longer kinds, so a name fits its row); a Describe submask shows its words ("“red car” Submask"); a name the user gave is kept. The name is worked out when shown and never stored, so submasks saved before get it and no photo is written until one is renamed. The numbers count by position in the mask, so a name can shift after a delete; the saved format is unchanged. All rows of the list share one text size (down to 10.5 pt) and end with … only when that still does not fit (`mask_name::common_size`). The UI says Submask wherever it said Part; command ids stay.
- A click on the empty space under the list, like Esc, goes back to one submask. The set is trusted only while the photo, the engine's selected submask and the submask count are unchanged, so a delete, undo or another photo drops it; after undo or redo the engine forgets a selected submask that no longer exists. Mode on the current mode does nothing and makes no undo step. A gradient, colour or luminance range added from a menu starts its tool.

## Branch

`feat/61-mask-menu-submasks`

## Own files

`crates/engine/src/cmd/mask_multi.rs`, `crates/ui-egui/src/panels/mask_menus.rs`, `crates/ui-egui/src/tests/mask_menus.rs`, `fork/mask-menu-submasks.md`, `fork/tests.d/mask-menu-submasks.txt`

## Hooks in official files

- `crates/engine/src/cmd.rs`: `pub mod mask_multi;`, `mask_multi::specs()`, `mask_names` is `pub`. `crates/engine/src/cmd/masks.rs`: `mask.component` calls `mask_multi::component` (its body moved there); `masks_edit`, `optional_index` and `find` are `pub(super)`. `crates/engine/src/cmd/mask_names.rs`: `kind_of`, `submask_name`. `crates/engine/src/cmd/mask_parts.rs`: `follow` takes the list.
- `crates/ui-egui/src/panels/masking.rs`: the mask menu is `mask_menus::mask_menu`, the Delete All window is drawn under the heading, one toast says submask. `panels/mask_parts.rs`: rows use the submask name and the selection, `click_part`, Esc and Delete take the selection, the part menu moved to `mask_menus::part_menu`, Submask wording, Brush enabled in Intersect. `menus/run.rs` (`view_back_command`): `collapse_selection` first. `state.rs`: field `mask_ui`. `panels.rs`, `tests.rs`: `mod` lines.

## Tests

`cargo test -p lightcraft-engine --lib -- mask_multi mask_names the_selected_part` · `cargo test -p lightcraft-ui-egui --lib -- mask_menus mask_parts` (also in `fork/tests.d/mask-menu-submasks.txt`).

## What's New entry

### Combine Masks From The Right-Click Menu
- A mask's menu has Add to Mask, Subtract from Mask and Intersect Mask with, each listing every kind of submask, plus Delete Empty Masks and Delete All Masks. The Brush can now intersect.
- ⌘-click and ⇧-click select several submasks; the submask menu duplicates, inverts, changes the mode of and deletes them together, or copies them into a new mask.
- Submasks are named by kind and number ("Brush Submask 2"); the panel says Submask where it said Part.

## Parity rows

| MENU-CTX-MASK | Mask / submask menu | P0 | ✅ | `crates/ui-egui/src/panels/mask_menus.rs` (`mask_menu`, `part_menu`), `cmd:mask.component`, `cmd:mask.deleteEmpty` | mask rows: duplicate (and invert), invert and delete (named), show/hide, add to / subtract from / intersect with every kind of submask, move, rename, delete empty masks, delete all masks (asks first); submask rows (⌘-click and ⇧-click select several; the menu acts on all of them): invert, duplicate (and invert), duplicate to a new mask, mode (add / subtract / intersect), rename, delete (the last one deletes the mask) |
