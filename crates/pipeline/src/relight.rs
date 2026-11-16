//! Fork (issue #288, `fork/relight.md`): Relight in the render. The photo's field ([`SourceInfo::relight`], made
//! once per photo by the engine) and the settings give the light's shading over the whole photo, made once per
//! light and kept ([`lit`]); each render reads its planes from it at its own pixels and hands them to the finish
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
use lightcraft_relight::{Field, Light, Shading};

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

/// The light of a render: its shading over the photo, and its colour as the finish stage's temperature.
#[derive(Clone)]
pub struct Lit {
    pub shading: Arc<Shading>,
    pub tilt: f32,
}

/// The ids the light's planes take among the masks: brightening, darkening, colour.
pub const IDS: [u32; 3] = [u32::MAX - 2, u32::MAX - 1, u32::MAX];

/// Shadings kept (one per light: a drag makes a new one for each position; the newest are reused by the draft,
/// the full render and every window of the same light).
const KEEP: usize = 4;

static MEMO: Mutex<Vec<(u64, Arc<Shading>)>> = Mutex::new(Vec::new());

/// The light in settings `s` (`None`: Relight is off, or its section's eye is).
pub fn light_of(s: &DevelopSettings) -> Option<Light> {
    let r = &s.relight;
    if !r.is_on() || !s.section_enabled(lightcraft_develop::relight::SECTION) {
        return None;
    }
    Some(Light {
        x: r.x,
        y: r.y,
        depth: r.depth / 100.0,
        amount: r.amount / 100.0,
        softness: r.softness / 100.0,
        warmth: r.warmth / 100.0,
        darken: r.darken / 100.0,
        wrap: r.wrap / 100.0,
    })
}

/// The light of a render of `info` with settings `s` (`None`: off, or the photo has no field yet).
pub fn lit(info: &SourceInfo, s: &DevelopSettings) -> Option<Lit> {
    let input = info.relight.as_ref()?;
    let light = light_of(s)?;
    let ev = s.light.exposure;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&input.key, &mut h);
    for v in [light.x, light.y, light.depth, light.amount, light.softness, light.darken, light.wrap, ev] {
        std::hash::Hash::hash(&v.to_bits(), &mut h);
    }
    let key = std::hash::Hasher::finish(&h);
    let tilt = lightcraft_relight::warmth_tilt(light.warmth);
    if let Some((_, sh)) = MEMO.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
        return Some(Lit { shading: sh.clone(), tilt });
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let shading = Arc::new(input.field.shading(&input.field.rig(&light, ev)));
    if let Some(t) = t {
        eprintln!("    relight shading ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    let mut memo = MEMO.lock().unwrap_or_else(PoisonError::into_inner);
    memo.retain(|(k, _)| *k != key);
    memo.insert(0, (key, shading.clone()));
    memo.truncate(KEEP);
    Some(Lit { shading, tilt })
}

/// One of the light's planes as a mask the finish stage applies: `plane` (any scale) divided by its largest value,
/// which goes into `adjust` through `put`. `None` when the plane is all zero.
fn evaluated(id: u32, width: usize, height: usize, data: Vec<f32>, put: impl FnOnce(f32) -> LocalAdjustments) -> Option<Evaluated> {
    let top = data.iter().copied().filter(|v| v.is_finite()).fold(0f32, f32::max);
    if top <= 1e-5 {
        return None;
    }
    let data = data.into_iter().map(|v| if v.is_finite() { (v / top).clamp(0.0, 1.0) } else { 0.0 }).collect();
    Some(Evaluated { id, alpha: Arc::new(MaskAlpha::from_plane(Plane { width, height, data })), amount: 1.0, adjust: put(top) })
}

/// The light's planes for the render `plan` makes, as masks for the finish stage (none while Relight is off or the
/// photo has no field yet).
pub fn masks(plan: &Plan<'_>) -> Vec<Evaluated> {
    let Some(lit) = &plan.relight else { return Vec::new() };
    let (w, h) = (plan.w, plan.h);
    let t = crate::profiling().then(std::time::Instant::now);
    // (the light lives on the photo before the lens corrections and transform, like a mask made on it)
    let map = plan.frame.for_mask(true).out_to_mask(w, h);
    let p = lit.shading.planes(w, h, &|x, y| {
        let q = map.apply(Point::new(x, y));
        (q.x, q.y)
    });
    let mut out = Vec::with_capacity(3);
    let up: Vec<f32> = p.stops.iter().map(|v| v.max(0.0)).collect();
    out.extend(evaluated(IDS[0], w, h, up, |top| LocalAdjustments { exposure: f64::from(top), ..LocalAdjustments::default() }));
    let down: Vec<f32> = p.stops.iter().map(|v| (-v).max(0.0)).collect();
    out.extend(evaluated(IDS[1], w, h, down, |top| LocalAdjustments { exposure: -f64::from(top), ..LocalAdjustments::default() }));
    if lit.tilt != 0.0 {
        let tilt = f64::from(lit.tilt);
        out.extend(evaluated(IDS[2], w, h, p.share, |top| LocalAdjustments { temp: 100.0 * tilt * f64::from(top), ..LocalAdjustments::default() }));
    }
    if let Some(t) = t {
        eprintln!("    relight planes ({w} × {h}): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    out
}
