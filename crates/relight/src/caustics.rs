//! Light through moving water (issue #333, `fork/relight.md`): the network of bright lines a rippled surface throws on
//! what lies below it, drawn from numbers (a sum of ridged noise); nothing is read.
//!
//! The surface is a flat plane a little above the camera. A point of the scene (a surface, or a point of the air for the
//! shafts) is carried up along the sun's ray to that plane, and the network is read there, in plan: so the lines lie on
//! a floor as a floor shows them (foreshortened, converging), and the same line runs down the air as a shaft, which
//! wavers a little with the length of its path. The net is sharp near the surface and softer deeper (an edge is the ripples'
//! angular size times the path the light has run), and a pixel's own footprint is the least blur.
//!
//! The lobe's facing term (a sun from above) decides which surfaces get it: floors that face up are lit, a wall that
//! faces the camera or the side is not.

use crate::effects::Effects;
use crate::kinds::Shape;
use crate::light::Rig;
use crate::pattern::noise;

/// How far above the camera the surface lies (the photo's median depth is 1).
const SURFACE: f32 = 0.9;
/// Cells across the frame at the median depth at Scale 0 (each 0.1 of Scale divides it by about 1.26).
const CELLS: f32 = 40.0;
/// The ripples' angular size at Softness 0 and the most Softness adds (the blur per unit of path, in median depths).
const RIPPLE: (f32, f32) = (0.002, 0.012);
/// The network's mean brightness over the surfaces (the Amount of the light sets how much that is).
const MEAN: f32 = 0.5;
/// The brightest a line of the net gets, in units of the light's own level.
const PEAK: f32 = 4.0;
/// A line's half-width at no blur, in cells.
const LINE: f32 = 0.07;
/// The gradient of the noise the lines follow (typical, per cell): a line's distance is the noise over this.
const SLOPE: f32 = 1.6;

/// The water's surface for one sun, set up on a field.
#[derive(Clone, Debug)]
pub(crate) struct Surface {
    /// The unit vector towards the sun (camera space, y down).
    to: [f32; 3],
    /// The surface plane's y.
    top: f32,
    /// A cell's size, in median depths.
    cell: f32,
    /// The blur per unit of path, in median depths.
    ripple: f32,
    /// A field pixel's size per unit of depth.
    px: f32,
}

fn unit(v: f64, d: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { d }
}

/// The share of the surface a layer's lines of half-width `w` (cells) cover (measured over the noise: a line's reach into
/// the surface grows as `1 − exp(−2.6 w)`).
fn covered(w: f32) -> f32 {
    (1.0 - (-2.6 * w.clamp(0.0, 50.0)).exp()).clamp(1e-3, 1.0)
}

/// One layer of lines at `(u, v)` (cells): bright along the curves where a warped noise crosses zero, `w` cells wide.
fn layer(u: f32, v: f32, w: f32, salt: u32) -> f32 {
    let (wu, wv) = (u + 0.45 * noise(0.7 * u, 0.7 * v, salt), v + 0.45 * noise(0.7 * u + 5.2, 0.7 * v + 1.3, salt.wrapping_add(1)));
    let a = noise(wu, wv, salt.wrapping_add(2)).abs() / SLOPE;
    let w = w.max(1e-4);
    (-(a / w) * (a / w)).exp()
}

/// The network at `(u, v)` (cells) with lines `w` cells wide, at mean brightness [`MEAN`] over the surface whatever `w` is.
fn net(u: f32, v: f32, w: f32) -> f32 {
    // (two layers at other scales and angles: where lines of both cross they join into the cells' corners)
    let a = layer(u, v, w, 41);
    let b = layer(0.8 * u + 0.6 * v + 17.0, 0.8 * v - 0.6 * u - 9.0, 1.3 * w, 53);
    let both = 1.0 - (1.0 - covered(w)) * (1.0 - covered(1.3 * w));
    let k = MEAN / both.max(1e-3);
    let value = k * (a + b - a * b);
    if value.is_finite() { value.clamp(0.0, PEAK) } else { 0.0 }
}

