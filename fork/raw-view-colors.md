# Raw view colours

## What it does

No camera-JPEG flash in the loupe while a raw renders; lensfun distortion zoomed to fill the frame (no empty corners).

## Branch

`fix/raw-view-colors`

## Own files

`crates/ui-egui/src/headless/tests/loupe_stand_in.rs`, tests in `crates/lensdb/src/tests.rs`

## Hooks in official files

`crates/ui-egui/src/panels/detail.rs` (the loupe keeps the previous photo up), `crates/engine/src/media/session.rs`, `crates/engine/src/lens_profiles.rs`, `crates/lensdb/src/lib.rs` (fill-frame scale)

## Tests

`loupe_stand_in`, `the_loupe_says`, `lightcraft-lensdb`, `quick_jobs`, `lens_profiles`
