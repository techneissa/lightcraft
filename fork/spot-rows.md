# Spot rows

List: each Remove, Content-Aware, Heal or Clone spot has a row in the Remove panel to select, hide, delete and set when its circle shows, and Tool Overlay Never covers nothing; `feat/69-spot-rows`.

## What it does

- **Rows**: under Delete All Spots and above Distraction Removal, one row per spot, named by its tool and its number among that tool's spots ("Heal 2", "Content-Aware 1"). The row is laid out like a submask row in the Masks panel. The list scrolls inside a height of seven rows, so Find Dust or Find People adding dozens of spots keeps the panel usable. A spot picked on the photo lights its row and scrolls it into view.
- **Select**: a click on the row selects its spot (`spot.select`). A spot whose circle is Never can't be grabbed on the photo; its row selects it.
- **Hide or show**: the eye (`spot.visible`) turns the spot's effect off. A hidden spot has no effect in the preview, in an export or on the GPU, and stays hidden when the photo is closed and reopened (`Spot::hidden`, saved with the photo's settings and left out of the file while the spot is shown). The renderers never see it: `DevelopSettings::effective()` drops hidden spots, as it already resets the sections whose eye is off. Its circle still shows as its own setting says.
- **Circle**: the menu at the left of the eye sets when this spot's circle shows: Use Global, Auto, Always, Selected or Never (`spot.circle`; `Spot::circle`, left out of the file for Use Global). It beats the Tool Overlay for this spot only and changes only the circle, not the effect. Auto still hides the circle while a new spot is painted.
- **Delete**: the trash can (`spot.delete` with the row's index).
- Hide, show, the circle setting and delete are each one undo step. Setting a value a spot already has changes nothing and adds no step.
- **Tool Overlay Never (#69)**: no brush rings follow the pointer and no white fill is drawn while painting; a crosshair pointer shows where the brush is. A click or drag still adds a spot. Auto, Always and Selected are as they were.
- A hidden spot written to a Lightroom sidecar: LightCraft does not write Lightroom's `RetouchAreas`; the spot is part of the `lc:settings` JSON, which keeps `hidden` and `circle`.

Issues #69 and #71.

## Branch

`feat/69-spot-rows`

## Own files

`crates/develop/src/spot_circle.rs`, `crates/engine/src/cmd/spot_rows.rs`, `crates/engine/src/tests/spot_rows.rs`, `crates/pipeline/src/tests_spot_hidden.rs`, `crates/gpu/tests/it/hidden_spot.rs`, `crates/ui-egui/src/panels/right/spot_rows.rs`, `crates/ui-egui/src/tests/spot_rows.rs`

## Hooks in official files

`crates/develop/src/settings.rs` (`Spot::hidden`, `Spot::circle` at the end), `crates/develop/src/lib.rs` (`effective()` drops hidden spots; `mod spot_circle`), `crates/engine/src/cmd.rs` (`mod spot_rows` and its `specs()`), `crates/engine/src/cmd/masks.rs` (`spot_index` and `spots_edit` are `pub(super)`; the `spot.add` literal takes `..Default::default()`), `crates/engine/src/cmd/find_removals.rs` (the same), `crates/ui-egui/src/panels/right/crop.rs` (one `spot_rows::section` call), `crates/ui-egui/src/panels/right.rs` (`mod spot_rows`), `crates/ui-egui/src/panels/detail/retouch.rs` (each spot asks its own circle setting; Never draws no ring or fill), the `mod` lines of `crates/engine/src/tests.rs`, `crates/pipeline/src/lib.rs`, `crates/gpu/tests/it/main.rs` and `crates/ui-egui/src/tests.rs`.

## Tests

`cargo test -p lightcraft-develop --lib spot_circle`, `cargo test -p lightcraft-engine --lib spot_rows`, `cargo test -p lightcraft-pipeline --lib tests_spot_hidden`, `cargo test -p lightcraft-gpu --test it hidden_spot` (CPU and GPU agree on a hidden spot, and both equal the render without it), `cargo test -p lightcraft-ui-egui --lib -- spot_rows tool_overlay` (rows, select, eye, trash, circle menu, scrolling list, Never covering nothing).

## What's New entry

### Spot Rows in Remove
- Every Remove, Content-Aware, Heal and Clone spot has a row in the Remove panel: click it to select the spot, use the eye to hide or show its effect, the trash can to delete it, and the circle menu to set when its circle shows on the photo (Use Global, Auto, Always, Selected or Never).
- With Tool Overlay at Never, the brush rings and the white painting fill are gone too; a small crosshair shows where the brush is.

## Parity rows

| LR-REM-SPOT-ROWS | A row for each spot | P1 | ✅ | `cmd:spot.select`, `cmd:spot.visible`, `cmd:spot.circle`, `cmd:spot.delete`, `crates/ui-egui/src/panels/right/spot_rows.rs` | Remove panel ▸ one row per spot ("Heal 2"): click selects, the eye hides or shows the effect (preview, export and GPU; saved with the photo), the circle menu overrides Tool Overlay for that spot (Use Global, Auto, Always, Selected, Never), the trash can deletes; each is one undo step; the list scrolls inside seven rows (#71). Tool Overlay Never also hides the brush rings and the painting fill (#69) |
