# Slider strength audit

> **Last updated:** 2026-10-30 · **Change:** minor (Sharpening Radius and the Vignette response, process V3)

How much does each Develop slider change the picture at its ends, measured on real renders? Users report
that most sliders feel weaker than Lightroom's. This page records the measurements, a verdict per slider and
what was changed. It tracks gap 3 in [gaps.md](gaps.md) (render fidelity is unmeasured against Lightroom).

## Method

`crates/pipeline/src/tests_slider_strength.rs` renders procedural images through `lightcraft_pipeline::render`
(CPU reference, default settings except the one slider, so the latest process, V3) and measures the 8-bit sRGB result.
Re-run the whole table with

```
cargo test -p lightcraft-pipeline --release slider_strength_report -- --ignored --nocapture
```

| Slider group | Test image | Metric |
|---|---|---|
| Temp, Tint | grey patch (scene 0.18) of a raw source shot at 5500 K, Bradford white balance | OkLab a, b, chroma of the grey |
| Vibrance, Saturation | patches of OkLab chroma 0.03 / 0.08 / 0.15 at hue 40 (skin) and 250 (blue) | chroma ratio to the unedited render |
| Colour mixer | one patch per band (HSV s 0.6, v 0.75 at the band hue) | hue shift (deg), chroma ratio, OkLab L change |
| Colour grading | neutral patches at scene luminance 0.01 / 0.18 / 0.8, wheel hue 220, sat 100 | OkLab chroma added; L change for Luminance |
| Parametric curve | grey ramp of 32 patches | largest rise / fall in 8-bit levels |
| Texture, Clarity | value-noise texture, 1024 x 683, periods 3 / 12 / 48 px at 0.3 EV each | energy of difference-of-Gaussian bands (1-2, 4-8, 16-32 px) and overall std, ratio to unedited |
| Dehaze | the same texture seen through a veil (transmission 0.55, airlight 0.7) | 5-95 percentile spread of encoded luma, mean chroma |
| Sharpening | soft step edge (sigma 1.2 px) over a flat noisy area (0.06 EV) | peak edge slope ratio, overshoot, flat-noise std ratio |
| Noise reduction | flat grey with 0.2 EV luma and 0.25 EV colour noise, plus a 2:1 step edge | luma / colour noise std ratio, edge contrast kept |
| Vignette | flat grey 600 x 400 | stops of luminance change at the corner and the middle of the long edge |
| Grain | flat grey 800 x 600 | std dev of the encoded luma in 8-bit levels |

**About the "Lightroom-typical" column.** The clean-room rules forbid reading Adobe's code, and no Lightroom
render could be measured here. The expectations are qualitative, from Lightroom's public documentation and
widespread user experience (for instance, Temp 5500 K to 3000 K turns a grey clearly blue; Saturation -100 is
monochrome and +100 about doubles chroma). The verdicts are judgement calls, not measurements against
Lightroom: *much weaker* = barely visible at the slider's end; *weaker* = visible but clearly short of
Lightroom; *similar*; *stronger*. The real comparison is still the fidelity suite of gap 3.

## Results (2026-10-30)

