# Masking submask fixes (Edge sliders, Invert Mask)

List: each submask's Edge, Smoothness and Refine slider changes only its own submask, and Invert Mask turns every submask around, not just the first (issues #68, #72). Branch `fix/68-masking-submasks`.

## What it does

- **Sliders per submask** (#68): every submask row of the Masking panel is its own id scope (`widgets::in_repeated_row`), so two submasks' Edge sliders (and Smoothness, Refine) are separate widgets. Before, they shared one widget id: they moved together and only the bottom one could be grabbed. The control channel names each slider `slider:<id>:<row>` (`slider:edge:0`, `sliderValue:edge:1`) besides the old `slider:<id>`, which stays.
- **Invert Mask** (#72): `mask.invert` and `mask.duplicate` with `invert` flip `Mask.invert` and rewrite the later submasks in the same undo step (`cmd::mask_invert::invert`), so the whole mask selects the opposite:

  | Submask before | after |
  | --- | --- |
  | Add C | Subtract C |
  | Subtract C | Add C |
  | Intersect C | Add C, the submask's own Invert flipped |

  Hard edges give the exact opposite. Where soft edges overlap the result differs slightly (Add is `max`, Subtract multiplies). Inverting twice selects the same area; an Intersect comes back as Subtract with its Invert on. A saved mask with Invert already on renders as before: `evaluate_one` is unchanged, only a new click rewrites submasks.

## Branch

`fix/68-masking-submasks`

## Own files

`crates/engine/src/cmd/mask_invert.rs`, `crates/ui-egui/src/tests/mask_edge_rows.rs`, `fork/masking-submask-fixes.md`, `fork/tests.d/masking-submask-fixes.txt`

## Hooks in official files

- `crates/engine/src/cmd.rs`: `mod mask_invert;`. `crates/engine/src/cmd/masks.rs`: `mask.invert` and `mask.duplicate` call `mask_invert::invert`.
- `crates/ui-egui/src/widgets.rs`: `in_repeated_row`, and `slider` also registers `slider:<id>:<row>` and `sliderValue:<id>:<row>` inside it. `crates/ui-egui/src/panels/mask_parts.rs`: `part_rows` calls `range_controls` through `in_repeated_row`. `crates/ui-egui/src/tests.rs`: `mod mask_edge_rows;`.

## Tests

`cargo test -p lightcraft-engine --lib -- mask_invert` · `cargo test -p lightcraft-ui-egui --lib -- mask_edge_rows` (also in `fork/tests.d/masking-submask-fixes.txt`).
