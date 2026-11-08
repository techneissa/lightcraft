# Slider strength (the fork's own)

List: The fork rewrote the maths of several Develop sliders on purpose (process V3, built on by V4): Contrast, Whites, Blacks, Highlights, Shadows, Sharpening Radius / Detail / Masking, Vignette and Noise Reduction Contrast. techneissa found the stock sliders had little or no effect on the images; these are stronger on purpose and are kept over upstream's versions (techneissa, 2026-11-05).

## What it does

**Reason.** The stock sliders had little or no effect on the images. The fork's versions are stronger on purpose, and techneissa wants them kept and stronger, not matched to upstream. They are in process V3 (`docs/process-versions.md`); V1 and V2 edits keep their old look until Update to Current Process. Measurements: `docs/slider-strength.md`.

Per slider group (all process V3 unless noted):

- **Contrast, Whites, Blacks.** One slider model on every tone path, so they respond as strongly on raws with a camera look and on JPEG / PNG as on raws without one. `tone::Sliders` bends gamma-space tones about the pivot (midtone slope x1.55 at +100, x0.6 at -100, black and white held), moves the white point 0.75 EV up / 0.7 EV down, and uses the blacks toe (`toe`); edited display curves get a looser `soft_clip` so Whites +100 reaches clipping. Built by `ToneMap::camera_hdr_with` and `ToneMap::display_hdr_with` (`SliderModel::Unified`), chosen in `finish::base_tone`. Neutral sliders stay the identity. Tests: `tests_tone_sliders` (`v3_sliders_are_visible_and_bounded_on_every_path`, `v3_paths_agree_within_tolerance`, `v3_whites_reach_clipping_on_display_sources`, `v3_neutral_is_identity_and_curves_are_sane`).
- **Highlights, Shadows (reach and roll-off).** Shadows +100 lifts the lower midtones (about 1.7 EV in the deepest tones, none above ~0.4 linear); Highlights -100 pulls from the upper midtones; Highlights +100 brightens the upper tones and then rolls off on a tanh shoulder before white, so a bright sky keeps its texture. Weights: `finish::shadows_highlights_delta(…, v3)` (`FinishParams::tone_v3`) and the same constants in `crates/gpu/src/wgsl/finish.wgsl` (`F_TONE_V3`). The slider preview's base radius matches the export at every quality (all processes). Tests in `tests_tone`: `shadows_open_the_whole_lower_half_but_not_to_grey`, `highlights_pull_down_the_upper_half_and_spare_the_shadows`, `positive_highlights_brighten_the_upper_tones_but_roll_off_before_white`, `v1_and_v2_keep_their_shadows_and_highlights_weights`, `base_radius_is_the_same_fraction_of_the_image_at_every_quality`.
- **Sharpening Radius, Detail, Masking (and the Amount response).** Radius (`detail.sharpenRadius`) sets a pixel-scale Gaussian band of `1.5 x Radius` original pixels, scaled to the render so a preview and an export sharpen the same edges (`local::sharpen_sigma(radius, detail_scale)`, `local::plane_sigmas`). The gain is `finish::sharpen_gain` (`SHARPEN_GAIN_V3` = 2.4 up to Radius 1, then `Radius^-0.7`). Detail and Masking ranges are fitted to that band (`finish::sharpen_mix`, `finish::sharpen_mask_weight(…, v3)`, shared with the ⌥-drag Masking preview). V1 and V2 ignore Radius and Detail. Tests: `tests_slider_strength` (`sharpening_radius_sets_the_band_scale`, `sharpening_radius_is_still_unread_before_v3`, `sharpening_detail_trades_edges_for_noise`), `tests_sharpen_mask`.
- **Vignette.** The post-crop response is smooth: `finish::vignette_dark_factor` darkens by `2.75 x |amount|^1.2` stops at the full falloff (about -2.8 / -1.2 / -0.4 stops in the corner at -100 / -50 / -20, not black at -100) and lightens by 0.40 of the way to white at +100. Test: `vignette_response_is_smooth_and_natural` (`tests_slider_strength`).
- **Noise Reduction Contrast** (`detail.nrContrast`), every process: upstream's code read it nowhere. `local::nr_keep` takes back the share of the luminance filter's change that is above noise; GPU kernel `nr_lum` in `crates/gpu/src/wgsl/map.wgsl`. 0 is an exact identity. Test: `nr_contrast_keeps_more_of_the_noise_and_detail`.
- **Texture, Clarity, Dehaze, Temp, Tint, Saturation and the rest.** Measured and found similar to Lightroom's strength, so unchanged. `strong_sliders_stay_strong` (`tests_slider_strength`) pins their floors.

