# Export Animated GIF

## What it does

Export Animated GIF (File menu), quality above ffmpeg.

## Branch

`feat/animated-gif-export`

## Own files

`crates/codecs/src/animated_gif.rs` and `animated_gif/quantize.rs`, `crates/engine/src/animated_gif.rs`, `crates/ui-egui/src/dialogs/animated_gif.rs`

## Hooks in official files

engine `lib.rs` (module, session field), `cmd/export.rs` (two commands), `slideshow_export.rs` (helpers made `pub(crate)`); UI dialog, menu and frame-loop hooks; `Cargo.toml` (`gif`, `color_quant`)

## Tests

`animated_gif` in codecs, engine and ui-egui
