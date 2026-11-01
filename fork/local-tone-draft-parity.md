# Process V3: Shadows and Highlights

## What it does

Process V3 Shadows / Highlights: Shadows +100 reaches the lower midtones, Highlights the upper ones, positive Highlights roll off before white; the slider preview's Highlights / Shadows base radius matches the export at every quality (all processes).

## Branch

`claude/local-tone-draft-parity`

## Own files

—

## Hooks in official files

`pipeline/src/finish.rs` (`shadows_highlights_delta`, `FinishParams::tone_v3`), `pipeline/src/local.rs` (`plane_sigmas` without the Draft cap), `gpu/src/params.rs` (`TONE_V3`), `gpu/src/wgsl/finish.wgsl` (V3 weights and shoulder), `develop/src/settings.rs` (V3 doc line), `pipeline/src/tests_tone.rs`, `docs/process-versions.md`, `docs/parity-checklist.md`, `docs/gaps.md`

## Tests

`tests_tone`
