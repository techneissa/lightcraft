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

### Accepted failures that Fork CI skips

`fork/known-failures.txt` is the list Fork CI skips (one test path per line, `#` for a comment); it is read by
`fork/skip-known-failures.sh`, cargo's test runner on Linux (`.cargo/config.toml`), which passes each line to the test
binaries as `--skip`. A line skips every test whose path contains it, so write the full path. A red Fork CI run
therefore means a failure nobody accepted. Add a line here and to the `.txt` only when techneissa accepts a failure;
delete both when the test is fixed. A test file is never edited to hide a failure.

None at the moment (`fork/known-failures.txt` holds no test).

### Known failures on Fork CI only (Linux)

None other. `tests::macos_colour` (`crates/engine`) runs on macOS only (`#[cfg(target_os = "macos")]` in `tests.rs`,
techneissa 2026-11-01): it failed on Linux and stopped `cargo test` before the later crates (ui-egui, apps) ran.
