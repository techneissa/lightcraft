# Process V3: Sharpening Radius and Vignette

## What it does

Sharpening Radius (`detail.sharpenRadius`) works in process V3 (its own pixel-scale Gaussian band, 1.5 x Radius original pixels, preview matches export; Amount / Detail / Masking retuned); the post-crop Vignette has a smooth, natural response in V3 (-100 is about -2.8 stops, not black); V1 / V2 edits keep their look. The ⌥-drag Masking preview shows V3's own band and mask range (the weight the sharpening uses). Measured 2026-11-01: on a real ISO 2500 raw (A7S III) V3 Amount 60 adds about the flat-area noise V2 Amount 40 did (x1.30 vs x1.31), V3 Amount 40 x1.20; GPU and CPU differ by at most 1 LSB (mean 0.0002) on an Apple M4 Pro. Numbers and pictures: `docs/slider-strength.md`.

## Branch

`claude/detail-followups`

## Own files

`crates/pipeline/src/tests_slider_strength.rs` (`sharpening_radius_sets_the_band_scale`, `sharpening_radius_is_still_unread_before_v3`, `vignette_response_is_smooth_and_natural`), `crates/gpu/tests/it/equivalence.rs` (`process_v3_sharpening_and_vignette_match`), `crates/pipeline/src/tests_sharpen_mask.rs` (`the_sharpen_mask_follows_the_process_v3_band_and_range`)

## Hooks in official files

`pipeline/src/local.rs` (`sharpen_sigma`, `PlaneSigmas::sharpen`, `Planes::sharpen`), `pipeline/src/finish.rs` (`vignette_dark_factor`, `sharpen_gain`, `sharpen_mix`, the V3 sharpening branch, `sharpen_mask_weight(…, v3)` shared with the ⌥-drag preview), `pipeline/src/visualize.rs` (`sharpen_plane` reads the V3 band), `gpu/src/render.rs` (the same call), `pipeline/src/lib.rs` (`Prepared::sharpen_blur`; the mask preview call takes the process), `gpu/src/render.rs` (second plane in the `tex` buffer), `gpu/src/params.rs` (`HAS_SHARP`, `SHARP_OFF`), `gpu/src/wgsl/finish.wgsl`, `docs/slider-strength.md`, `docs/process-versions.md`, `docs/whats-new.md`, `docs/parity-checklist.md`

## Tests

`tests_slider_strength`, `tests_sharpen_mask` in `lightcraft-pipeline`; with a GPU `cargo test -p lightcraft-gpu --test it -- process_v3_sharpening` (skips without)
