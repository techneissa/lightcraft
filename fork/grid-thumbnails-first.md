# Performance: grid thumbnails before background work

List: On-screen grid thumbnails come before the background preview build, and two macOS Colour thumbnails decode at once. Issue #352, branch `perf/352-grid-thumbnails`.

## What it does

- **The build waits for the grid.** While an on-screen grid cell's thumbnail is on its way, the grid gives the background preview build the same hold the loupe gives it (#196, `prep::Foreground::signal`): the build pauses before its next photo and goes on once the visible thumbnails are in. The grid asks for a frame every 150 ms meanwhile, so the 250 ms hold does not lapse between frames. Before, a folder opened while the build ran filled its screen about twice as slowly.
- **Two small Mac decodes at once.** At the memory gate a macOS Colour decode weighs Core Image's peak footprint (1 GB), more than the gate's limit (a quarter of the memory budget), so each ran alone. One no larger than a preview (2560 px: grid thumbnails, which decode at the preview size when they also make the loupe's preview, #221, and the loupe's preview) now weighs at most half the gate, so two run side by side, as the decoder's own slots allow (`lightcraft_macraw` runs at most two that small). A full-size one keeps its whole weight: it waits for an empty gate and nothing starts beside it, not even a thumbnail.

## Branch

`perf/352-grid-thumbnails`

## Own files

`fork/grid-thumbnails-first.md`, `fork/tests.d/grid-thumbnails-first.txt`

## Hooks in official files

`crates/ui-egui/src/panels/grid.rs` (`hold_background_for`), `crates/ui-egui/src/panels/detail.rs` (`signal_foreground` shared with the grid), `crates/engine/src/files/fs.rs` (`load_weight`, `SMALL_MACOS_EDGE`), `crates/engine/src/media/source.rs` (`SourceLevel::max_edge` is `const`)

## Tests

`fork/tests.d/grid-thumbnails-first.txt`: the grid holds background preparation while its thumbnails are on their way and not after; the gate admits two macOS Colour thumbnails but never two full-size renders, nor a thumbnail beside a full-size one.

## What's New entry

### Grid Thumbnails Come First

Opening a folder while LightCraft prepares previews in the background no longer slows its thumbnails down: the background work waits until the thumbnails on screen are in. macOS Colour thumbnails are made two at a time.
