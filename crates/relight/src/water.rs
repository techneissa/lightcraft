//! The water's colour by distance (issue #333, `fork/relight.md`): water takes red first, then green, so far things turn
//! blue-green and lose contrast. All of it is drawn from numbers; nothing is read.
//!
//! A pixel `x` of the photo's depth range past its nearest thing is seen through water with a transmittance `T_c =
//! exp(−K · k_c · x)` per colour (red's `k` the largest) and a veil of the water's own light, `V_c · (1 − T_c)`:
//! `out = in · T + veil`. The finish stage has no per-colour gain, but its local exposure, temperature and tint together
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
/// The colour of the water's own light, for the same three.
const LIGHT_BLUE: [f32; 3] = [0.06, 0.38, 1.0];
const LIGHT_TEAL: [f32; 3] = [0.08, 0.72, 0.72];
const LIGHT_GREEN: [f32; 3] = [0.28, 0.9, 0.3];
/// The water's light, as a share of the photo's median luminance (what far things turn into).
const VEIL: f32 = 0.6;
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
        let (clarity, amount) = (unit(self.clarity, 0.5), unit(self.amount, 0.0));
        let reach = amount * (REACH.0 + (REACH.1 - REACH.0) * clarity);
        blend(tint_of(self.tint), BLUE, TEAL, GREEN).map(|k| k * reach)
    }

    /// What of each colour is left at distance `x` (0 the nearest thing, 1 the farthest).
    pub fn transmittance(&self, x: f32) -> [f32; 3] {
        let x = if x.is_finite() { x.max(0.0) } else { 0.0 };
        self.optical().map(|k| (-k * x).exp())
    }

    /// The water's own light as colours of unit luminance.
    fn light(&self) -> [f32; 3] {
        // (a little white in it: water's light is never one pure colour)
        let c = blend(tint_of(self.tint), LIGHT_BLUE, LIGHT_TEAL, LIGHT_GREEN).map(|v| 0.6 * v + 0.4);
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
        // no water, no loss; a clearer water carries every colour farther; amount 0 is off
        assert_eq!(w.transmittance(0.0), [1.0; 3]);
        assert!(Water { clarity: 1.0, ..w }.transmittance(0.5)[0] > w.transmittance(0.5)[0]);
        assert!(!Water::default().is_on());
        assert!(Water::default().transmittance(1.0).iter().all(|t| *t == 1.0));
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
