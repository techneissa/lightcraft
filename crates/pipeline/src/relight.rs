//! Fork (issue #288, `fork/relight.md`): Relight in the render. The photo's field ([`SourceInfo::relight`], made
//! once per photo by the engine) and the settings give the light's shading over the whole photo, made once per
//! set of lights and kept ([`lit`]); each render reads its planes from it at its own pixels and hands them to the finish
//! stage as local adjustments ([`masks`]): the gain as an exposure (one plane where it brightens, one where it
//! darkens) and the light's colour as a temperature weighted by its share of each pixel's light. So the light
//! works in scene-linear light before the tone curve, on the CPU and the GPU alike, and a drag of the light runs
//! one pass over the field and one over the render's pixels.
//!
//! The planes take mask ids beyond what a photo's masks use ([`IDS`]) and come after them; nothing else of the
//! masks (their overlays, Amount, edges) sees them.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_develop::{DevelopSettings, LocalAdjustments};
use lightcraft_geom::Point;
use lightcraft_raster::Plane;
use lightcraft_relight::{Effects, Field, HAZE_GAIN, Light, Lobe, MAX_LIGHTS, Part, Pattern, Rig, Shading};

use crate::masks::{Evaluated, MaskAlpha};
use crate::{Plan, SourceInfo};

/// A photo's Relight field as renders get it: the field and a key naming it (the photo's depth and guide).
#[derive(Clone)]
pub struct Input {
    pub field: Arc<Field>,
    pub key: u64,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "relight::Input({:016x}, {} × {})", self.key, self.field.width, self.field.height)
    }
}

impl PartialEq for Input {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

/// The light of a render: the lights' shading over the photo (its colour planes carry the tilts).
#[derive(Clone)]
pub struct Lit {
    pub shading: Arc<Shading>,
}

/// The ids the light's planes take among the masks: brightening, darkening, warm colour, cool colour, and for a gel
/// magenta and green (those two after the others, so a light without a gel keeps the ids it had).
pub const IDS: [u32; 6] = [u32::MAX - 3, u32::MAX - 2, u32::MAX - 1, u32::MAX, u32::MAX - 5, u32::MAX - 4];

/// Shadings kept: one per set of lights (a drag makes a new one for each position; the newest are reused by the
/// draft, the full render and every window of the same lights).
const KEEP: usize = 4;

/// Lobes kept: one per light (a drag makes a new one for each position of the light it moves, and the others'
/// stay). A field of 1600 × 1067 makes 6.8 MB of them each, so this holds 8 lights and a drag's last positions.
const KEEP_LOBES: usize = 12;

static MEMO: Mutex<Vec<(u64, Arc<Shading>)>> = Mutex::new(Vec::new());
static LOBES: Mutex<Vec<(u64, Arc<Lobe>)>> = Mutex::new(Vec::new());

// (the settings and the light's numbers keep to the same cap)
const _: () = assert!(lightcraft_develop::relight::MAX_LIGHTS == lightcraft_relight::MAX_LIGHTS);

/// The lights that change the photo, and Darken Surroundings: what [`scene_of`] makes of the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// The lights with an Amount (a light with none adds nothing), at most [`MAX_LIGHTS`] of them.
    pub lights: Vec<Light>,
    /// Each of those lights' gel, pattern and haze (issue #302), in the same order.
    pub effects: Vec<Effects>,
    /// Darken Surroundings, 0..1.
    pub darken: f64,
}

/// The lights in settings `s` (`None`: Relight is off, or its section's eye is).
pub fn scene_of(s: &DevelopSettings) -> Option<Scene> {
    let r = &s.relight;
    if !r.is_on() || !s.section_enabled(lightcraft_develop::relight::SECTION) {
        return None;
    }
    let on = || r.lights.iter().take(MAX_LIGHTS).filter(|l| l.amount > 0.0);
    let lights = on()
        .map(|l| Light {
            x: l.x,
            y: l.y,
            depth: l.depth / 100.0,
            amount: l.amount / 100.0,
            softness: l.softness / 100.0,
            warmth: l.warmth / 100.0,
            wrap: l.wrap / 100.0,
        })
        .collect();
    let effects = on().map(|l| effects_of(&l.effects)).collect();
    Some(Scene { lights, effects, darken: r.darken / 100.0 })
}

