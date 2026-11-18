//! The water's colour by distance (issue #333, `fork/relight.md`): water takes red first, then green, so far things turn
//! blue-green and lose contrast. All of it is drawn from numbers; nothing is read.
//!
//! A pixel `x` of the photo's depth range past its nearest thing is seen through water with a transmittance `T_c =
//! exp(−K · (k_c + s) · (b + x))` per colour (red's `k` the largest, `s` the water's scattering, the same for every colour,
//! `b` the water the light came down through and the camera looks through to the nearest thing, so the whole scene is under
//! water) and a veil of the water's own light, `V_c · (1 − T_c)`: `out = in · T + veil`. `T` keeps only [`SATURATION`] of its
//! colour (scattered light mixes the colours back): real water darkens and goes a muted blue-teal, never a bright cyan, and a
//! gain that tilts colour much past the luminance the finish stage keeps would clip a bright surface flat (issue #346). The finish stage has no per-colour gain, but its local exposure, temperature and tint together
//! are one: the gain on luminance is the exposure, red against blue the temperature, green against the two the tint
//! (the same split [`Effects::light_colour`](crate::Effects::light_colour) makes), so the planes of this module are those
//! and the light added on top (the veil), and the CPU and the GPU cannot differ.
//!
//! The water's distance is the photo's own: its depth map is relative, so the farthest thing is `K` optical lengths of the
//! clearest colour away, `K` set by Clarity.

use rayon::prelude::*;

use crate::field::Field;
use crate::light::Lobe;

/// The water's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    /// How much of the effect there is, 0 (none) .. 1.
    pub amount: f64,
    /// How clear the water is, 0 (murky) .. 1 (clear): how far the colour carries.
    pub clarity: f64,
    /// −1 (blue ocean) .. 0 (blue-green) .. 1 (green lake).
    pub tint: f64,
}

impl Default for Water {
    fn default() -> Self {
        Water { amount: 0.0, clarity: 0.5, tint: 0.0 }
    }
}

/// The optical depth of the photo's farthest thing, for the red channel, at Clarity 0 (murky) and 1 (clear).
const REACH: (f32, f32) = (7.0, 0.9);
/// How fast each colour is absorbed relative to red, for blue water, blue-green and green: (red, green, blue). Red goes
/// first, green next, blue lasts; a green lake's silt takes the blue.
const BLUE: [f32; 3] = [1.0, 0.32, 0.05];
const TEAL: [f32; 3] = [1.0, 0.2, 0.1];
const GREEN: [f32; 3] = [0.85, 0.12, 0.32];
/// What the water scatters away, the same for every colour, relative to red's absorption: the scene darkens with distance,
/// blue as well.
const SCATTER: f32 = 0.35;
/// The water between the nearest thing and the surface and camera, in the same units as the distance (the farthest thing is 1).
const BASE: f32 = 0.3;
/// How much of the transmittance's colour is kept (the rest is its luminance).
const SATURATION: f32 = 0.6;
/// The colour of the water's own light, for the same three: a muted blue, blue-teal and green (mixed with grey in
/// [`Water::light`]).
const LIGHT_BLUE: [f32; 3] = [0.1, 0.35, 0.75];
const LIGHT_TEAL: [f32; 3] = [0.1, 0.5, 0.58];
const LIGHT_GREEN: [f32; 3] = [0.25, 0.55, 0.32];
/// The water's light, as a share of the photo's median luminance (what far things turn into: darker than the scene).
const VEIL: f32 = 0.3;
/// 18 % grey, scene-linear.
const GREY: f32 = 0.18;
/// The rec. 2020 luminance weights.
const LUMA: [f32; 3] = [0.2627, 0.678, 0.0593];

fn unit(v: f64, d: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { d }
}

fn tint_of(v: f64) -> f32 {
    if v.is_finite() { v.clamp(-1.0, 1.0) as f32 } else { 0.0 }
}

fn blend(t: f32, blue: [f32; 3], teal: [f32; 3], green: [f32; 3]) -> [f32; 3] {
    let (a, b, w) = if t < 0.0 { (teal, blue, -t) } else { (teal, green, t) };
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * w)
}

impl Water {
    /// Whether it changes the photo.
    pub fn is_on(&self) -> bool {
        self.amount.is_finite() && self.amount > 0.0
    }

    /// The optical depth per unit of distance past the nearest thing (the farthest is 1), per colour.
    pub fn optical(&self) -> [f32; 3] {
        let reach = self.reach();
        blend(tint_of(self.tint), BLUE, TEAL, GREEN).map(|k| k * reach)
    }

    /// Red's optical depth of the photo's farthest thing in blue-green water.
    fn reach(&self) -> f32 {
        let (clarity, amount) = (unit(self.clarity, 0.5), unit(self.amount, 0.0));
        amount * (REACH.0 + (REACH.1 - REACH.0) * clarity)
    }