| Slider | Min / max measured | Lightroom-typical expectation | Verdict |
|---|---|---|---|
| Temp (5500 K to 3000 K) | grey a -0.026, b -0.104, chroma 0.108 (4000 K: 0.044; 7500 K: 0.025; 10000 K: 0.041) | clearly blue | similar |
| Tint +150 / -150 | chroma 0.115 (magenta) / 0.101 (green) | strong magenta / green | similar |
| Saturation -100 / +100 | chroma x0.00 / x2.0 (x1.5 on already chroma 0.15, gamut-limited) | monochrome / about double | similar |
| Vibrance -100 / +100 | low-chroma x0.12 / x1.40 (skin hue), x1.88 (blue); mid x0.52 / x1.2 (skin), x1.5 (blue); high x0.88 / x1.05 | +100 clearly lifts muted colours, less than Saturation, spares skin | similar to slightly weaker (skin protection holds skin to x1.4) |
| Colour mixer Hue +-100 | 22-29 deg on every band | up to about a neighbouring hue step | similar to weaker, left |
| Colour mixer Saturation -100 / +100 | chroma x0.00-0.10 / x1.21-1.84 | grey / strongly saturated | similar |
| Colour mixer Luminance -100 / +100 | OkLab L -0.16..-0.11 / +0.11..+0.16 | strong lightening / darkening of the band | similar to weaker, left |
| Colour grading wheel Saturation 100 | shadows wheel adds chroma 0.079 on deep shadows (0.001 on mid grey); midtones 0.066; highlights 0.035; global 0.09 | a strongly coloured tone range | weaker (a pale tint), left: no Lightroom measurement to size a retune |
| Grading Luminance -100 / +100 | L -0.10..-0.12 / +0.07..+0.12 | strong | similar |
| Grading Blending 0 / 100, Balance -100 / +100 | shadows reach into mid grey: 0.000 / 0.003 (blend); 0.054 / 0.000 (balance -100 / +100 at mid) | overlap grows with Blending, Balance moves the split | works; Blending's effect on the overlap is small, left |
| Parametric curve regions +-100 | 27-53 levels (shadows 29, darks 52, lights 52, highlights 27) | a region moves by roughly a fifth of the range | similar |
| Texture -100 / +100 | fine band x0.31 / x1.84; medium x0.90 / x1.11 | clearly softer / crisper fine detail | similar |
| Clarity -100 / +100 | medium band x0.22 / x1.80; coarse x0.68 / x1.35 | strongly softer / punchier midtone contrast | similar |
| Dehaze -100 / 0 / +50 / +100 | spread 0.019 / 0.056 / 0.098 / 0.417, chroma 0.004 / 0.012 / 0.021 / 0.077 | strongly hazier / dramatic clearing | similar to stronger |
| Sharpening Amount 40 / 100 / 150 (V1/V2) | edge slope x1.04 / x1.16 / x1.24; flat noise x1.31 / x1.78 / x2.18 | a visibly crisper soft edge | weaker on edges, noisy; unchanged (V1/V2 keep their look) |
| Sharpening Amount 40 / 100 / 150 (V3 before the Radius band, Detail 25) | slope x1.04 / x1.12 / x1.20; noise x1.24 / x1.62 / x1.94 | same | slightly less noise per edge gain; still modest on a sigma-1.2 edge because the band is fixed at 0.18 % of the long edge |
| Sharpening Radius 0.5 / 3.0 | identical to Radius 1 | wider halos / finer detail | **broken: not read** (V1/V2 still; fixed in V3, see below) |
| Sharpening Amount 40 / 60 / 100 / 150 (V3 after the Radius change, Radius 1) | slope x1.20 / x1.32 / x1.56 / x1.88; overshoot 0.05 / 0.09 / 0.14 / 0.23; noise x1.13 / x1.20 / x1.33 / x1.50 | clear at 40-60, strong but without heavy halos at 150 | works |
| Sharpening Radius 0.5 / 1 / 2 / 3 (V3, Amount 100) | slope x1.28 / x1.56 / x1.76 / x1.84; overshoot 0.02 / 0.14 / 0.30 / 0.36; halo 10 / 17 / 26 / 32 px on a 1000 px picture | halo widens with Radius | works |
| Sharpening Detail 0 / 100 (V1/V2) | identical to Detail 25 | halo suppression / fine-detail emphasis | **broken: not read** |
| Sharpening Detail 0 / 100 (V3, Amount 100; with the Radius band: slope x1.56 / x1.56, noise x1.13 / x1.96) | before the band: slope x1.04 / x1.36, noise x1.23 / x2.83 | 0 edges only, 100 also fine detail and noise | works (was a no-op) |
| Sharpening Masking 0 / 100 | slope 1.16 / 1.00, noise 1.78 / 1.00 | flat areas unsharpened at 100 | similar |
| NR Luminance 50 / 100 | luma noise std x0.74 / x0.32, edge contrast kept 1.00 | heavy smoothing at 100, edges kept | similar |
| NR Detail 0 / 100 (at 50) | x0.67 / x0.84 | higher keeps more detail | works |
| NR Contrast (at Luminance 50) | x0.74 before, x0.79 at 100 now (was identical at every value) | higher keeps contrast, noisy blotches | was **broken: not read**; fixed |
| Colour NR 50 / 100 | colour noise std x0.63 / x0.25 | strong | similar |
| Colour NR Detail 0 / Smoothness 100 | x0.50 (at 50: 0.63); Smoothness 0.62 vs 0.63 | Smoothness widens the blur | Detail works; Smoothness changes the radius only, so this metric cannot show it; left |
| Vignette -100 / -50 / +100 (V1/V2; mid 50, feather 50) | corner -10.8 / -1.0 / +2.3 stops, long-edge middle -1.6 / -0.6 / +1.8 | corners near black at -100 | uneven: black at -100, mild at -50; unchanged in V1/V2 |
| Vignette -100 / -50 / -20 / +50 / +100 (V3) | corner -2.79 / -1.21 / -0.42 / +0.92 / +1.49 stops, long-edge middle -1.82 / -0.79 / -0.27 / +0.67 / +1.13 | smooth, clearly visible, corners dark but not black | works |
| Vignette Midpoint 0 / 100, Feather 0, Roundness | midpoint 100: corner -0.3; roundness -100: corner -2.8, +100: -6.6 | effects move with the sliders | works |
| Grain Amount 25 / 50 / 100 | std 2.9 / 5.9 / 11.7 levels (Size has no effect on the std, as it should not; Roughness 100: 6.1) | subtle / visible / heavy | similar, left |

