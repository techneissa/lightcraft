//! Sky Rays (issue #330): a sun's rays in the sky, shaped by the user and cut by what the photo shows.
//!
//! - **The shape** ([`RayShape`]) is the user's: how many rays, how far they reach, how wide they are and how much that
//!   varies from ray to ray, the angle range they fan over (a direction and a width) and how soft their edges are. The rays
//!   are radial lines from the sun's point, so one ray is a range of angles round it and the whole fan is one table over
//!   the angle, made in a moment. The widths vary from a seed fixed per photo, so a drag never makes them flicker.
//! - **What cuts them** ([`RayField`], made once per sun position): each reduced cell looks along the line towards the sun
//!   and sums what it passes. Clouds hold the light as the sky's cloud thickness says ([`crate::sky`]), except within the
//!   sun's own glow, and near things (land clearly nearer than the photo's far end: trees, buildings, a hill close by)
//!   are opaque. What it passes casts a penumbra that widens with the distance from it, so a cut's edge is soft and
//!   softens away from what casts it. Blocking blends between no cut at all (0) and this (100); the blend is linear, so
//!   the field keeps both ends and a drag of any shape slider never traces again.
//! - **Where they show:** the air in front of each pixel: all of it over the sky, some over far land and the haze at the
//!   horizon, none over the middle distance or near things. They fade along their length.
//! - **How they show: contrast.** A ray is a shaft a little brighter than the sky and the gap beside it a little darker,
//!   both as shares of the pixel's own light, so the slider means the same contrast on any sky. Where the sky has room below
//!   white the shafts brighten it; where it has little (a bright overcast, a near-white haze) the gaps darken instead, as
//!   real rays read against a bright sky. The brightening stays within the headroom, so nothing clips.
//!
//! All of it is drawn from numbers here; nothing is read.

use rayon::prelude::*;

use crate::field::Field;
use crate::light::Lobe;
use crate::sky::{PENUMBRA, STEP_PX, STEPS, THICK, level_at, smooth, soft_levels};

/// The sun's own glow, as a share of the long side: clouds within it cast no ray (a cloud's edge on the sun itself would
/// cut one hard line across the whole sky). Near things still do: a tree in front of the sun is where rays come from.
const SUN_GLOW: f32 = 0.05;
/// How much more a near thing holds the light than a cloud as thick as it is wide (opaque).
const NEAR_THICK: f32 = 6.0;
/// Land is near from this far (log depth) below the photo's farthest, in full from the second.
const NEAR_FROM: (f32, f32) = (0.35, 1.0);
/// How much air far land has in front of it against the sky, and from where along the photo's depth (0 nearest, 1 farthest)
/// it begins to: rays show over the haze of far hills, never over the middle distance.
const AIR_LAND: f32 = 0.75;
const AIR_FROM: f32 = 0.6;
/// At Sky Rays 100: the most light a shaft adds, and the most a gap loses, as shares of the pixel's own scene light (the
/// planes are per unit of each). The tone curve shows a brightening in a mid sky about as plainly as a third of it as a
/// darkening in a bright one. The brightening is kept within the headroom at its maximum.
pub const MAX_BRIGHT: f32 = 1.0;
pub const MAX_DARK: f32 = 0.35;
/// The headroom against the pixel's own light (scene light) below which its rays are all darker gaps, and above which all
/// brighter shafts.
const ROOM_SPLIT: (f32, f32) = (0.04, 0.45);
/// The angles (radians) round the sun a line is held against: a shaft is a line lit more than these.
const AROUND: [f32; 8] = [-0.3, -0.18, -0.1, -0.05, 0.05, 0.1, 0.18, 0.3];
/// Where the rays fade in from the sun (shares of the long side): near it a soft glow, no line ends.
const FADE_IN: (f32, f32) = (0.02, 0.15);
/// The glow round the sun, against a full ray, and its radius (share of the long side).
const GLOW: (f32, f32) = (0.35, 0.07);
/// A cloud's own pixels hide the rays from this thickness (0..1), in full from the second: a thick body, not the sky's
/// thinner haze, which the thickness map also counts a little.
const OWN: (f32, f32) = (0.45, 0.85);
/// A shaft's half-width at Ray Width 0 and how much Ray Width 100 adds (degrees, before Width Variation).
const WIDTH: (f32, f32) = (0.4, 7.0);
/// Steps of the fan's table over a full turn.
const TABLE: usize = 8192;