impl Surface {
    /// The surface for sun `rig` and effects `e` (Scale: the cells' size, Softness: the ripples') on a field whose pixel is
    /// `px` wide per unit of depth; `None` for a light that is not a sun.
    pub(crate) fn new(rig: &Rig, e: &Effects, px: f32) -> Option<Surface> {
        let Shape::Sun { to, .. } = rig.shape else { return None };
        // (a sun on the horizon would carry every point out of the picture: kept at least a fifth of a turn above it)
        let up = (-to[1]).max(0.2);
        let to = [to[0], -up, to[2]];
        let cells = CELLS * 10f32.powf(-unit(e.scale, 0.5));
        let ripple = RIPPLE.0 + RIPPLE.1 * unit(e.softness, 0.3);
        Some(Surface { to, top: -SURFACE, cell: crate::field::LENS / cells, ripple, px: if px.is_finite() { px.max(0.0) } else { 0.0 } })
    }

    /// Where the ray from camera-space point `p` towards the sun meets the surface: its place there in cells, and the
    /// path's length. `None` for a point above the surface.
    fn locate(&self, p: [f32; 3]) -> Option<(f32, f32, f32)> {
        let t = (self.top - p[1]) / self.to[1];
        if !(t.is_finite() && t > 0.0) {
            return None;
        }
        let (a, b) = (p[0] + self.to[0] * t, p[2] + self.to[2] * t);
        let (u, v) = (a / self.cell, b / self.cell);
        (u.is_finite() && v.is_finite() && u.abs() < 1e5 && v.abs() < 1e5).then_some((u, v, t))
    }

    /// The light the net lets reach the surface point `p` (0 .. [`PEAK`], 0.5 on average): sharp lines near the surface,
    /// soft farther down, never finer than a pixel.
    pub(crate) fn at(&self, p: [f32; 3]) -> f32 {
        let Some((u, v, t)) = self.locate(p) else { return 0.0 };
        let blur = (self.ripple * t + 0.5 * self.px * p[2]) / self.cell;
        net(u, v, LINE + blur)
    }

    /// The share of the light (0 .. 1) that reaches the point `q` of the air: broad shafts, the net at its softest, which
    /// waver with the path's length and stay as the rays carry them.
    pub(crate) fn column(&self, q: [f32; 3]) -> f32 {
        let Some((u, v, t)) = self.locate(q) else { return 1.0 };
        let path = t / self.cell;
        // (the bands sway as they run down: slowly, and more with the path)
        let sway = noise(0.07 * path + 3.0, 0.3 * v, 7) * (0.04 * path).min(1.0);
        let blur = (self.ripple * t) / self.cell;
        (0.9 * net(u + 1.2 * sway, v + 0.4 * sway, 0.45 + blur)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_net_keeps_its_mean_whatever_the_blur_and_stays_finite() {
        // sharp lines and a soft glow carry the same light on average (a sharp net has high peaks, a soft one is even)
        let mean = |w: f32| {
            let n = 150;
            let sum: f32 = (0..n * n).map(|i| net((i % n) as f32 * 0.37 + 0.11, (i / n) as f32 * 0.41 + 0.07, w)).sum();
            sum / (n * n) as f32
        };
        for w in [0.07, 0.2, 0.6, 1.5] {
            let m = mean(w);
            assert!((0.3..=0.75).contains(&m), "width {w}: mean {m}");
        }
        let sharp_peak = (0..4000).map(|i| net(i as f32 * 0.13, i as f32 * 0.07, 0.07)).fold(0.0f32, f32::max);
        let soft_peak = (0..4000).map(|i| net(i as f32 * 0.13, i as f32 * 0.07, 1.5)).fold(0.0f32, f32::max);
        assert!(sharp_peak > 2.0 * soft_peak, "sharp {sharp_peak}, soft {soft_peak}");
        for v in [f32::NAN, f32::INFINITY, -1e30, 1e30] {
            assert!(net(v, 1.0, 0.1).is_finite() && net(1.0, v, f32::NAN).is_finite());
        }
    }
}
