# Slider strength audit

## What it does

Slider strength audit: a measurement harness and `docs/slider-strength.md` for every remaining Develop slider; Noise Reduction Contrast (`detail.nrContrast`) now works on every process (0 unchanged); Sharpening Detail works in process V3 with a stronger gain.

## Branch

`claude/slider-strength-audit`

## Own files

`crates/pipeline/src/tests_slider_strength.rs`, `docs/slider-strength.md`

## Hooks in official files

`pipeline/src/local.rs` (`nr_keep`), `pipeline/src/finish.rs` (`sharpen_mix`, `SHARPEN_GAIN_V3`), `gpu/src/wgsl/map.wgsl` (`nr_lum`), `gpu/src/wgsl/finish.wgsl`, `gpu/src/params.rs` (`SHARPEN_DETAIL`), `gpu/src/render.rs`, `pipeline/src/lib.rs` (`mod`), `docs/parity-checklist.md`, `docs/gaps.md`

## Tests

`tests_slider_strength`