/// The shape of a sun's rays, as the user sets it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayShape {
    /// How many rays fan out over the spread, 1..60.
    pub count: usize,
    /// How far they reach, 0..1 (1: one and a half times the photo's long side).
    pub length: f32,
    /// How wide each ray is, 0..1: a half-width of 0.4° to 7.4° round the sun, at most half the gap to the next.
    pub width: f32,
    /// How much the widths and brightness differ from ray to ray, 0..1.
    pub variation: f32,
    /// Where the fan points from the sun (radians: 0 straight down the photo, π/2 to the right).
    pub direction: f32,
    /// How wide the fan is (radians, up to a full turn).
    pub spread: f32,
    /// How soft each ray's edges are, 0..1.
    pub softness: f32,
    /// How much the clouds and near things cut the rays, 0 (not at all) .. 1.
    pub blocking: f32,
    /// The seed the widths vary from (fixed per photo).
    pub seed: u64,
}

impl Default for RayShape {
    /// The controls' defaults: 7 broad soft shafts of uneven width all round, shaped by the clouds.
    fn default() -> Self {
        RayShape {
            count: 7,
            length: 0.6,
            width: 0.8,
            variation: 0.8,
            direction: 0.0,
            spread: std::f32::consts::TAU,
            softness: 0.85,
            blocking: 0.6,
            seed: 0,
        }
    }
}

/// A random number 0..1 from `state` (splitmix64), stepping it.
fn next(state: &mut u64) -> f32 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / (1u64 << 24) as f32
}

/// `a` wrapped to −π..π.
fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let r = (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI;
    if r.is_finite() { r } else { 0.0 }
}

impl RayShape {
    /// The fan over a full turn of angle (0..1 per step of [`TABLE`], from −π): each ray a soft range of angles, nothing
    /// outside the spread.
    fn tables(&self) -> (Vec<f32>, Vec<f32>) {
        use std::f32::consts::{PI, TAU};
        let n = self.count.clamp(1, 60);
        let spread = if self.spread.is_finite() { self.spread.clamp(1f32.to_radians(), TAU) } else { TAU };
        let half = spread / 2.0;
        let dir = if self.direction.is_finite() { self.direction } else { 0.0 };
        let spacing = spread / n as f32;
        let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let (var, soft) = (unit(self.variation), unit(self.softness));
        // (a shaft's half-width is its own angle, a few degrees, not a share of the gap to the next: few rays stay apart)
        let sigma = (WIDTH.0 + WIDTH.1 * unit(self.width)).to_radians().min(0.5 * spacing);
        let mut state = self.seed ^ 0x5EED_0330;
        let rays: Vec<(f32, f32, f32)> = (0..n)
            .map(|k| {
                let (a, b, c) = (next(&mut state), next(&mut state), next(&mut state));
                let at = dir - half + (k as f32 + 0.5) * spacing + (a - 0.5) * 0.7 * spacing * var;
                let width = sigma * (1.0 + 1.6 * var * (b - 0.5)).max(0.15);
                (at, width, 1.0 - 0.6 * var * c)
            })
            .collect();
        // (the spread's own edge tapers over a little of it, so a fan's side is no hard line; nothing passes it)
        let full = spread >= TAU - 1e-3;
        let taper = (0.25 * half).min(10f32.to_radians());
        (0..TABLE)
            .map(|bin| {
                let a = -PI + (bin as f32 + 0.5) / TABLE as f32 * TAU;
                let inside = if full { 1.0 } else { 1.0 - smooth(half - taper, half, wrap(a - dir).abs()) };
                if inside <= 0.0 {
                    return (0.0, 0.0);
                }
                let dark = rays.iter().fold(1.0f32, |dark, (at, width, bright)| {
                    // (a smooth profile across the shaft; Softness widens its shoulders to a full raised cosine)
                    let e = width * (0.08 + 0.92 * soft);
                    let on = 1.0 - smooth(width - e, width + e, wrap(a - at).abs());
                    dark * (1.0 - bright * on)
                });
                ((1.0 - dark) * inside, inside)
            })
            .unzip()
    }
}