/// A light's effects (gel, pattern, haze) as the light takes them.
fn effects_of(e: &lightcraft_develop::relight_effects::LightEffects) -> Effects {
    Effects {
        gel_hue: e.gel_hue,
        gel_strength: e.gel_strength / 100.0,
        pattern: Pattern::from_index(e.pattern),
        scale: e.pattern_scale / 100.0,
        rotation: e.pattern_rotation,
        softness: e.pattern_softness / 100.0,
        haze: e.haze / 100.0,
    }
}

/// The light of a render of `info` with settings `s` (`None`: off, or the photo has no field yet).
pub fn lit(info: &SourceInfo, s: &DevelopSettings) -> Option<Lit> {
    let input = info.relight.as_ref()?;
    let scene = scene_of(s)?;
    let (_, shading) = shading_for(input, &scene, s.light.exposure);
    Some(Lit { shading })
}

fn hashed(parts: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts(&mut h);
    std::hash::Hasher::finish(&h)
}

/// The key naming `light`'s lobe on `input`'s field: its place, depth, Softness and Wrap and its pattern (not Amount,
/// Warmth, a gel, haze or Darken Surroundings, which the lobe doesn't depend on).
fn lobe_key(input: &Input, light: &Light, effects: &Effects) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(0u8, input.key), h);
        for v in [light.x, light.y, light.depth, light.softness, light.wrap] {
            std::hash::Hash::hash(&v.to_bits(), h);
        }
        // (a light without a pattern hashes as it did before patterns: the same key, the same kept lobe)
        if effects.pattern != Pattern::None {
            std::hash::Hash::hash(&effects.pattern, h);
            for v in [effects.scale, effects.rotation, effects.softness] {
                std::hash::Hash::hash(&v.to_bits(), h);
            }
        }
    })
}

/// The key naming the shading of `scene` on `input`'s field for a photo at exposure `ev`: the field and every number the
/// shading depends on.
pub fn shading_key(input: &Input, scene: &Scene, ev: f64) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(1u8, input.key, ev.to_bits(), scene.darken.to_bits(), scene.lights.len()), h);
        for l in &scene.lights {
            for v in [l.x, l.y, l.depth, l.amount, l.softness, l.warmth, l.wrap] {
                std::hash::Hash::hash(&v.to_bits(), h);
            }
        }
        // (a scene whose lights have no effects hashes as it did before them)
        for e in scene.effects.iter().filter(|e| **e != Effects::default()) {
            std::hash::Hash::hash(&format!("{e:?}"), h);
        }
    })
}

