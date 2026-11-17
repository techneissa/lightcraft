//! Low mist and forest fog (issue #331): the haze a light's beams show in ([`crate::beam`]) thickens towards the ground,
//! and far things fade into the air.
//!
//! - **The ground.** A plane fitted to the photo's depth ([`Field::ground`], made once per photo): the surfaces that face
//!   up (their normals lean the way of the image's top) give its up direction, the lower of them its level. A tilted or a
//!   high camera, a rolled horizon, a slope: height is measured from that plane, never by the image's row. With no ground in
//!   the photo the plane lies under the lowest thing, level with the camera's own horizon.
//! - **Mist.** The density of the air at a point is the haze's, the same everywhere, plus the mist's, which falls off as
//!   `exp(−height / H)` above the ground (and is at its thickest below it, where the depth is noisy). `H`, Mist Height, is a
//!   share of the camera's own height over the ground, so it needs no metres: a relative depth map gives the same mist as a
//!   metric one. The beams walk this density along each line of sight, as they walk the haze.
//! - **Distance Fade** (aerial perspective). A surface is seen through the air in front of it: its light comes through dimmed
//!   by `T = exp(−τ)`, `τ` the air's optical depth up to it, and the air adds its own glow `(1 − T)` in the air's colour (the
//!   photo's far colour, a little of the light's). `τ` along a straight line through an exponential layer has a closed form,
//!   so this costs one pass over the field and no march. Near things have little air in front of them and keep their
//!   contrast; far ones, and the ground seen at a grazing angle through the mist, fade.
//!
//! Mist 0 and Distance Fade 0 leave the haze as it was to the bit: the density is the haze's own number, and nothing fades.

use rayon::prelude::*;

use crate::effects::Effects;
use crate::field::{Field, LENS};
use crate::light::Lobe;

/// The mist's optical depth at its ground density over [`REACH`] camera heights along the ground, at Mist 1 (it grows with
/// the square of Mist, as the haze's does with Haze): at Mist 100 a thing that far away on the ground keeps 5 % of its light,
/// a dense ground fog. The mist is measured in camera heights, not in the photo's farthest depth (often the sky's): a
/// garden's mist lies over its first few metres whatever the depth model makes of the sky.
const TAU_MIST: f32 = 3.0;
const REACH: f32 = 12.0;
/// The haze's optical depth from the camera to the photo's farthest thing at Haze 1 (`exp(−1.5)`: 22 % of that far thing's
/// light comes through); it grows with the square of Haze, so the low end is fine-grained.
const TAU_HAZE: f32 = 1.5;
/// Mist Height's span: the layer's scale height over the camera's height above the ground, at Mist Height 0 and 1 (it grows
/// with the square, so the low end, where mist lies, is fine-grained).
const HEIGHT_LOW: f32 = 0.06;
const HEIGHT_SPAN: f32 = 1.6;
/// The least share of a surface's light the fade lets through (keeps the planes' range bounded).
const FADE_FLOOR: f32 = 0.06;
/// The air begins at this share of the nearest thing's depth (as the beams' does).
const START: f32 = 0.3;
/// The ground's fit: the samples over the long side, how far up a surface's normal must lean (cosine) to be ground, and the
/// least share of the samples that must be ground for a fit.
const GROUND_SAMPLES: usize = 96;
const UPWARD: f32 = 0.5;
const UPWARD_REFINED: f32 = 0.8;
const GROUND_MIN: f32 = 0.03;
/// How much of the light's own colour the air's colour takes.
const LIGHT_SHARE: f32 = 0.3;

fn unit(v: f64, d: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { d }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let n = dot(v, v).sqrt();
    (n.is_finite() && n > 1e-6).then(|| v.map(|c| c / n))
}

fn lum(c: [f32; 3]) -> f32 {
    0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]
}

/// The ground under the photo: a point `q` (camera space) stands `dot(up, q) + level` above it. The camera stands at
/// `level` (always above 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ground {
    pub up: [f32; 3],
    pub level: f32,
}

impl Ground {
    /// The height of camera-space point `q` above the ground.
    pub fn height(&self, q: [f32; 3]) -> f32 {
        dot(self.up, q) + self.level
    }
}

/// The value at share `q` (0..1) of `v` (sorted in place).
fn quantile(v: &mut [f32], q: f32) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    let i = ((v.len().checked_sub(1)?) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    v.get(i).copied()
}

impl Field {
    /// The ground plane fitted to the photo's depth (see the module docs); made once.
    pub fn ground(&self) -> Ground {
        *self.ground.get_or_init(|| self.fit_ground())
    }