/// What cuts a sun's rays from one sun position, at the field's size. See the module docs.
#[derive(Clone, Debug)]
pub struct RayField {
    width: usize,
    height: usize,
    /// The sun's point in field pixels.
    centre: (f32, f32),
    /// How much air the rays show in (0..1) with nothing cut, and with the clouds and near things cut.
    open: Vec<f32>,
    through: Vec<f32>,
    /// Per unit of strength: the light a full shaft adds (units of 18 % grey) and the share of its light a full gap loses.
    bright: Vec<f32>,
    dark: Vec<f32>,
    /// The rays' colour from the sky's own glow (scene-linear Rec.2020, luminance about 1).
    pub glow: [f32; 3],
}

impl RayField {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        (self.open.len() + self.through.len() + self.bright.len() + self.dark.len()) * 4
    }

    /// The rays of `shape` at the field's size, per unit of strength ([`MAX_BRIGHT`] and [`MAX_DARK`] at Sky Rays 100): the
    /// light the shafts add (units of 18 % grey) and the share of its light each gap loses. Cheap: one pass over the field with the fan's
    /// table, no tracing.
    pub fn shape(&self, shape: &RayShape) -> (Lobe, Lobe) {
        let (w, h) = (self.width, self.height);
        let (table, inside) = shape.tables();
        let long = w.max(h).max(1) as f32;
        let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let reach = (0.05 + 1.45 * unit(shape.length)) * long;
        let b = unit(shape.blocking);
        let c = self.centre;
        let mut light = vec![0f32; w * h];
        let mut shade = vec![0f32; w * h];
        light.par_chunks_mut(w.max(1)).zip(shade.par_chunks_mut(w.max(1))).enumerate().for_each(|(y, (lrow, srow))| {
            for (x, (lo, so)) in lrow.iter_mut().zip(srow.iter_mut()).enumerate() {
                let (dx, dy) = (x as f32 + 0.5 - c.0, y as f32 + 0.5 - c.1);
                let d = (dx * dx + dy * dy).sqrt();
                let fall = 1.0 - smooth(0.0, reach, d);
                if fall <= 0.0 {
                    continue;
                }
                let fade_in = smooth(FADE_IN.0 * long, FADE_IN.1 * long, d);
                let glow = GLOW.0 * (1.0 - smooth(0.0, GLOW.1 * long, d)).powi(2);
                // (the angle from straight down, towards the right: the tables')
                let f = (dx.atan2(dy) + std::f32::consts::PI) / std::f32::consts::TAU * TABLE as f32 - 0.5;
                let k = f.floor();
                let t = f - k;
                let read = |v: &[f32], k: f32| v.get((k as i64).rem_euclid(TABLE as i64) as usize).copied().unwrap_or(0.0);
                let lerp = |v: &[f32]| read(v, k) + (read(v, k + 1.0) - read(v, k)) * t;
                let (fan, within) = (lerp(&table), lerp(&inside));
                let i = y * w + x;
                let get = |v: &[f32]| v.get(i).copied().unwrap_or(0.0);
                let open = get(&self.open);
                // (how much of the shaft comes through the clouds and near things, 0..1)
                let lit = if open > 1e-6 { ((1.0 - b) + b * get(&self.through) / open).clamp(0.0, 1.0) } else { 0.0 };
                let env = fall * open;
                // (a shaft where the fan is and the light comes through; a gap within the fan's range where it doesn't, the
                // shadow a cloud casts included)
                let ray = (fan * lit).clamp(0.0, 1.0);
                let l = (ray * fade_in + glow).min(1.0) * env * get(&self.bright);
                let g = (1.0 - ray) * within * fade_in * env * get(&self.dark);
                *lo = if l.is_finite() { l.max(0.0) } else { 0.0 };
                *so = if g.is_finite() { g.clamp(0.0, 1.0) } else { 0.0 };
            }
        });
        (Lobe { m: light }, Lobe { m: shade })
    }
}