/// `light`'s lobe on `input`'s field (`rig`: the light set up on it), seen through its pattern; made once and kept.
fn lobe_for(input: &Input, light: &Light, effects: &Effects, rig: &Rig) -> Arc<Lobe> {
    let key = lobe_key(input, light, effects);
    if let Some((_, l)) = LOBES.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
        return l.clone();
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let lobe = Arc::new(input.field.patterned(rig, input.field.lobe(rig), effects));
    if let Some(t) = t {
        eprintln!("    relight lobe ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    let mut memo = LOBES.lock().unwrap_or_else(PoisonError::into_inner);
    memo.retain(|(k, _)| *k != key);
    memo.insert(0, (key, lobe.clone()));
    memo.truncate(KEEP_LOBES);
    lobe
}

/// The shading of `scene` on `input`'s field for a photo at exposure `ev`, and the key naming it. Made once per set of
/// lights and kept, from one lobe per light that is made once and kept too: the render ([`lit`]) and the ⌥-drag preview
/// (`fork/relight.md`) take the same one, and dragging one light makes one lobe and one shading, not one per light.
pub fn shading_for(input: &Input, scene: &Scene, ev: f64) -> (u64, Arc<Shading>) {
    let key = shading_key(input, scene, ev);
    if let Some((_, sh)) = MEMO.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
        return (key, sh.clone());
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let rigs: Vec<Rig> = scene.lights.iter().take(MAX_LIGHTS).map(|l| input.field.rig(l, scene.darken, ev)).collect();
    let none = Effects::default();
    let effects = |i: usize| scene.effects.get(i).unwrap_or(&none);
    let lobes: Vec<Arc<Lobe>> = scene.lights.iter().zip(&rigs).enumerate().map(|(i, (l, r))| lobe_for(input, l, effects(i), r)).collect();
    // (a light's haze is one more lobe of the same light, in the same colour; made here, not kept: it is one pass)
    let hazes: Vec<Option<Lobe>> = scene.lights.iter().enumerate().map(|(i, l)| (effects(i).haze > 0.0).then(|| input.field.haze_lobe(l))).collect();
    let mut parts: Vec<Part<'_>> = Vec::with_capacity(2 * rigs.len());
    for (i, ((l, r), lobe)) in scene.lights.iter().zip(&rigs).zip(&lobes).enumerate() {
        let (tilt, tint) = effects(i).light_colour(l.warmth);
        parts.push(Part { lobe, gain: r.gain(), tilt, tint });
        if let Some(Some(haze)) = hazes.get(i) {
            parts.push(Part { lobe: haze, gain: r.gain() * HAZE_GAIN * effects(i).haze.clamp(0.0, 1.0) as f32, tilt, tint });
        }
    }
    let shading = Arc::new(input.field.shading_of(&parts, lightcraft_relight::ambient(scene.darken), ev as f32));
    if let Some(t) = t {
        eprintln!(
            "    relight shading ({} × {}, {} lights): {:.1} ms with the lobes",
            input.field.width,
            input.field.height,
            scene.lights.len(),
            t.elapsed().as_secs_f64() * 1e3
        );
    }
    let mut memo = MEMO.lock().unwrap_or_else(PoisonError::into_inner);
    memo.retain(|(k, _)| *k != key);
    memo.insert(0, (key, shading.clone()));
    memo.truncate(KEEP);
    (key, shading)
}

/// One of the light's planes as a mask the finish stage applies, with `adjust` per unit of it.
fn evaluated(id: u32, width: usize, height: usize, data: Vec<f32>, adjust: LocalAdjustments) -> Evaluated {
    Evaluated { id, alpha: Arc::new(MaskAlpha::from_plane(Plane { width, height, data })), amount: 1.0, adjust }
}

/// The light's planes for the render `plan` makes, as masks for the finish stage (none while Relight is off or the
/// photo has no field yet).
pub fn masks(plan: &Plan<'_>) -> Vec<Evaluated> {
    let Some(lit) = &plan.relight else { return Vec::new() };
    let (w, h) = (plan.w, plan.h);
    let t = crate::profiling().then(std::time::Instant::now);
    // (the light lives on the photo before the lens corrections and transform, like a mask made on it)
    let map = plan.frame.for_mask(true).out_to_mask(w, h);
    let sh = &lit.shading;
    let p = sh.split(w, h, &|x, y| {
        let q = map.apply(Point::new(x, y));
        (q.x, q.y)
    });
    let mut out = Vec::with_capacity(6);
    if sh.up_top > 1e-5 {
        out.push(evaluated(IDS[0], w, h, p.up, LocalAdjustments { exposure: f64::from(sh.up_top), ..LocalAdjustments::default() }));
    }
    if sh.down_top > 1e-5 {
        out.push(evaluated(IDS[1], w, h, p.down, LocalAdjustments { exposure: -f64::from(sh.down_top), ..LocalAdjustments::default() }));
    }
    // (the colour: one plane for the warm tilts, one for the cool ones, each with the strongest of its lights' tilts as
    // its temperature, so a pixel lit by a warm and a cool light gets the mix of the two)
    if let Some(warm) = p.warm {
        out.push(evaluated(IDS[2], w, h, warm, LocalAdjustments { temp: 100.0 * f64::from(sh.tilt_up), ..LocalAdjustments::default() }));
    }
    if let Some(cool) = p.cool {
        out.push(evaluated(IDS[3], w, h, cool, LocalAdjustments { temp: -100.0 * f64::from(sh.tilt_down), ..LocalAdjustments::default() }));
    }
    // (a gel's tint: one plane for the magenta, one for the green, in the same way)
    if let Some(magenta) = p.magenta {
        out.push(evaluated(IDS[4], w, h, magenta, LocalAdjustments { tint: 100.0 * f64::from(sh.tint_up), ..LocalAdjustments::default() }));
    }
    if let Some(green) = p.green {
        out.push(evaluated(IDS[5], w, h, green, LocalAdjustments { tint: -100.0 * f64::from(sh.tint_down), ..LocalAdjustments::default() }));
    }
    if let Some(t) = t {
        eprintln!("    relight planes ({w} × {h}): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    out
}
