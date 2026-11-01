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

### Known failures on Fork CI only (Linux; not fixed, Mac only)

- `tests::macos_colour::a_macos_colour_photo_never_shows_the_camera_jpeg_while_it_renders` (`crates/engine`): the
  macOS Colour feature needs macOS's raw decoder, so the test fails on Linux (Fork CI run 41155652051, 2026-10-31).