    fn fit_ground(&self) -> Ground {
        let (w, h) = (self.width, self.height);
        let step = (w.max(h) / GROUND_SAMPLES).max(1);
        let samples: Vec<([f32; 3], [f32; 3])> = (0..h)
            .step_by(step)
            .flat_map(|y| (0..w).step_by(step).map(move |x| (x, y)))
            .filter_map(|(x, y)| {
                let (lz, n, _) = self.at_px(y * w + x);
                let p = self.point((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, lz);
                (p.iter().chain(&n).all(|c| c.is_finite()) && lz <= self.far).then_some((p, n))
            })
            .collect();
        let level_under = |up: [f32; 3], q: f32, pts: &mut dyn Iterator<Item = [f32; 3]>| {
            let mut d: Vec<f32> = pts.map(|p| dot(up, p)).collect();
            quantile(&mut d, q).map(|v| -v)
        };
        let fallback = || {
            let up = [0.0, -1.0, 0.0];
            let level = level_under(up, 0.02, &mut samples.iter().map(|s| s.0)).unwrap_or(1.0);
            Ground { up, level: level.max(0.05) }
        };
        // (the up direction: the mean normal of what faces up, then of what faces that way closely)
        let mut up = [0.0, -1.0, 0.0];
        for cos in [UPWARD, UPWARD_REFINED] {
            let ground: Vec<[f32; 3]> = samples.iter().filter(|(_, n)| dot(*n, up) > cos).map(|(_, n)| *n).collect();
            if (ground.len() as f32) < GROUND_MIN * samples.len() as f32 || ground.is_empty() {
                return fallback();
            }
            let sum = ground.iter().fold([0.0f32; 3], |a, n| [a[0] + n[0], a[1] + n[1], a[2] + n[2]]);
            let Some(u) = normalized(sum) else { return fallback() };
            up = u;
        }
        // (the level: the lower of the ground's points, so a table or a bench top among them doesn't lift it)
        let Some(level) = level_under(up, 0.35, &mut samples.iter().filter(|(_, n)| dot(*n, up) > UPWARD_REFINED).map(|s| s.0)) else {
            return fallback();
        };
        if !(level.is_finite() && level > 1e-3) {
            return fallback();
        }
        Ground { up, level }
    }

    /// The air of effects `e` on this field: `None` with neither haze nor mist.
    pub fn medium(&self, e: &Effects) -> Option<Medium> {
        let zfar = self.far.exp().max(1e-6);
        let haze = unit(e.haze, 0.0);
        let mist = unit(e.mist, 0.0);
        if haze <= 0.0 && mist <= 0.0 {
            return None;
        }
        let sigma = TAU_HAZE * haze * haze / zfar;
        if mist <= 0.0 {
            return Some(Medium { haze: sigma, mist: 0.0, ground: Ground { up: [0.0, -1.0, 0.0], level: 1.0 }, inv_h: 0.0 });
        }
        let ground = self.ground();
        let mh = unit(e.mist_height, 0.3);
        let scale = (ground.level * (HEIGHT_LOW + HEIGHT_SPAN * mh * mh)).max(1e-4);
        Some(Medium { haze: sigma, mist: TAU_MIST * mist * mist / (REACH * ground.level), ground, inv_h: 1.0 / scale })
    }

    /// How far each pixel of the field fades into the air of effects `e` (Distance Fade, see the module docs), as the air's
    /// optical depth in front of it (`T = exp(−m)`): `None` with no Distance Fade, or no air.
    pub fn fade(&self, e: &Effects) -> Option<Lobe> {
        let k = unit(e.fade, 0.0);
        if k <= 0.0 {
            return None;
        }
        let medium = self.medium(e)?;
        // (on the shadows' reduced field, then smoothed over each surface and brought up edge-aware: a far grazing slope's
        // depth is noisy, and the mist's height there with it; per pixel it fades in speckles)
        let r = self.reduced(false);
        let long = self.width.max(self.height).max(1) as f32;
        let (kx, ky) = (LENS * self.width as f32 / long, LENS * self.height as f32 / long);
        let (fw, fh) = ((r.w * r.f).max(1) as f32, (r.h * r.f).max(1) as f32);
        let (start, zfar) = (START * self.near.exp(), self.far.exp());
        let most = -FADE_FLOOR.ln();
        let mut tau = vec![0f32; r.w * r.h];
        tau.par_chunks_mut(r.w.max(1)).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let z = r.z.get(y * r.w + x).copied().unwrap_or(0.0).min(zfar);
                let (u, v) = (((x * r.f + r.f / 2) as f32 + 0.5) / fw, ((y * r.f + r.f / 2) as f32 + 0.5) / fh);
                let d = [(u - 0.5) * kx, (v - 0.5) * ky, 1.0];
                let len = dot(d, d).sqrt();
                let e = d.map(|c| c / len);
                let (s0, s1) = (start * len, z * len);
                let t = if s1 > s0 { k * (medium.depth(e, s1) - medium.depth(e, s0)) } else { 0.0 };
                *o = if t.is_finite() { t.clamp(0.0, most) } else { 0.0 };
            }
        });
        let tau = crate::beam::same_surface_mean(r, &crate::beam::same_surface_mean(r, &tau, 2), 2);
        let mut m = self.upsampled(r, &tau, 0.0);
        m.par_iter_mut().for_each(|v| *v = if v.is_finite() { v.clamp(0.0, most) } else { 0.0 });
        Some(Lobe { m })
    }

    /// The air's colour (scene-linear Rec.2020, at the guide's exposure): the photo's far colour, as bright as the brighter
    /// of the far things and the photo's light quarter, with [`LIGHT_SHARE`] of the colour `light` (any level).
    pub fn air_colour(&self, light: [f32; 3]) -> [f32; 3] {
        let sky = self.air;
        let level = lum(sky).max(self.bright).max(1e-4);
        let chroma = |c: [f32; 3]| {
            let l = lum(c);
            if l.is_finite() && l > 1e-6 && c.iter().all(|v| v.is_finite() && *v >= 0.0) { c.map(|v| v / l) } else { [1.0; 3] }
        };
        let (s, l) = (chroma(sky), chroma(light));
        let mix: [f32; 3] = std::array::from_fn(|i| s[i] * (1.0 - LIGHT_SHARE) + l[i] * LIGHT_SHARE);
        let mix = chroma(mix);
        mix.map(|v| if (v * level).is_finite() { (v * level).max(0.0) } else { 0.0 })
    }
}

