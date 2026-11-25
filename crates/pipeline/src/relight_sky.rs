//! Fork (issue #330, `fork/relight.md`): a sun's rays in the render. A Sun / Window light with Sky Rays adds rays the user
//! shapes (count, length, width and its variation, spread, softness), cut by the photo's clouds and near things as much as
//! Blocking says ([`lightcraft_relight::sun_rays`]); with Opposite Rays it adds soft bands converging on the point opposite the
//! sun ([`lightcraft_relight::sky`]). What cuts the rays is traced once per sun position and kept ([`ray_field_for`]); a drag of
//! a shape slider only redraws the fan over it ([`shaped_for`]), and strengths and the sun's colour only weigh the planes. They
//! reach the finish stage as local adjustments, light added on top (as a light's beams through haze do) and, for the bands
//! opposite, an exposure, so the CPU and the GPU render them alike.

use std::sync::Arc;

use rayon::prelude::*;

use lightcraft_develop::LocalAdjustments;
use lightcraft_develop::relight_effects::LightEffects;
use lightcraft_relight::sun_rays::{MAX_BRIGHT, MAX_DARK};
use lightcraft_relight::{LightKind, Lobe, MAX_LIGHTS, RayField, RayShape, Rays, SkyRays};

use crate::masks::Evaluated;
use crate::relight::{Input, Memo, Scene, evaluated, hashed};

/// A light's rays in the sky as the settings give them: strengths 0..1, the rays' shape, and the centre when it is placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyLight {
    pub rays: f64,
    pub opposite: f64,
    pub at: Option<(f64, f64)>,
    pub shape: RayShape,
}

impl SkyLight {
    pub(crate) fn of(e: &LightEffects) -> SkyLight {
        let unit = |v: f64| if v.is_finite() { (v / 100.0).clamp(0.0, 1.0) } else { 0.0 };
        let at = (e.rays_placed >= 0.5 && e.rays_x.is_finite() && e.rays_y.is_finite()).then_some((e.rays_x, e.rays_y));
        let num = |v: f64, fallback: f64| if v.is_finite() { v } else { fallback };
        let shape = RayShape {
            count: num(e.ray_count, 7.0).round().clamp(3.0, 60.0) as usize,
            length: unit(e.ray_length) as f32,
            width: unit(e.ray_width) as f32,
            variation: unit(e.ray_variation) as f32,
            direction: num(e.ray_direction, 0.0).clamp(-180.0, 180.0).to_radians() as f32,
            spread: num(e.ray_spread, 360.0).clamp(10.0, 360.0).to_radians() as f32,
            softness: unit(e.ray_softness) as f32,
            blocking: unit(e.ray_blocking) as f32,
            seed: 0,
        };
        SkyLight { rays: unit(e.sky_rays), opposite: unit(e.opposite_rays), at, shape }
    }

    pub(crate) fn is_on(&self) -> bool {
        self.rays > 0.0 || self.opposite > 0.0
    }
}

/// How Sky Rays' strength rises with the slider (0..1): `s^CURVE` of [`MAX_BRIGHT`] and [`MAX_DARK`], so the rays are plain
/// to see at 60 and strong at 100.
const CURVE: f32 = 1.3;

/// Sky Rays' strength at the slider's `s` (0..1).
fn strength(s: f64) -> f32 {
    let s = if s.is_finite() { s.clamp(0.0, 1.0) as f32 } else { 0.0 };
    s.powf(CURVE)
}

/// The share of the headroom below white that Opposite Rays 100 fills at its lighter bands.
const OPPOSITE_GAIN: f32 = 0.15;
/// How much Opposite Rays 100 darkens its dark bands (stops).
const OPPOSITE: f32 = 0.35;

/// One light's rays: its planes, the light the beams add per unit of their plane (scene-linear Rec.2020), and the stops
/// the shade takes per unit of its plane.
#[derive(Clone)]
pub struct Rayed {
    pub planes: Arc<SkyRays>,
    pub colour: [f32; 3],
    pub dark: f32,
}

/// Opposite Rays kept: one per centre (two planes of the field's size each, 13.7 MB on a 1600-pixel field).
static RAYS: Memo<SkyRays> = Memo::new(2);
/// What cuts Sky Rays, kept per sun position (two planes of the field's size), and the rays shaped over it (one plane).
static FIELDS: Memo<RayField> = Memo::new(2);
static SHAPED: Memo<SkyRays> = Memo::new(4);

