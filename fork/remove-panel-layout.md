# Remove panel layout

List: the Remove panel's tools and Find buttons are rows of icon tiles that fill the side bar, with a short spot count; `fix/53-remove-panel-layout`.

## What it does

- **Tools**: Remove · Content-Aware · Heal · Clone are four equal tiles across the whole side bar (12 pt margins, 4 pt gaps), each with its own icon above the label, a hover sentence, and a clear selected state (lighter fill, accent outline). Labels are 10.5 pt, never under 9 pt; "Content-Aware" wraps to "Content" / "Aware". If four tiles ever stop fitting a row at 9 pt (a long translation), the tools go to two rows of two.
- **Fill**: the Classic / LaMa dropdown stays on the heading row and shows only while Remove or Content-Aware is chosen.
- **Brush settings** (Size, Feather, Opacity) sit right under the tools.
- **Find**: one row of three tiles, Dust · People · Wires, under a small "Find" caption; the found candidates' Remove / Remove All / Clear show under it only while there are candidates.
- **Spot count**: "6 Spots" / "1 Spot", small and quiet, at the right of the "Find" caption.
- No help line under the tools; "Delete all spots" is now "Delete All Spots".
- New icons (original vectors in `icons.rs`): a chunky rounded cross (Heal), two overlapping circles (Clone), a sparkle (Content-Aware).

Issue #53.

## Branch

`fix/53-remove-panel-layout`

## Own files

`crates/ui-egui/src/panels/right/remove_compact.rs`, `crates/ui-egui/src/tests/remove_compact.rs`

## Hooks in official files

`panels/right.rs` (`mod remove_compact`), `panels/right/crop.rs` (`remove` draws the tool tiles and calls the Find section; the count and help line are gone), `panels/right/remove_finds.rs` (the Find row is the tile row; Find Dust moves here), `panels/right/remove_fill.rs` (the picker shows only for Remove and Content-Aware), `icons.rs` (`HealCross`, `CloneStamp`, `SparkleFill`), `assets/ATTRIBUTION.md` (a note on `icons.rs`), the key "Delete all spots" renamed in `crates/ui-egui/locales/*.json`, and the `mod` line of `crates/ui-egui/src/tests.rs`.

## Tests

`cargo test -p lightcraft-ui-egui --lib -- remove_compact remove_fill find_removals` (tiles in one row of equal size at the narrowest and a wide side bar, Heal hides the Fill picker).
