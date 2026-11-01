# Filmstrip badge bar at one height

List: the filmstrip's badge bar (stars, flag, edited mark) sits at one height in every cell, along the bottom of the cell, and the photo is fitted above it; `fix/41-filmstrip-badge-bar`.

## What it does

Each filmstrip cell draws its badge bar in the same place: the cell's full width, along its bottom edge. The photo is fitted and centred in the space above the bar, so photos of different shapes (16:9, 1:1, 4:5) show their bars at one height and the bar never covers a photo. Icons, order, selection ring and hover highlight are unchanged. With Show ratings, flags and edit badges off, the photo uses the whole picture area as before.

The Library grid needs no change: its badge bar already sits on the cell's bottom edge (square cells) or on the bottom edge of an equal-height row (justified rows), not on the photo's own edge.

## Branch

`fix/41-filmstrip-badge-bar`

## Own files

`crates/ui-egui/src/panels/detail/film_bar.rs` (the cell layout and its tests)

## Hooks in official files

`panels/detail.rs` (`mod film_bar;`), `panels/detail/filmstrip.rs` (the cell takes its photo and bar rectangles from `film_bar::layout`; `film_badges` paints into the bar it is given)

## Tests

`film_bar` (a 16:9, a 1:1 and a 4:5 photo: one bar rectangle, no overlap with the photo)