thread_local! {
    /// How many times this thread traced what cuts Sky Rays (tests: a shape slider's drag traces nothing).
    static TRACES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn traces() -> usize {
    TRACES.with(std::cell::Cell::get)
}

pub(crate) fn forget() {
    RAYS.clear();
    FIELDS.clear();
    SHAPED.clear();
}

/// How much light a pixel can take before it shows as white: for the luminance `y` (0..1, linear) the field's guide shows
/// it at (the photo as shot, rendered as by default), the scene-linear light (units of 18 % grey) between its light in this
/// render (the default tone map turned round, times this render's exposure) and the light this render's tone map shows as
/// white. A table, and a key naming it.
pub(crate) struct Headroom {
    scene: Vec<f32>,
    white: f32,
    pub key: u64,
}

/// Steps of the table, over display luminance 0..1.
const ROOM_STEPS: usize = 256;
/// Display luminance a bright shaft stops short of.
const SHOWS_WHITE: f32 = 0.93;

impl Headroom {
    pub(crate) fn of(tone: &crate::tone::ToneMap, guide: &crate::tone::ToneMap, ev: f64) -> Headroom {
        // (the scene light a tone map shows at display luminance `y`: it rises with the light, so halve the bracket)
        let inverse = |tone: &crate::tone::ToneMap, y: f32| {
            let (mut lo, mut hi) = (0f32, 64f32);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if tone.apply(mid) < y { lo = mid } else { hi = mid }
            }
            0.5 * (lo + hi)
        };
        let gain = if ev.is_finite() { 2f32.powf(ev.clamp(-10.0, 10.0) as f32) } else { 1.0 };
        let scene: Vec<f32> = (0..=ROOM_STEPS).map(|k| gain * inverse(guide, k as f32 / ROOM_STEPS as f32)).collect();
        let white = inverse(tone, SHOWS_WHITE);
        let key = hashed(|h| {
            for v in scene.iter().step_by(16).chain([&white]) {
                std::hash::Hash::hash(&v.to_bits(), h);
            }
        });
        Headroom { scene, white, key }
    }

    fn at(&self, y: f32) -> f32 {
        let f = (y.clamp(0.0, 1.0) * ROOM_STEPS as f32).min(ROOM_STEPS as f32);
        let (k, t) = (f as usize, f.fract());
        let a = self.scene.get(k).copied().unwrap_or(self.white);
        let b = self.scene.get(k + 1).copied().unwrap_or(a);
        ((self.white - (a + (b - a) * t)) / crate::tone::GREY).max(0.0)
    }
}

