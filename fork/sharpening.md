# Sharpening (the fork's own)

List: Sharpening Radius and Detail are the fork's own implementation (process V3 and V4), on purpose: its sliders change the picture more than upstream's #757 version, which was not taken (techneissa, 2026-11-05). Branches `claude/detail-followups`, `claude/slider-strength-audit`.

## What it does

Sharpening Radius (`detail.sharpenRadius`) and Detail (`detail.sharpenDetail`) work in process V3, and V4 builds on them. Radius sets a pixel-scale Gaussian band of `1.5 × Radius` original pixels, scaled to the render so a preview and an export sharpen the same edges (`local::sharpen_sigma(radius, detail_scale)`, `local::plane_sigmas`). The gain is `finish::sharpen_gain` (`SHARPEN_GAIN_V3` = 2.4 up to Radius 1, then `Radius^-0.7`). Detail and Masking ranges are fitted to that band (`finish::sharpen_mix`, `finish::sharpen_mask_weight(…, v3)`, shared with the ⌥-drag Masking preview). V1 and V2 keep their old look and ignore Radius. techneissa prefers this version because its sliders have a stronger effect on the picture.

Upstream's different version (#757: `finish::sharpen_term`, `sharpen_limit`, `pixel_sharpen_mask_weight`, `local::pixel_sharpen`, `sharpen_sigma_raw`) was **not taken** (decided 2026-11-05). It must not replace this one. The parts of #757 that do not touch the sharpening maths are kept: `GAUSS_SIGMA_MAX` and the blur in `crates/raster/src/blur.rs`, `crates/gpu/src/wgsl/gauss.wgsl`, and `crates/pipeline/src/tests_sharpen.rs`, which is upstream's file adapted to the fork's functions.

## Branch

`claude/detail-followups` (Radius band, V3 gain, Masking preview), `claude/slider-strength-audit` (Detail in V3). Both are merged into `custom`.

## Own files

`crates/pipeline/src/tests_slider_strength.rs` (`sharpening_radius_sets_the_band_scale`, `sharpening_radius_is_still_unread_before_v3`), `crates/pipeline/src/tests_sharpen_mask.rs`, `crates/gpu/tests/it/equivalence.rs` (`process_v3_sharpening_and_vignette_match`), `docs/slider-strength.md`

## Hooks in official files

- `crates/pipeline/src/finish.rs`: `SHARPEN_GAIN_V3`, `sharpen_gain`, `sharpen_mix`, `sharpen_mask_weight(masking, det, v3)`, the V3 sharpening branch.
- `crates/pipeline/src/local.rs`: `sharpen_sigma(radius, detail_scale)`, `plane_sigmas`, `PlaneSigmas::sharpen`, `Planes::sharpen`.
- `crates/pipeline/src/visualize.rs` (the Masking preview reads the V3 band), `crates/pipeline/src/lib.rs` (`Prepared::sharpen_blur`).
- GPU: `crates/gpu/src/params.rs` (`HAS_SHARP`, `SHARP_OFF`, `SHARPEN_DETAIL`), `crates/gpu/src/wgsl/finish.wgsl`, `crates/gpu/src/render.rs` (the band is a second plane in the `tex` buffer).
- `docs/process-versions.md` (the V3 and V4 rows), `docs/slider-strength.md`.

## Tests

`tests_sharpen`, `tests_slider_strength`, `tests_sharpen_mask` in `lightcraft-pipeline`; with a GPU `cargo test -p lightcraft-gpu --test it -- process_v3_sharpening` (skips without).

## When upstream merges

When upstream changes sharpening again (any of the files above, or `sharpen_term` and the others named under What it does):

1. Keep the fork's code in every file of the hooks list. Drop upstream's sharpening change in them.
2. Take upstream's changes that are not about the sharpening maths (the blur, `gauss.wgsl`, tests that still pass against the fork's functions).
3. Adapt an upstream test to the fork's functions, or leave it out if it only checks upstream's numbers. Never change the fork's gain, band scale or ranges to make an upstream test pass.
4. Tell techneissa in plain words that upstream changed sharpening again and the fork kept its own, and name what upstream changed so they can decide whether to want any of it.
