//! Wrinkles v2, small helpers the search's parts share: the log picture and a smooth step.

use lightcraft_raster::Rgb32f;

/// The most a pixel is lightened (log light): a crease is lifted to the skin around it, never past a plausible range.
pub(super) const MOST_LIGHTER: f32 = 0.8;

pub(super) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The natural log of linear light per channel (floored, so black stays finite).
pub(super) fn log_image(img: &Rgb32f) -> Rgb32f {
    img.map(|p| p.map(|v| if v.is_finite() { v.max(1e-4).ln() } else { 1e-4f32.ln() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_picture_stays_finite() {
        let img = Rgb32f::from_fn(2, 1, |x, _| if x == 0 { [0.0, f32::NAN, 1.0] } else { [f32::INFINITY, -1.0, 0.5] });
        assert!(log_image(&img).data.iter().flatten().all(|v| v.is_finite()));
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
    }
}