Upstream's different sharpening (#757: `finish::sharpen_term`, `sharpen_limit`, `pixel_sharpen_mask_weight`, `local::pixel_sharpen`, `sharpen_sigma_raw`) was **not taken** (decided 2026-11-05). The parts of #757 that do not touch the sharpening maths are kept: `GAUSS_SIGMA_MAX` and the blur in `crates/raster/src/blur.rs`, `crates/gpu/src/wgsl/gauss.wgsl`, and `crates/pipeline/src/tests_sharpen.rs`, which is upstream's file adapted to the fork's functions.

## Branch

`claude/tone-slider-strength` (Contrast / Whites / Blacks), `claude/local-tone-draft-parity` (Highlights / Shadows), `claude/detail-followups` (Radius band, V3 gain, Masking preview, Vignette), `claude/slider-strength-audit` (Detail in V3, NR Contrast). All are merged into `custom`.

## Own files

`crates/pipeline/src/tests_tone_sliders.rs`, `crates/pipeline/src/tests_slider_strength.rs`, `crates/pipeline/src/tests_sharpen_mask.rs`, `crates/gpu/tests/it/equivalence.rs` (`process_v3_sharpening_and_vignette_match`), `docs/slider-strength.md`

## Hooks in official files

- `crates/pipeline/src/tone.rs`: `SliderModel`, `Sliders`, `toe`, `soft_clip`, `ToneMap::camera_hdr_with`, `ToneMap::display_hdr_with`.
- `crates/pipeline/src/finish.rs`: `base_tone`, `shadows_highlights_delta`, `vignette_dark_factor`, `SHARPEN_GAIN_V3`, `sharpen_gain`, `sharpen_mix`, `sharpen_mask_weight(masking, det, v3)`, the V3 sharpening branch.
- `crates/pipeline/src/local.rs`: `sharpen_sigma`, `plane_sigmas`, `PlaneSigmas::sharpen`, `Planes::sharpen`, `nr_keep`.
- `crates/pipeline/src/visualize.rs` (the Masking preview reads the V3 band), `crates/pipeline/src/lib.rs` (`Prepared::sharpen_blur`).
- GPU: `crates/gpu/src/params.rs` (`HAS_SHARP`, `SHARP_OFF`, `SHARPEN_DETAIL`, the `tone_v3` flag), `crates/gpu/src/wgsl/finish.wgsl`, `crates/gpu/src/wgsl/map.wgsl` (`nr_lum`), `crates/gpu/src/render.rs`.
- `crates/develop/src/settings.rs` (`Process::V3`), `docs/process-versions.md` (the V3 and V4 rows), `docs/slider-strength.md`.

## Tests

`tests_tone_sliders`, `tests_tone`, `tests_slider_strength`, `tests_sharpen`, `tests_sharpen_mask` in `lightcraft-pipeline`; with a GPU `cargo test -p lightcraft-gpu --test it -- process_v3_sharpening` (skips without).

## When upstream merges

Upstream's V4 (calibrated Contrast and Blacks, `light_v3`, `docs/light-v3.md`) sits on top of the fork's V3 only on raws with a camera tone curve; keep the fork's V3 under it and keep Whites as the fork's V3 has it.

For any other upstream change to slider maths (sharpening #757 `pixel_sharpen`, tone, Highlights / Shadows, vignette, Noise Reduction), follow the craftupdate skill rule "same feature on both sides":

1. Keep the fork's code in every file of the hooks list. Drop upstream's change to the slider maths in them.
2. Take upstream's changes that are not about the slider maths (the blur, `gauss.wgsl`, tests that still pass against the fork's functions).
3. Adapt an upstream test to the fork's functions, or leave it out if it only checks upstream's numbers. Never change the fork's gains, band scale, ranges or curves to make an upstream test pass.
4. Tell techneissa in plain words which slider upstream changed, that the fork kept its own stronger version, and what differs, so they can decide whether to want any of it.