    /// What of each colour is left at distance `x` (0 the nearest thing, 1 the farthest), with the water in front of the nearest
    /// thing ([`BASE`]) and the colour muted ([`SATURATION`]).
    pub fn transmittance(&self, x: f32) -> [f32; 3] {
        if !self.is_on() {
            return [1.0; 3];
        }
        let x = if x.is_finite() { x.max(0.0) } else { 0.0 };
        let scatter = SCATTER * self.reach();
        let t = self.optical().map(|k| (-(k + scatter) * (BASE + x)).exp());
        let y = t[0] * LUMA[0] + t[1] * LUMA[1] + t[2] * LUMA[2];
        t.map(|v| (y + SATURATION * (v - y)).max(0.0))
    }

    /// The water's own light as colours of unit luminance.
    fn light(&self) -> [f32; 3] {
        // (a little white in it: water's light is never one pure colour)
        let c = blend(tint_of(self.tint), LIGHT_BLUE, LIGHT_TEAL, LIGHT_GREEN).map(|v| 0.65 * v + 0.35);
        let y = (c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]).max(1e-6);
        c.map(|v| v / y)
    }
}

/// The water over a field: for each field pixel the planes the finish stage takes (see the module docs).
#[derive(Clone, Debug)]
pub struct WaterPlanes {
    /// The local exposure, stops (≤ 0).
    pub stops: Lobe,
    /// The local temperature and tint, in the finish stage's units (`(1 + 0.3 t, 1 − 0.22 s, 1 − 0.3 t)`).
    pub temp: Lobe,
    pub tint: Lobe,
    /// The light added on top, per colour, in scene-linear light after the photo's exposure.
    pub veil: [Lobe; 3],
}

/// The most the temperature and tint planes ask of the finish stage (keeps blue's `1 − 0.3 t` above zero).
const TEMP_MAX: f32 = 3.3;
const TINT_MIN: f32 = -60.0;
const TINT_MAX: f32 = 4.5;
/// The least gain the exposure plane asks of the finish stage, stops.
const STOPS_MIN: f32 = -9.0;

impl Water {
    /// The exposure (stops), temperature and tint the finish stage takes for a pixel `x` of the depth range past the nearest
    /// thing (see [`Water::transmittance`]) and the veil's per-colour light there, for a veil of `level` (the scene-linear
    /// luminance of the water's own light).
    pub fn planes_at(&self, x: f32, level: f32) -> (f32, f32, f32, [f32; 3]) {
        self.pixel(self.transmittance(x), self.light(), level)
    }

    /// The planes for transmittance `t` and the veil's level `level`.
    fn pixel(&self, t: [f32; 3], light: [f32; 3], level: f32) -> (f32, f32, f32, [f32; 3]) {
        let y = (t[0] * LUMA[0] + t[1] * LUMA[1] + t[2] * LUMA[2]).max(1e-9);
        let (r, g, b) = (t[0], t[1], t[2]);
        let sum = (r + b).max(1e-9);
        let temp = ((r - b) / sum / 0.3).clamp(-TEMP_MAX, TEMP_MAX);
        let tint = ((1.0 - 2.0 * g / sum) / 0.22).clamp(TINT_MIN, TINT_MAX);
        let veil = [0, 1, 2].map(|k| level * light[k] * (1.0 - t[k]));
        (y.log2().max(STOPS_MIN), if temp.is_finite() { temp } else { 0.0 }, if tint.is_finite() { tint } else { 0.0 }, veil)
    }
}

impl Field {
    /// The median of the guide's luminance, linear, relative to 18 % grey (a coarse sample).
    fn median_level(&self) -> f32 {
        let step = (self.lum.len() / 4096).max(1);
        let mut v: Vec<f32> = self.lum.iter().step_by(step).copied().filter(|l| l.is_finite()).collect();
        if v.is_empty() {
            return 1.0;
        }
        let mid = v.len() / 2;
        let (_, m, _) = v.select_nth_unstable_by(mid, f32::total_cmp);
        m.clamp(-12.0, 6.0).exp2()
    }