/// The air of a light's effects on a field ([`Field::medium`]): its density at each point, and its optical depth along a line
/// of sight.
#[derive(Clone, Copy, Debug)]
pub struct Medium {
    /// The haze's density (the same everywhere) and the mist's at the ground.
    haze: f32,
    mist: f32,
    ground: Ground,
    /// One over the mist's scale height.
    inv_h: f32,
}

impl Medium {
    /// The density at camera-space point `q`.
    pub fn at(&self, q: [f32; 3]) -> f32 {
        if self.mist <= 0.0 {
            return self.haze;
        }
        self.haze + self.mist * (-self.ground.height(q).max(0.0) * self.inv_h).exp()
    }

    /// The optical depth from the camera to distance `s` along the unit line of sight `e`.
    pub fn depth(&self, e: [f32; 3], s: f32) -> f32 {
        if self.mist <= 0.0 {
            return self.haze * s;
        }
        let s = s.max(0.0);
        self.haze * s + self.mist * layer(dot(self.ground.up, e) * self.inv_h, self.ground.level * self.inv_h, s)
    }
}

/// `∫₀ˢ exp(−max(a·t + c, 0)) dt`: the mist along a straight line whose height (in scale heights) is `a·t + c`.
fn layer(a: f32, c: f32, s: f32) -> f32 {
    // (the part below the ground has the ground's density; the rest falls off; it splits where the line meets the ground)
    let piece = |p: f32, q: f32| -> f32 {
        if q <= p {
            return 0.0;
        }
        let (hp, hq) = (a * p + c, a * q + c);
        if hp <= 0.0 && hq <= 0.0 {
            return q - p;
        }
        if a.abs() * (q - p) < 1e-4 {
            return (q - p) * (-(0.5 * (hp + hq)).max(0.0)).exp();
        }
        ((-hp).exp() - (-hq).exp()) / a
    };
    let v = if a.abs() > 1e-9 {
        let cross = (-c / a).clamp(0.0, s);
        piece(0.0, cross) + piece(cross, s)
    } else {
        piece(0.0, s)
    };
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layer_integral_matches_a_fine_sum() {
        for (a, c, s) in [(0.3, 2.0, 5.0), (-0.4, 1.5, 9.0), (0.0, 0.7, 3.0), (-1.2, 0.5, 4.0), (2.0, -1.0, 2.0)] {
            let n = 20_000;
            let sum: f32 = (0..n).map(|i| (-((i as f32 + 0.5) / n as f32 * s * a + c).max(0.0)).exp() * s / n as f32).sum();
            let v = layer(a, c, s);
            assert!((v - sum).abs() < 1e-3 * sum.max(1.0), "a {a} c {c} s {s}: {v} vs {sum}");
        }
        assert!(layer(f32::NAN, 1.0, 2.0).is_finite() && layer(1e30, -1e30, 1e30).is_finite());
    }
}