impl Field {
    /// What cuts a sun's rays with the sun at `centre` (normalized photo coordinates; may lie outside the photo), `None` for
    /// a photo without a sky. `headroom` is as for [`Field::opposite_rays`]. See the module docs; this is the slow part, made
    /// once per sun position and kept.
    pub fn ray_field(&self, centre: (f32, f32), headroom: &(dyn Fn(f32) -> f32 + Sync)) -> Option<RayField> {
        let s = self.sky();
        if !s.found || !(centre.0.is_finite() && centre.1.is_finite()) {
            return None;
        }
        let r = self.reduced(false);
        let (w, h) = (s.w, s.h);
        if (w, h) != (r.w, r.h) || w < 2 || h < 2 {
            return None;
        }
        let long = w.max(h) as f32;
        let at = (centre.0.clamp(-4.0, 5.0), centre.1.clamp(-4.0, 5.0));
        let c = (at.0 * w as f32, at.1 * h as f32);
        let far = self.far;
        // (what holds the light: the clouds of the sky and a dark cloud bank the depth model took for land; and land clearly
        // nearer than the far end)
        let get = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
        let cloud: Vec<f32> = (0..w * h).map(|i| (get(&s.backlit, i) * get(&s.sky, i)).max(get(&s.bank, i))).collect();
        let near: Vec<f32> = (0..w * h)
            .map(|i| (1.0 - get(&s.sky, i)) * (1.0 - smooth(far - NEAR_FROM.1, far - NEAR_FROM.0, r.lz.get(i).copied().unwrap_or(far))))
            .collect();
        let (clouds, nears) = (soft_levels(&cloud, w, h), soft_levels(&near, w, h));
        let march = |p: (f32, f32)| -> f32 {
            let (dx, dy) = (c.0 - p.0, c.1 - p.1);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 1.0 || !d.is_finite() {
                return 1.0;
            }
            let u = (dx / d, dy / d);
            let n = STEPS.min(((d / STEP_PX).ceil() as usize).max(1));
            let ds = d / n as f32;
            let mut tau = 0f32;
            for k in 0..n {
                let t = (k as f32 + 0.5) * ds;
                let q = (p.0 + u.0 * t, p.1 + u.1 * t);
                // (the penumbra of what the line passes at distance `t` widens with `t`: the sun's disc, and the haze between)
                let seen = smooth(0.5 * SUN_GLOW * long, 1.5 * SUN_GLOW * long, d - t);
                let held = seen * level_at(&clouds, w, h, long, q, PENUMBRA * t) + NEAR_THICK * level_at(&nears, w, h, long, q, PENUMBRA * t);
                tau += held * ds / long;
            }
            let v = (-THICK * tau).exp();
            if v.is_finite() { v } else { 0.0 }
        };
        let room = self.least_room(w, h, headroom, Some(&self.tint_top));
        // (the air in front of each pixel: all of it over the sky, a little over far land, none nearer)
        let span = (far - self.near).max(0.1);
        let air = |i: usize| {
            let k = get(&s.soft, i);
            let t = ((r.lz.get(i).copied().unwrap_or(far) - self.near) / span).clamp(0.0, 1.0);
            k + (1.0 - k) * AIR_LAND * smooth(AIR_FROM, 1.0, t)
        };
        // (how much air each cell's rays show in)
        let fills: Vec<f32> = (0..w * h)
            .map(|i| {
                let v = air(i) * (1.0 - get(&s.bank, i));
                if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
            })
            .collect();
        // (per unit of contrast: a shaft's light as a share of the pixel's own scene light, as much of it as the headroom lets
        // the shafts take; the rest of the contrast goes to darker gaps)
        let black = headroom(0.0);
        let (bright, dark): (Vec<f32>, Vec<f32>) = (0..w * h)
            .map(|i| {
                let c = self.tint.get(i).copied().unwrap_or([0.0; 3]);
                let own = (black - headroom(lightcraft_color::luminance_2020(c).clamp(0.0, 1.0))).max(1e-3);
                let room = get(&room, i);
                let share = smooth(ROOM_SPLIT.0, ROOM_SPLIT.1, room / own);
                let light = (share * own).min(0.9 * room / MAX_BRIGHT);
                let v = (if light.is_finite() { light.max(0.0) } else { 0.0 }, 1.0 - share);
                if v.1.is_finite() { v } else { (v.0, 1.0) }
            })
            .unzip();
        let lit: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| if get(&fills, i) > 0.0 || get(&cloud, i) > 0.0 { march(((i % w) as f32 + 0.5, (i / w) as f32 + 0.5)) } else { 1.0 })
            .collect();
        // (a shaft is a line lit more than the lines round it at the same distance from the sun, as in the sky's own rays: a
        // gap between clouds makes one, an evenly lit sky none. A line round it off the photo is marched there)
        let turn = |p: (f32, f32), a: f32| {
            let (sn, cs) = a.sin_cos();
            let (x, y) = (p.0 - c.0, p.1 - c.1);
            (c.0 + x * cs - y * sn, c.1 + x * sn + y * cs)
        };
        let lit_at = |q: (f32, f32)| {
            if q.0 >= 0.0 && q.1 >= 0.0 && q.0 < w as f32 && q.1 < h as f32 { get(&lit, (q.1 as usize) * w + q.0 as usize) } else { march(q) }
        };
        let raw: Vec<f32> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                if get(&fills, i) <= 0.0 {
                    return 0.0;
                }
                let p = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                let around = AROUND.iter().map(|a| lit_at(turn(p, *a))).sum::<f32>() / AROUND.len() as f32;
                (get(&lit, i) - 0.9 * around).max(0.0)
            })
            .collect();
        let top = {
            let mut on: Vec<f32> = raw.iter().copied().filter(|v| *v > 1e-4).collect();
            if on.len() < 8 { 1.0 } else { crate::sky::percentile(&mut on, 0.95).max(0.05) }
        };
        // (what comes through: half of the plain light, and the shafts the gaps make in full; a thick cloud's own pixels hold
        // the rays whatever Blocking is, as a cloud in front of them would)
        let own_of = |i: usize| smooth(OWN.0, OWN.1, get(clouds.first().map_or(&[][..], Vec::as_slice), i));
        let (open, through): (Vec<f32>, Vec<f32>) = (0..w * h)
            .map(|i| {
                let fill = get(&fills, i) * (1.0 - own_of(i));
                // (a soft roll-off, not a clip: a clip would turn the penumbra's soft edge into a step)
                let shaft = 1.0 - (-1.5 * (0.5 * get(&lit, i) + get(&raw, i) / top)).exp();
                (fill, fill * shaft)
            })
            .unzip();
        // (soft over about a percent of the long side: the headroom's and the clouds' cells never show as blocks)
        let ones = vec![1.0f32; w * h];
        let soft = (w.max(h) / 80).max(1);
        let mean = |v: &[f32]| crate::sky::sky_mean(v, &ones, w, h, soft);
        let (open, through, bright, dark) = (mean(&open), mean(&through), mean(&bright), mean(&dark));
        // (the brightening kept within the least headroom over the blur's reach and a cell more, after the blur: a brighter
        // neighbour's share never spills onto a pixel nearer white, also once brought up to the field's size)
        let reach = soft + 2;
        let bright: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let least = (y.saturating_sub(reach)..(y + reach + 1).min(h))
                    .flat_map(|yy| (x.saturating_sub(reach)..(x + reach + 1).min(w)).map(move |xx| yy * w + xx))
                    .map(|q| get(&room, q))
                    .fold(f32::MAX, f32::min);
                get(&bright, i).min(0.9 * least / MAX_BRIGHT)
            })
            .collect();
        Some(RayField {
            width: self.width,
            height: self.height,
            centre: (at.0 * self.width as f32, at.1 * self.height as f32),
            open: self.depth_upsampled(r, &open),
            through: self.depth_upsampled(r, &through),
            bright: self.depth_upsampled(r, &bright),
            dark: self.depth_upsampled(r, &dark),
            glow: s.glow,
        })
    }
}