    /// The water's planes over the field for a photo at exposure `ev` (stops; the veil is a share of the light the photo
    /// shows). `None` while the water is off.
    pub fn water_planes(&self, water: &Water, ev: f32) -> Option<WaterPlanes> {
        if !water.is_on() || self.ln_z.is_empty() {
            return None;
        }
        let (near, far) = (self.near.exp(), self.far.exp());
        let span = (far - near).max(1e-4);
        let level = VEIL * GREY * self.median_level() * if ev.is_finite() { ev.clamp(-10.0, 10.0).exp2() } else { 1.0 };
        let light = water.light();
        let n = self.ln_z.len();
        let (mut stops, mut temp, mut tint) = (vec![0f32; n], vec![0f32; n], vec![0f32; n]);
        let mut veil = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
        let [vr, vg, vb] = &mut veil;
        stops
            .par_iter_mut()
            .zip(temp.par_iter_mut())
            .zip(tint.par_iter_mut())
            .zip(vr.par_iter_mut().zip(vg.par_iter_mut()).zip(vb.par_iter_mut()))
            .zip(self.ln_z.par_iter())
            .for_each(|((((s, t), i), ((r, g), b)), lz)| {
                let x = ((lz.exp() - near) / span).max(0.0);
                let (a, c, d, v) = water.pixel(water.transmittance(x), light, level);
                (*s, *t, *i, *r, *g, *b) = (a, c, d, v[0], v[1], v[2]);
            });
        let [vr, vg, vb] = veil;
        Some(WaterPlanes {
            stops: Lobe { m: stops },
            temp: Lobe { m: temp },
            tint: Lobe { m: tint },
            veil: [Lobe { m: vr }, Lobe { m: vg }, Lobe { m: vb }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_goes_first_then_green_and_the_gain_is_what_each_colour_keeps() {
        let w = Water { amount: 1.0, clarity: 0.5, tint: 0.0 };
        let mut last = [1.0f32; 3];
        for x in [0.1, 0.3, 0.6, 1.0] {
            let t = w.transmittance(x);
            // each colour only falls with distance, red the most, then green, then blue
            assert!(t.iter().zip(&last).all(|(a, b)| a < b), "{t:?} after {last:?}");
            assert!(t[0] < t[1] && t[1] < t[2], "{t:?}");
            last = t;
        }
        // the nearest thing is under water too, but keeps most of its light; a clearer water carries every colour farther;
        // amount 0 is off
        assert!(w.transmittance(0.0).iter().all(|t| (0.2..1.0).contains(t)), "{:?}", w.transmittance(0.0));
        assert!(Water { clarity: 1.0, ..w }.transmittance(0.5)[0] > w.transmittance(0.5)[0]);
        assert!(!Water::default().is_on());
        assert!(Water::default().transmittance(1.0).iter().all(|t| *t == 1.0));
    }

    #[test]
    fn a_few_metres_down_is_darker_and_a_muted_blue_teal_never_a_bright_cyan() {
        // (issue #346) a grey surface of any brightness, near or far, at Water Depth 60 and any clarity and tint: the water only takes
        // light away, its colour stays muted (no channel far past the luminance, which the finish stage keeps, so a bright stair would
        // clip flat), and far things are darker than near ones
        let lum = |c: [f32; 3]| c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
        for clarity in [0.0, 0.5, 1.0] {
            for tint in [-1.0, 0.0, 1.0] {
                let w = Water { amount: 0.6, clarity, tint };
                for l in [0.2f32, 0.5, 0.9] {
                    let mut last = f32::MAX;
                    for x in [0.0, 0.25, 0.5, 1.0] {
                        let t = w.transmittance(x);
                        let (.., veil) = w.planes_at(x, VEIL * l);
                        let out = [0, 1, 2].map(|c| l * t[c] + veil[c]);
                        let y = lum(out);
                        assert!(y < l && y <= last + 1e-6, "clarity {clarity} tint {tint} x {x}: {out:?} from {l}");
                        let (hi, lo) = (out.iter().copied().fold(0f32, f32::max), out.iter().copied().fold(f32::MAX, f32::min));
                        assert!(hi / y < 1.6 && lo > 0.4 * hi, "too strong a colour, clarity {clarity} tint {tint} x {x}: {out:?}");
                        last = y;
                    }
                }
            }
        }
        // blue-teal: in blue-green water red is the lowest and blue the highest far away
        let w = Water { amount: 0.6, clarity: 0.5, tint: 0.0 };
        let t = w.transmittance(1.0);
        assert!(t[0] < t[1] && t[1] < t[2], "{t:?}");
    }

    #[test]
    fn hostile_numbers_give_finite_planes_and_a_flat_field_gets_a_uniform_one() {
        let f = Field::from_parts(32, 24, vec![0.0; 32 * 24], vec![-2.0; 32 * 24]).unwrap();
        for water in [Water { amount: f64::NAN, ..Water::default() }, Water { amount: 0.0, ..Water::default() }] {
            assert!(f.water_planes(&water, 0.0).is_none());
        }
        for water in [
            Water { amount: 1e9, clarity: f64::NAN, tint: f64::INFINITY },
            Water { amount: 1.0, clarity: -5.0, tint: -1e9 },
            Water { amount: 1.0, clarity: 0.0, tint: 1.0 },
        ] {
            let p = f.water_planes(&water, f32::NAN).unwrap();
            let all = p.stops.m.iter().chain(&p.temp.m).chain(&p.tint.m).chain(&p.veil[0].m).chain(&p.veil[1].m).chain(&p.veil[2].m);
            assert!(all.clone().all(|v| v.is_finite()));
            assert!(p.temp.m.iter().all(|t| t.abs() <= TEMP_MAX) && p.stops.m.iter().all(|s| *s <= 0.0 && *s >= STOPS_MIN));
        }
    }
}
