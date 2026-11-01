# A7 V colour fallback

## What it does

A7 V colour fallback: dull photos no longer fall back to raw sensor colour (the partial camera look starts from the camera profile or a spectral prior, the mean of 51 bodies, never the identity; `calibrate` fits white-keeping matrices).

## Branch

`fix/a7v-colour-fallback`

## Own files

tests in `crates/engine/src/camera_preview/tests.rs`

## Hooks in official files

engine `camera_preview/colour.rs` (`spectral_prior`), `pairs.rs` (`fit_matrix_neutral`), `look_fit.rs`, `proxy.rs`, `camera_preview.rs` (`LOOK_VERSION` 13), `media/source.rs` (`RENDER_CACHE_VERSION` 71); `docs/camera-preview-colour.md`. Next to the Camera Matching hooks in `colour.rs` / `proxy.rs`: both sides kept

## Tests

`camera_preview`
