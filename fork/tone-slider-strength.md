# Process V3: Contrast, Whites and Blacks

## What it does

Process V3: Contrast, Whites and Blacks clearly visible on camera-look raws and JPEG / PNG (one slider model on every tone path, natural strength, not a Lightroom match); V1 / V2 edits keep their look until Update to Current Process.

## Branch

`claude/tone-slider-strength`

## Own files

`crates/pipeline/src/tests_tone_sliders.rs`

## Hooks in official files

`pipeline/src/tone.rs` (`SliderModel`, `Sliders`, `toe`, `soft_clip`, `*_with` constructors), `pipeline/src/finish.rs` (`base_tone`), `pipeline/src/local.rs` (one match arm), `pipeline/src/lib.rs` (`mod`), `develop/src/settings.rs` (`Process::V3`, `LATEST`), `docs/process-versions.md`, `docs/whats-new.md`, `docs/parity-checklist.md`, `docs/gaps.md`

## Tests

`tests_tone_sliders`, `tests_tone`
