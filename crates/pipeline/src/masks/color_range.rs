//! Colour Range masks: pixels whose colour is near one of the sampled colours.

use lightcraft_raster::{Plane, Rgb32f};

use super::smooth;

/// The alpha of a colour range with `samples` (OkLab, see [`tonemap_for_select`]) and Refine `refine`, over `img`
/// (before exposure: `gain` is the exposure's linear factor).
pub(super) fn fill(out: &mut Plane, img: &Rgb32f, samples: &[[f64; 3]], refine: f64, gain: f32) {
    let tol = color_range_tolerance(refine);
    // (a settings value built in code can skip the deserializer's cap: cost is pixels x samples)
    let samples: Vec<[f32; 3]> =
        samples.iter().take(lightcraft_develop::MAX_COLOR_RANGE_SAMPLES).map(|s| [s[0] as f32, s[1] as f32, s[2] as f32]).collect();
    for (v, c) in out.data.iter_mut().zip(&img.data) {
        let lab = lightcraft_color::perceptual::oklab_from_2020(tonemap_for_select(c.map(|v| v * gain)));
        let d = samples
            .iter()
            .map(|s| ((lab[1] - s[1]).powi(2) + (lab[2] - s[2]).powi(2) + 0.25 * (lab[0] - s[0]).powi(2)).sqrt())
            .fold(f32::MAX, f32::min);
        *v = 1.0 - smooth(tol * 0.5, tol, d);
    }
}

/// A rough display mapping for colour picking (so samples taken on screen match).
pub(crate) fn tonemap_for_select(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| v / (1.0 + v))
}

/// Colour range: the OkLab distance (a, b at full weight, L at a quarter) at which a pixel leaves
/// the selection for a Refine amount of 0..100. A pixel is fully selected up to half of it and
/// fades out over the other half (CPU here, `mask.wgsl` kind 3; both take this value).
///
/// Issue #157: `0.04 + 0.16·refine` selected the whole photo. Through [`tonemap_for_select`] a
/// slightly teal mid-grey (sRGB 120,150,155) and a neutral light grey (200,198,195) are only ~0.063
/// apart — L 0.60 vs 0.72, a/b ≤ 0.02 — so at the default Refine 50 (full selection below 0.06)
/// the grey patch was 99 % selected, and at Refine 85 / 100 still most of it. A JND is ~0.01 on
/// this scale, so Refine 50 now holds a surface up to ~2 JND from its sample and drops everything
/// beyond ~4.5 (full to 0.0225, gone at 0.045); Refine 85 is gone at 0.066, past that grey patch;
/// Refine 100 reaches 0.075 and Refine 0 is the sample's own shade (full to 0.0075, gone at 0.015).
pub fn color_range_tolerance(refine: f64) -> f32 {
    0.015 + 0.06 * (refine / 100.0).clamp(0.0, 1.0) as f32
}
