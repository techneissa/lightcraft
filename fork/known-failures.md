# Known failures

### Known failures on this Mac (not caused by the fork)

- `accessibility::every_interactive_node_has_a_name_in_every_view_and_tool`: fails the same way on official `main`
  (59a5d00c), with and without `CRAFT_FONTS_DIR=…/craft-fonts`: `No glyph for "▸" (U+25B8) in Proportional`, from the
  "▸" in official strings such as the Reference-view message. Neither the bundled Inter nor craft-fonts has that glyph,
  so the fonts cannot fix it. Official strings are left alone (changing them would conflict on every update); when
  official fixes them, this note can go. Checked 2026-10-28. In the fork the test would also have failed on the top
  bar's unnamed click area; that one is fixed (`panels/topbar.rs`).
- `cargo xtask ci` stops at its `test` step because of it; run the rest by hand (`cargo xtask parity`, `layers`,
  `assets`).

- `tests::masking::a_brush_stroke_is_not_tinted_while_the_mask_overlay_is_hidden` (ui-egui): fails on `custom` before the
  masking rework (c3e32eea) with `74 of 74 points` tinted, `[253, 209, 181]` against `[255, 255, 255]`: the live
  adjustment of a +1 exposure mask shows while painting with the overlay hidden. Not touched by issue #26. Checked
  2026-11-01.

- `tests::masking::mask_adjustments_hide_overlay_without_disabling_the_mask` (ui-egui, official #519): fails since the
  masking rework (PR #38, c1a21808) with `exposure initially needs scrolling`. Its first step asserts the Exposure slider
  starts below the fold; the reworked Masking panel is shorter, so the slider is already visible. The behaviour it
  tests (a slider change hides the overlay, the mask stays enabled, O restores it) still holds; the fork test
  `a_mask_slider_hides_the_overlay_and_keeps_the_mask_enabled` (`tests/mask_parts.rs`) proves it. The official test is
  left as is. Checked 2026-11-02.

### Known failures on Fork CI only (Linux)

None. `tests::macos_colour` (`crates/engine`) runs on macOS only (`#[cfg(target_os = "macos")]` in `tests.rs`,
techneissa 2026-11-01): it failed on Linux and stopped `cargo test` before the later crates (ui-egui, apps) ran.
