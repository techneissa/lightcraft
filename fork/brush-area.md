# Brush area

List: a brushed Remove, Heal or Clone spot shows one outline of the area it covers instead of a circle for every point, and a Remove stroke that ends where it began covers its inside too, so drawing around an object removes it; `feat/70-remove-area`.

## What it does

- **One outline**: a brushed spot shows ONE thin, semi-transparent outline, the contour of the area it covers (the union of its discs), for the target and, when the spot is selected, for its source copy. No circle for each stored point, so the photo under the line stays visible. A dark 1 px halo under the white line shows it on white sky. The contour is worked out once per spot and kept in egui's memory under a cheap fingerprint (number of points, three of them, the size, the photo's shape on screen): a spot changes only by moving or by a new size, never by one point in the middle.
- **While painting**: the stroke is one translucent filled shape (one mesh) with one outline, not a filled circle per point. It is measured again once the stroke has four more points, not every frame.
- **Lasso** (the Remove panel's **Fill Closed Loops**, on by default, saved in `ui.json` as `removeLasso` and synced with the other tool options): a Remove or Content-Aware stroke that closes on itself covers the loop's inside as well as its ribbon. It is a lasso when all of these hold (`lightcraft_pipeline::spot_lasso`):
  - it ends within **2 brush radii** of where it began and is at least **6 radii** long;
  - the loop holds, beyond what the ribbon itself covers, at least **3 brush discs (3 π r²)** of area. This is measured on the grid, not from a signed area: a figure 8 (whose signed area cancels) fills both lobes, while a stroke out and back, a sliver, and a small circle around a blemish (inside under three discs, a loop of radius below about 2.7 radii) stay strokes, with today's source copy and its pin. Reading "ribbon's own area" as the area of one brush disc is a choice of this work: measured against the ribbon's whole area, a loop would have to be about 12 radii wide, which a brush of the usual size never draws around an object;
  - there is no explicit `source` (a `spot.add` that names a source keeps its stroke);
  - the loop does not cover more than 60 % of the photo: it is refused with Remove This Area's message ("the area covers most of the photo; Remove rebuilds smaller areas from what is around them") and nothing is added.
  While painting, a loop that qualifies already shows filled. Heal and Clone keep their stroke (they need a source). `spot.add` takes `lasso?: bool` (default true).
- **How a lasso is stored**: as a shaped spot (`Spot::area`, kind `lasso`, row "Remove 3 · Lasso"). The shape is the loop's inside, clipped to the photo (so its box and pin stay on the photo), measured on 840 cells across the loop whatever its size (a stored shape may have 1,024 at most), so a full-frame export is not soft. `size` is the brush radius and acts as the Expand: the spot reaches one brush radius past the loop and fades over Feather × that radius, the same footprint and feather as the stroke's ribbon (the Feather slider works on it live). It has no source and uses the brush's fill (Classic or LaMa), as every shaped spot does. The outline on the photo is the loop.
- **Make Stroke** (a button on a lasso's row in the Remove panel, `spot.makeStroke`, one undo step): the lasso becomes the plain stroke it came from, without its inside. Nothing is lost: the shape keeps the stroke's points and brush size in two new optional fields of `SpotArea` (`stroke`, `brush`; left out of the file when empty, and read as none from a file without them, so no catalog format bump). A moved lasso moves its stroke with it. A Classic Remove stroke picks its source again.
- **Open strokes**: unchanged. A stroke that is not a lasso is stored and rendered exactly as before; an old spot saves byte for byte as it did.
- **Hostile strokes**: points that are not numbers are dropped, far-off ones clamped, the stroke thinned to 2,048 points, segments cut to the grid, and the grid at most 640 cells a side (300 while painting, 840 for a stored lasso), so 100,000 points or NaN neither panic nor stall.

Issue #70 (Ellipse and Egg shapes are not done: a spot in a mask's shape and a lasso cover what they were for).

## Branch

`feat/70-remove-area`

## Own files

`crates/pipeline/src/spot_lasso.rs`, `crates/pipeline/src/tests_spot_lasso.rs`, `crates/engine/src/tests/brush_area.rs`, `crates/ui-egui/src/panels/detail/brush_outline.rs`, `crates/ui-egui/src/tests/brush_area.rs`

## Hooks in official files

- `crates/develop/src/spot_area.rs` (the `lasso` kind and its label; `SpotArea::stroke` and `brush`), `crates/pipeline/src/lib.rs` (`pub mod spot_lasso`).
- `crates/engine/src/cmd/masks.rs` (`spot.add` asks `spot_area::lasso`), `crates/engine/src/cmd/spot_area.rs` (`lasso`, `spot.makeStroke`), `crates/engine/src/spot_area.rs` (`centre_of` is `pub(crate)`), the `mod` line of `crates/engine/src/tests.rs`.
- `crates/ui-egui/src/panels/detail.rs` (`mod brush_outline`), `crates/ui-egui/src/panels/detail/retouch.rs` (a brushed spot and its source draw `brush_outline::outline`; painting draws `brush_outline::painting`; `spot.add` gets `lasso`; a refused lasso is a toast), `crates/ui-egui/src/panels/detail/object_remove.rs` (the halo), `crates/ui-egui/src/panels/right/remove_object.rs` (Fill Closed Loops), `crates/ui-egui/src/panels/right/spot_rows.rs` (Make Stroke), `crates/ui-egui/src/state.rs` (`remove_lasso`), `crates/ui-egui/src/settings_sync/rules.rs` (`removeLasso` syncs), the `mod` line of `crates/ui-egui/src/tests.rs`.
- `docs/parity-checklist.md`: LR-REM-SPOT-EDIT and LR-REM-DETECT changed in place.

## Tests

`fork/tests.d/brush-area.txt`: the contour, loop detection (arcs, out and back, slivers, small loops, a figure 8), the filled loop, clipping, the 60 % cap, the stored resolution, a lasso feathering like a stroke, a hostile stroke, and a render where the loop's inside changes with a lasso and not with an open stroke (pipeline); the lasso's stroke in the file (develop); `spot.add` storing a lasso, strokes that stay strokes (open, Heal, Clone, `lasso: false`, an explicit `source`, a small loop), the cap, a loop off the edge, Make Stroke and a moved lasso, a hostile stroke (engine); one outline however many points, one translucent shape and a halo while painting, a closed stroke making a lasso, Fill Closed Loops off, Make Stroke on the row, the outline following a size change (UI, headless).

## What's New entry

### Remove Draws One Outline and Fills Loops
- A brushed Remove, Heal or Clone spot shows one thin outline of the area it covers, instead of a ring for every point, so you can see the photo under it. While you paint, the stroke is one translucent shape.
- Draw a loop around something with Remove or Content-Aware and let go where you began: the whole inside is removed, not just the line you drew. Fill Closed Loops in the Remove panel turns this off, and a lasso's row has Make Stroke to turn it back into the line you drew.

## Parity rows

LR-REM-SPOT-EDIT and LR-REM-DETECT say so in `docs/parity-checklist.md`.