/// The bands opposite the sun centred on `centre` on `input`'s field, with `room`; made once and kept. `None`: the photo has
/// no sky.
fn opposite_for(input: &Input, centre: (f32, f32), room: &Headroom) -> Option<Arc<SkyRays>> {
    let key = hashed(|h| std::hash::Hash::hash(&(6u8, input.key, Rays::Opposite, centre.0.to_bits(), centre.1.to_bits(), room.key), h));
    if let Some(r) = RAYS.get(key) {
        return Some(r);
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let r = Arc::new(input.field.opposite_rays(centre, &|y| room.at(y))?);
    if let Some(t) = t {
        eprintln!("    relight opposite rays ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    RAYS.put(key, r.clone());
    Some(r)
}

/// What cuts Sky Rays with the sun at `centre` on `input`'s field, with `room`, and its key; traced once per sun position and
/// kept. `None`: the photo has no sky.
pub(crate) fn ray_field_for(fields: &Memo<RayField>, input: &Input, centre: (f32, f32), room: &Headroom) -> Option<(u64, Arc<RayField>)> {
    let key = hashed(|h| std::hash::Hash::hash(&(7u8, input.key, centre.0.to_bits(), centre.1.to_bits(), room.key), h));
    if let Some(r) = fields.get(key) {
        return Some((key, r));
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let r = Arc::new(input.field.ray_field(centre, &|y| room.at(y))?);
    TRACES.with(|n| n.set(n.get() + 1));
    if let Some(t) = t {
        eprintln!("    relight sky rays trace ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    fields.put(key, r.clone());
    Some((key, r))
}

/// Sky Rays of `shape` with the sun at `centre`: the fan drawn over the [`ray_field_for`] kept in `fields`, itself kept in
/// `shaped`. The widths vary from a seed of the photo's field, so they stay put through every drag.
pub(crate) fn shaped_for(
    (fields, shaped): (&Memo<RayField>, &Memo<SkyRays>),
    input: &Input,
    centre: (f32, f32),
    room: &Headroom,
    shape: &RayShape,
) -> Option<Arc<SkyRays>> {
    let shape = RayShape { seed: input.key, ..*shape };
    let base = hashed(|h| std::hash::Hash::hash(&(7u8, input.key, centre.0.to_bits(), centre.1.to_bits(), room.key), h));
    let key = hashed(|h| {
        std::hash::Hash::hash(&(8u8, base, shape.count, shape.seed), h);
        for v in [shape.length, shape.width, shape.variation, shape.direction, shape.spread, shape.softness, shape.blocking] {
            std::hash::Hash::hash(&v.to_bits(), h);
        }
    });
    if let Some(r) = shaped.get(key) {
        return Some(r);
    }
    let (_, field) = ray_field_for(fields, input, centre, room)?;
    let t = crate::profiling().then(std::time::Instant::now);
    let (light, shade) = field.shape(&shape);
    if let Some(t) = t {
        eprintln!("    relight sky rays shape ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    let r = Arc::new(SkyRays { light, shade, glow: field.glow });
    shaped.put(key, r.clone());
    Some(r)
}

/// The scene's rays in the sky: each Sun / Window light's Sky Rays and Opposite Rays, in its own colour (Warmth and gel; Sky
/// Rays also take a paler version of the sky's own glow). None on a photo without a sky.
pub(crate) fn rays_of(input: &Input, scene: &Scene, room: &Headroom) -> Vec<Rayed> {
    let mut out = Vec::new();
    for (i, l) in scene.lights.iter().take(MAX_LIGHTS).enumerate() {
        let Some(sky) = scene.skies.get(i).filter(|s| s.is_on()) else { continue };
        if !matches!(l.kind, LightKind::Sun(_)) {
            continue;
        }
        let centre = |rays: Rays| sky.at.map_or_else(|| input.field.rays_centre(rays), |(x, y)| (x as f32, y as f32));
        let colour = |strength: f32, gain: f32| {
            let (t, s) = scene.effects.get(i).map_or((0.0, 0.0), |e| e.light_colour(l.warmth));
            let c = [1.0 + 0.3 * t, 1.0 - 0.22 * s, (1.0 - 0.3 * t).max(0.0)];
            // (its brightest channel 1: the plane is the headroom of the pixel's brightest channel)
            let top = c[0].max(c[1]).max(c[2]).max(1e-6);
            let k = gain * strength * crate::tone::GREY / top;
            c.map(|v| if (v * k).is_finite() { (v * k).max(0.0) } else { 0.0 })
        };
        if sky.rays > 0.0
            && let Some(planes) = shaped_for((&FIELDS, &SHAPED), input, centre(Rays::Sun), room, &sky.shape)
        {
            // (the planes are per unit of strength: the shafts' light, and the gaps' loss as a share, which as stops is
            // −log2(1 − share), about share / ln 2 at these few percent)
            let k = strength(sky.rays);
            let (c, g) = (colour(k, MAX_BRIGHT), planes.glow);
            let gt = g[0].max(g[1]).max(g[2]).max(1e-6);
            let dark = k * MAX_DARK / std::f32::consts::LN_2;
            out.push(Rayed { colour: [c[0] * g[0] / gt, c[1] * g[1] / gt, c[2] * g[2] / gt], dark, planes });
        }
        if sky.opposite > 0.0
            && let Some(planes) = opposite_for(input, centre(Rays::Opposite), room)
        {
            out.push(Rayed { colour: colour(sky.opposite as f32, OPPOSITE_GAIN), dark: OPPOSITE * sky.opposite as f32, planes });
        }
    }
    out
}

/// The ids the rays' planes take among the masks, two per light's rays: `u32::MAX - 40` down to `u32::MAX - 71`, clear of the
/// beams' and the sources' (`u32::MAX - 6` to `- 21`), Distance Fade's (`u32::MAX - 22` down by two per fade) and the water's
/// (`u32::MAX - 94` to `- 101`, and `- 120`).
const RAY_IDS: u32 = u32::MAX - 40;

/// The rays' planes for a render of `w × h` pixels (`at`: a render pixel's centre in normalized photo coordinates), as masks
/// for the finish stage: the beams' light added on top, the shade as an exposure.
pub(crate) fn masks(
    field: &lightcraft_relight::Field,
    rays: &[Rayed],
    w: usize,
    h: usize,
    at: &(dyn Fn(f64, f64) -> (f64, f64) + Sync),
) -> Vec<Evaluated> {
    let mut out = Vec::new();
    for (k, r) in rays.iter().enumerate().take(2 * MAX_LIGHTS) {
        let id = RAY_IDS - 2 * k as u32;
        let peak = |lobe: &Lobe| {
            let plane = field.plane_at(lobe, w, h, at);
            let top = plane.par_iter().copied().reduce(|| 0f32, f32::max);
            (top > 1e-6 && top.is_finite()).then(|| (plane.par_iter().map(|v| (v / top).clamp(0.0, 1.0)).collect::<Vec<f32>>(), top))
        };
        if r.colour.iter().any(|c| *c > 0.0)
            && let Some((alpha, top)) = peak(&r.planes.light)
        {
            let added = r.colour.map(|c| f64::from(c * top));
            out.push(evaluated(id, w, h, alpha, LocalAdjustments { fork_added: added, ..LocalAdjustments::default() }));
        }
        if r.dark > 0.0
            && let Some((alpha, top)) = peak(&r.planes.shade)
        {
            out.push(evaluated(id - 1, w, h, alpha, LocalAdjustments { exposure: -f64::from(r.dark * top), ..LocalAdjustments::default() }));
        }
    }
    out
}
