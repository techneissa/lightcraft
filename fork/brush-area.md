# Brush area

List: a brushed Remove, Heal or Clone spot shows one outline of the area it covers instead of a circle for every point, and a Remove stroke that ends where it began covers its inside too, so drawing around an object removes it; `feat/70-remove-area`.

## What it does

- **One outline**: a brushed spot shows ONE thin, semi-transparent outline, the contour of the area it covers (the union of its discs), for the target and, when the spot is selected, for its source copy. No circle for each stored point, so the photo under the line stays visible. The contour is worked out once per spot and kept in egui's memory (it changes only when the spot's points, size or the photo's shape on screen do).
- **While painting**: the stroke is one translucent filled shape (one mesh) with one outline, not a filled circle per point.
- **Lasso**: a Remove or Content-Aware stroke that ends within two brush radii of where it started and encloses more than its own width (`lightcraft_pipeline::spot_lasso::is_loop`) covers the loop's inside as well as its ribbon. While painting, the loop already shows filled. Heal and Clone keep their stroke (they need a source). `spot.add` takes `lasso?: bool` (default true) to turn this off.
- **How a lasso is stored**: as a shaped spot (`Spot::area`, kind `lasso`, row "Remove 3 · Lasso"): the grid holds the loop's inside and the ribbon, `size` is the Expand (the brush radius, at most 0.5 %), so it covers what the stroke did. It has no source and fills with the fill the brush has (Classic or LaMa), as every shaped spot does. No new saved field and no catalog format bump: `area` was already there, and an old spot saves byte for byte as before.
- **Open strokes**: unchanged. A stroke that does not close is stored and rendered exactly as before.
- **Hostile strokes**: points that are not numbers are dropped, far-off ones clamped, the stroke thinned to 2,048 points and the grid at most 640 cells a side (300 while painting), so 100,000 points or NaN neither panic nor stall.

Issue #70 (Ellipse and Egg shapes are not done: a spot in a mask's shape and a lasso cover what they were for).

## Branch

`feat/70-remove-area`

## Own files

`crates/pipeline/src/spot_lasso.rs`, `crates/pipeline/src/tests_spot_lasso.rs`, `crates/engine/src/tests/brush_area.rs`, `crates/ui-egui/src/panels/detail/brush_outline.rs`, `crates/ui-egui/src/tests/brush_area.rs`

## Hooks in official files

- `crates/develop/src/spot_area.rs` (the `lasso` kind and its label), `crates/pipeline/src/lib.rs` (`pub mod spot_lasso`).
- `crates/engine/src/cmd/masks.rs` (`spot.add` asks `spot_area::lasso`), `crates/engine/src/cmd/spot_area.rs` (`lasso`), `crates/engine/src/spot_area.rs` (`centre_of` is `pub(crate)`), the `mod` line of `crates/engine/src/tests.rs`.
- `crates/ui-egui/src/panels/detail.rs` (`mod brush_outline`), `crates/ui-egui/src/panels/detail/retouch.rs` (a brushed spot and its source draw `brush_outline::outline`; painting draws `brush_outline::painting`), the `mod` line of `crates/ui-egui/src/tests.rs`.
- `docs/parity-checklist.md`: LR-REM-SPOT-EDIT and LR-REM-DETECT changed in place.

## Tests

`fork/tests.d/brush-area.txt`: the contour, loop detection, the filled loop, a hostile stroke, and a render where the loop's inside changes with a lasso and not with an open stroke (pipeline); `spot.add` storing a lasso, strokes that stay strokes, a hostile stroke (engine); one outline however many points, one translucent shape while painting, a closed stroke making a lasso (UI, headless).

## What's New entry

### Remove Draws One Outline and Fills Loops
- A brushed Remove, Heal or Clone spot shows one thin outline of the area it covers, instead of a ring for every point, so you can see the photo under it. While you paint, the stroke is one translucent shape.
- Draw a loop around something with Remove or Content-Aware and let go where you began: the whole inside is removed, not just the line you drew.

## Parity rows

LR-REM-SPOT-EDIT and LR-REM-DETECT say so in `docs/parity-checklist.md`.