The Light panel (Contrast, Highlights, Shadows, Whites, Blacks) and draft / export parity are being retuned
by other work and are not part of this audit. White Balance is measured on a raw source without camera
matrices; with a camera colour model the matrices decide how colours move. Calibration, Point Color, Optics
and the HDR sliders are not covered yet.

## What changed

- **Noise Reduction Contrast** (`detail.nrContrast`): no code read it. It now takes back the share of the
  luminance filter's change that is above noise (`local::nr_keep`: none below 0.08 EV, full at 0.4 EV, scaled
  by the slider), so detail and contrast stay and noisy blotches come back. The GPU kernel `nr_lum` uses the
  same edges. 0 is an exact identity; the cache key (`Plan::lin_key`) includes it. Tests:
  `nr_contrast_keeps_more_of_the_noise_and_detail`.
- **Sharpening Detail** (`detail.sharpenDetail`), process V3 only: read by nothing before. Sharpening now
  weights the band detail by `finish::sharpen_mix` (edges above ~0.3 EV always, finer noise-sized detail by the
  slider), with gain 3.0 at first (1.5 since the Radius band, see below) instead of 1.3. The WGSL kernel carries the same constants. V1 and
  V2 render as before. Test: `sharpening_detail_trades_edges_for_noise`.
- **Sharpening Radius** (`detail.sharpenRadius`) and the Amount response, process V3 only: sharpening has a band of
  its own (`PlaneSigmas::sharpen`, `Prepared::sharpen_blur`, `Planes::sharpen`; Texture, local Noise and Defringe
  keep the fine texture band). Its Gaussian sigma is `local::sharpen_sigma`: `0.0055 * Radius^0.6` of the long
  edge (Radius 1: 0.55 %, 0.5: 0.36 %, 3: 1.06 %), so a preview and an export sharpen at the same scale. The
  old band was 0.18 % of the long edge, finer than a soft edge, which is why Amount barely sharpened one. The gain
  is retuned to 1.5 (`SHARPEN_GAIN_V3`) for the wider band. On the GPU the sharpening band is a second plane in
  the `tex` buffer at `SHARP_OFF` (the kernel has no free binding); `HAS_SHARP` gates it. V1 and V2 render as
  before. Tests: `sharpening_radius_sets_the_band_scale`, `sharpening_radius_is_still_unread_before_v3`.
- **Vignette** darkening, process V3 only: the multiplier `1 + amount * t` went black at -100 and was -1 stop at -50.
  V3 darkens by `2.75 * |amount|^1.2` stops at the full falloff (`finish::vignette_dark_factor`), blended in the
  log domain, and lightens by 0.40 of the way to white at +100 (was 0.85; +1.5 stops on grey instead of +2.3).
  Midpoint, Roundness, Feather and Highlights act on the falloff as before. Test:
  `vignette_response_is_smooth_and_natural` (min and max bounds, V1 / V2 as before).
- Floors for the sliders found at Lightroom strength (`strong_sliders_stay_strong`), so a later retune cannot
  weaken them.

## Left, and why

- Colour grading wheel strength, Vibrance on skin, mixer Hue / Luminance: weaker or borderline, but without a
  Lightroom reference a retune would be guessing; they belong to the fidelity suite and a new process version
  ([process-versions.md](process-versions.md)).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-30 | minor | Sharpening Radius gets its own band and Amount is retuned; Vignette response made smooth (process V3 only) |
| 2026-10-30 | minor | First audit: measurement harness, results table, Noise Reduction Contrast made to work |
