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

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use lightcraft_develop::relight::KindSettings;
use lightcraft_develop::{DevelopSettings, LocalAdjustments};
use lightcraft_geom::Point;
use lightcraft_raster::Plane;
use lightcraft_relight::{
    Beam, Effects, Field, Fill, Light, LightKind, Lobe, MAX_LIGHTS, Part, Pattern, Rig, Rim, Shading, Shafts, Softbox, Spot, Sun, Visibility, Water,
    WaterPlanes,
};
use rayon::prelude::*;

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

/// The light of a render: the lights' shading over the photo (its colour planes carry the tilts), the lights' beams
/// through haze (issue #317), light added on top, and how far things fade into the air (issue #331).
#[derive(Clone)]
pub struct Lit {
    pub shading: Arc<Shading>,
    pub beams: Vec<Beamed>,
    /// The photo's Distance Fade: at most one ([`fades_of`]).
    pub fades: Vec<Faded>,
    /// Each sun's rays between clouds (issue #330).
    pub rays: Vec<crate::relight_sky::Rayed>,
    /// The field the beams' planes are on.
    pub field: Arc<Field>,
    /// The underwater look's colour planes (issue #333); `None` while it has no water.
    pub water: Option<Arc<WaterPlanes>>,
}

/// One light's beam: its plane over the field ([`Field::beam_plane`], in units of 18 % grey per unit of the light's strength)
/// and the scene-linear Rec.2020 light it adds per unit of the plane (the light's colour, its strength and 18 % grey).
#[derive(Clone)]
pub struct Beamed {
    pub plane: Arc<Lobe>,
    pub colour: [f32; 3],
    /// Which of the beams' mask ids it takes (below [`BEAM_IDS`]): the light's index, or [`MAX_LIGHTS`] more for a light's
    /// source seen as a bright point (issue #334), which is a plane of its own so a beam's detail keeps its scale.
    pub slot: u32,
    /// The water's shafts (issue #333), not a light's own beam: they take [`SHAFT_ID`].
    pub shafts: bool,
}

/// The photo's Distance Fade (issue #331): the air's optical depth in front of each pixel of the field ([`Field::fade`]), and the
/// air's colour (scene-linear Rec.2020, at the render's exposure) things fade to.
#[derive(Clone)]
pub struct Faded {
    pub plane: Arc<Lobe>,
    pub colour: [f32; 3],
}

/// The ids the beams' planes take among the masks (after [`IDS`]): `BEAM_IDS` minus the beam's slot, `u32::MAX - 6` down to
/// `u32::MAX - 21` (a light's beam, then its source's bright point, issue #334, `MAX_LIGHTS` slots on).
const BEAM_IDS: u32 = u32::MAX - 6;
/// The ids the fade's planes take (after the beams' and the sources', which use `2 · MAX_LIGHTS` slots): the dimming and the
/// air's glow, `u32::MAX - 22` down.
const FADE_IDS: u32 = BEAM_IDS - 2 * lightcraft_relight::MAX_LIGHTS as u32;

/// The ids the water's planes take (issue #333): exposure, temperature (cool, warm), tint (green, magenta), the veil's red,
/// green and blue, `u32::MAX - 101` up to `u32::MAX - 94`, and the shafts' beam `u32::MAX - 120`. Ranges among the masks: the
/// light's planes `u32::MAX - 5` up, the beams' and the sources' `u32::MAX - 21` to `u32::MAX - 6`, Distance Fade's (issue
/// #331) from `u32::MAX - 22` down by two for each fade (one, today), a sun's rays in the sky (issue #330) `u32::MAX - 40` to
/// `u32::MAX - 71`, the water's below all of those.
const WATER_IDS: u32 = u32::MAX - 93;
const SHAFT_ID: u32 = u32::MAX - 120;

/// The lights a scene holds: the photo's own and the water's sun of the caustics (issue #333).
const MAX_SCENE: usize = MAX_LIGHTS + 1;

/// The ids the light's planes take among the masks: brightening, darkening, warm colour, cool colour, and for a gel
/// magenta and green (those two after the others, so a light without a gel keeps the ids it had).
pub const IDS: [u32; 6] = [u32::MAX - 3, u32::MAX - 2, u32::MAX - 1, u32::MAX, u32::MAX - 5, u32::MAX - 4];

/// Shadings kept: one per set of lights (a drag makes a new one for each position; the newest are reused by the
/// draft, the full render and every window of the same lights).
const KEEP: usize = 4;

/// Lobes kept: one per light (a drag makes a new one for each position of the light it moves, and the others'
/// stay). A field of 1600 × 1067 makes 6.8 MB of them each (twice that for a light with a pattern, which keeps the light
/// its material passes too), so this holds 8 lights and a drag's last positions.
const KEEP_LOBES: usize = 12;

/// A light's lobes: the light itself (through its pattern's gaps) and, with a pattern, what the pattern's own material
/// passes (issue #316), which takes its own tint.
pub struct Lobes {
    pub light: Lobe,
    pub through: Option<Lobe>,
}

/// A few values most recently used first, by key.
pub(crate) struct Memo<T> {
    kept: Mutex<Vec<(u64, Arc<T>)>>,
    keep: usize,
    /// The keys a render is making now ([`Memo::claim`]), and the signal that one of them is done.
    making: Mutex<Vec<u64>>,
    made: Condvar,
}

/// What [`Memo::claim`] found: the value kept, or the right to make it (the key is free again when this drops, made or not).
pub(crate) enum Claim<'a, T> {
    Kept(Arc<T>),
    Make(Making<'a, T>),
}

/// The key a render makes ([`Claim::Make`]).
pub(crate) struct Making<'a, T> {
    memo: &'a Memo<T>,
    key: u64,
}

impl<T> Drop for Making<'_, T> {
    fn drop(&mut self) {
        self.memo.making.lock().unwrap_or_else(PoisonError::into_inner).retain(|k| *k != self.key);
        self.memo.made.notify_all();
    }
}

impl<T> Memo<T> {
    pub(crate) const fn new(keep: usize) -> Self {
        Memo { kept: Mutex::new(Vec::new()), keep, making: Mutex::new(Vec::new()), made: Condvar::new() }
    }

    /// The value of `key` ([`Memo::get`]), or, when no render is making it, the right to make it. While another render makes
    /// it, this waits for that one and takes what it kept (issue #374: the loupe and the open photo's filmstrip thumbnail, both
    /// rendered after an edit, each made the same whole-field shading at once).
    pub(crate) fn claim(&self, key: u64) -> Claim<'_, T> {
        let mut making = self.making.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(v) = self.get(key) {
                return Claim::Kept(v);
            }
            if !making.contains(&key) {
                making.push(key);
                return Claim::Make(Making { memo: self, key });
            }
            making = self.made.wait(making).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The value of `key`, which becomes the most recently used (so what is asked for every step of a drag stays).
    pub(crate) fn get(&self, key: u64) -> Option<Arc<T>> {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        let at = kept.iter().position(|(k, _)| *k == key)?;
        let entry = kept.remove(at);
        let value = entry.1.clone();
        kept.insert(0, entry);
        Some(value)
    }

    /// Keep `value` for `key` as the most recently used, pushing out the least.
    pub(crate) fn put(&self, key: u64, value: Arc<T>) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.retain(|(k, _)| *k != key);
        kept.insert(0, (key, value));
        kept.truncate(self.keep);
    }

    pub(crate) fn clear(&self) {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// [`Memo::put`] when `keep`: a small render's planes ([`Tier::Small`]) are made and dropped, so a flyout of preset
    /// pictures never pushes out the photo's own.
    fn put_if(&self, keep: bool, key: u64, value: Arc<T>) {
        if keep {
            self.put(key, value);
        }
    }
}

static MEMO: Memo<Shading> = Memo::new(KEEP);
static LOBES: Memo<Lobes> = Memo::new(KEEP_LOBES);
/// Shadow visibilities kept (issue #315): one per light (and one coarse one for a light being dragged), at a sixteenth of the field's size, so the Shadows slider and
/// the light's other numbers don't march again; only a move of the light, its size or its type's numbers do.
static SHADOWS: Memo<Visibility> = Memo::new(2 * KEEP_LOBES);
/// Beams (issue #317): the rays from the air towards each light, kept until the light moves (about 3.4 MB each on a 1600-pixel
/// field), and the beam they make with the light's Haze, Forward Scatter and Dust (0.4 MB), so those sliders only weigh the
/// kept rays again.
static SHAFTS: Memo<Shafts> = Memo::new(KEEP_LOBES);
static BEAMS: Memo<Beam> = Memo::new(KEEP_LOBES);
/// The beams at the field's size, as lobes (apart from the lights' own lobes, so the beams never push those out).
static BEAM_LOBES: Memo<Lobe> = Memo::new(KEEP_LOBES);
/// Distance Fades (issue #331), one per light's air: they don't depend on the light's place, so a drag of the light reuses them.
static FADES: Memo<Lobe> = Memo::new(KEEP_LOBES);
/// The underwater colour planes (issue #333): one per set of the water's numbers on a field (six planes of 4 bytes a pixel: about 41 MB on a 1600 × 1067 one, two sets kept).
static WATER: Memo<WaterPlanes> = Memo::new(2);
/// The glow past an edge (issue #332) at the field's size, per light and place, and the glow and the beam added up where a light
/// has both. Cheap to make (a closed form over the field), so few are kept.
static GLOW_LOBES: Memo<Lobe> = Memo::new(KEEP_LOBES);
static ADDED_LOBES: Memo<Lobe> = Memo::new(KEEP_LOBES);
/// The fields at half their size ([`Field::halved`]) the drags and the small renders take, one per photo (two kept: the
/// photo shown and the one before).
static HALVED: Memo<Field> = Memo::new(2);

/// Let go of every kept lobe and shading (the engine drops a photo's field: what was made from it goes too; they are
/// made again by the next render). They are keyed by the field, so one photo's go with another's.
pub fn forget() {
    MEMO.clear();
    LOBES.clear();
    SHADOWS.clear();
    SHAFTS.clear();
    BEAMS.clear();
    BEAM_LOBES.clear();
    FADES.clear();
    WATER.clear();
    GLOW_LOBES.clear();
    ADDED_LOBES.clear();
    HALVED.clear();
    crate::relight_sky::forget();
}

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
    /// The underwater look's colour (issue #333); off by default.
    pub water: Water,
    /// Lights that only show their beam (the water's shafts, issue #333): each with its effects. They add no light to the
    /// surfaces and take no place among `lights`.
    pub beams: Vec<(Light, Effects)>,
    /// Each of those lights' rays between clouds (issue #330), in the same order.
    pub skies: Vec<crate::relight_sky::SkyLight>,
}

impl Scene {
    /// Whether some light casts shadows.
    pub fn has_shadows(&self) -> bool {
        self.lights.iter().zip(&self.effects).any(|(l, e)| shadow_amount(l, e).is_some())
    }

    /// Whether a drag's shading differs from the full one: some light casts shadows (marched coarser) or has a pattern
    /// (drawn coarser; a rim or a fill with one casts no shadows).
    pub fn drafts_differ(&self) -> bool {
        self.has_shadows() || self.effects.iter().any(|e| e.pattern != Pattern::None)
    }
}

/// The lights in settings `s` (`None`: Relight is off, or its section's eye is).
pub fn scene_of(s: &DevelopSettings) -> Option<Scene> {
    let r = &s.relight;
    if !r.is_on() || !s.section_enabled(lightcraft_develop::relight::SECTION) {
        return None;
    }
    let on = || r.lights.iter().take(MAX_LIGHTS).filter(|l| l.amount > 0.0);
    let mut lights: Vec<Light> = on()
        .map(|l| Light {
            x: l.x,
            y: l.y,
            depth: l.depth / 100.0,
            amount: l.amount / 100.0,
            softness: l.softness / 100.0,
            warmth: l.warmth / 100.0,
            wrap: l.wrap / 100.0,
            kind: kind_of(&l.kind),
        })
        .collect();
    let mut effects: Vec<Effects> = on().map(|l| effects_of(&l.effects)).collect();
    let (water, beams) = underwater(&r.water, &mut lights, &mut effects);
    let skies = on().map(|l| crate::relight_sky::SkyLight::of(&l.effects)).collect();
    Some(Scene { lights, effects, darken: r.darken / 100.0, water, beams, skies })
}

/// The underwater look (issue #333) as the scene takes it: the water's colour, the sun of the caustics added to `lights`
/// and `effects`, and the sun of the shafts. Both suns shine from above and behind the scene, towards the camera
/// (the shafts fan from above the frame), in the water's own colour; they are the same sun, so the shafts are what the
/// net's lines are in the air.
fn underwater(w: &lightcraft_develop::relight::WaterSettings, lights: &mut Vec<Light>, effects: &mut Vec<Effects>) -> (Water, Vec<(Light, Effects)>) {
    let water = Water { amount: w.amount / 100.0, clarity: w.clarity / 100.0, tint: w.tint / 100.0 };
    // (the sun's light takes the water's colour: blue ocean 215°, blue-green 190°, green lake 140°, at a gentle strength)
    let hue = if w.tint < 0.0 { 190.0 + 25.0 * (-w.tint / 100.0) } else { 190.0 - 50.0 * (w.tint / 100.0) };
    let sun = |amount: f64| Light {
        x: 0.5,
        y: 0.5,
        depth: 0.3,
        amount,
        softness: 0.5,
        warmth: 0.0,
        wrap: 0.0,
        kind: LightKind::Sun(Sun { azimuth: SUN.0, elevation: SUN.1 }),
    };
    let sky = Effects { gel_hue: hue, gel_strength: 0.25, pattern: Pattern::Caustics, ..Effects::default() };
    let mut beams = Vec::new();
    if w.caustics > 0.0 {
        lights.push(sun(CAUSTICS_AMOUNT * w.caustics / 100.0));
        effects.push(sky);
    }
    if w.shafts > 0.0 {
        let s = w.shafts / 100.0;
        beams.push((sun(0.3 + 0.6 * s), Effects { haze: 0.2 + 0.55 * s, scatter: 0.8, dust: 0.2, ..sky }));
    }
    (water, beams)
}

/// The water's sun: from behind the scene (azimuth 180°, so the camera looks towards it and the shafts fan from above the
/// frame) and this high (degrees).
const SUN: (f64, f64) = (180.0, 62.0);
/// The caustics' light at Caustics 100 (the light's Amount, 0..1).
const CAUSTICS_AMOUNT: f64 = 0.5;

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
        scatter: e.scatter / 100.0,
        dust: e.dust / 100.0,
        shadows: e.shadows / 100.0,
        open: e.pattern_open / 100.0,
        mist: e.mist / 100.0,
        mist_height: e.mist_height / 100.0,
        fade: e.fade / 100.0,
        source: e.source / 100.0,
        edge_glow: e.edge_glow / 100.0,
        sunburst: e.sunburst / 100.0,
    }
}

/// The type of light `k` names, with its numbers in the units the relight crate takes (issue #300).
fn kind_of(k: &KindSettings) -> LightKind {
    match k.index() {
        1 => LightKind::Spot(Spot {
            aim_x: k.spot_aim_x,
            aim_y: k.spot_aim_y,
            angle: k.spot_angle,
            edge: k.spot_edge / 100.0,
            sharp: k.spot_sharp >= 0.5,
        }),
        2 => LightKind::Sun(Sun { azimuth: k.sun_azimuth, elevation: k.sun_elevation }),
        3 => {
            LightKind::Softbox(Softbox { width: k.box_width, height: k.box_height, rotation: k.box_rotation, aim_h: k.box_aim_h, aim_v: k.box_aim_v })
        }
        4 => LightKind::Rim(Rim { width: k.rim_width / 100.0, angle: k.rim_angle, one_sided: k.rim_side / 100.0 }),
        5 => LightKind::Fill(Fill { near: k.fill_near / 100.0 }),
        _ => LightKind::Point,
    }
}

/// Where a light dragged to `(x, y)` goes when it is put on the nearest depth edge (issue #332): its place and depth in the
/// settings' units, and for a sun the direction it must shine from to come from there (azimuth, elevation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapped {
    pub x: f64,
    pub y: f64,
    /// 0..100.
    pub depth: f64,
    /// The light's Wrap, 0..100: a light behind the edge shines at the scene's back, so it keeps to a low Wrap (what faces the
    /// camera is not lit by it) unless the light already had a lower one.
    pub wrap: f64,
    pub sun: Option<(f64, f64)>,
}

/// The most Wrap a light put behind an edge keeps (of 100).
pub const SNAP_WRAP: f64 = 10.0;

/// `light` dragged to `(x, y)` on `field`: the place just behind the nearest depth edge ([`Field::snap_to_edge`]), or
/// `None` when no edge is near (the light stays where it was put). A sun also gets the direction towards that place
/// ([`Field::sun_towards`]), so its shafts fan out from the edge.
pub fn snap_to_edge(field: &Field, light: &lightcraft_develop::relight::LightSettings, x: f64, y: f64) -> Option<Snapped> {
    let s = field.snap_to_edge(x, y)?;
    let sun = (light.kind.index() == 2).then(|| {
        let (az, el) = field.sun_towards(s.x, s.y);
        (az.round(), el.round())
    });
    Some(Snapped { x: s.x, y: s.y, depth: (100.0 * s.depth).round(), wrap: light.wrap.min(SNAP_WRAP), sun })
}

/// `light`'s cone as the photo shows it, for the overlay (issue #346; `None`: no spot, or a cone the camera can't see).
pub fn spot_cone(field: &Field, light: &lightcraft_develop::relight::LightSettings) -> Option<lightcraft_relight::SpotCone> {
    let l = Light {
        x: light.x,
        y: light.y,
        depth: light.depth / 100.0,
        amount: light.amount / 100.0,
        softness: light.softness / 100.0,
        warmth: light.warmth / 100.0,
        wrap: light.wrap / 100.0,
        kind: kind_of(&light.kind),
    };
    field.spot_cone(&l)
}

/// The light of a render of `info` with settings `s` (`None`: off, or the photo has no field yet).
///
/// `draft`: a drag's render ([`Tier::Draft`]): lit on the field at half its size, its shadows, rays and patterns coarser.
pub fn lit(info: &SourceInfo, s: &DevelopSettings, draft: bool) -> Option<Lit> {
    lit_for(info, s, if draft { Tier::Draft } else { Tier::Full })
}

/// How a render takes its light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The release, the export and every render at rest: the whole field, full shadows and rays.
    Full,
    /// A drag's render: the field at half its size ([`Field::halved`]: every per-pixel stage a quarter of the cost) with
    /// coarse shadows, rays and patterns; what it makes is kept for the drag's next steps.
    Draft,
    /// A small picture (a preset's, a thumbnail) whose longest side is this: coarse as a drag's, on the half field when that
    /// still has twice the picture's pixels each way, and nothing it makes is kept, so the photo's own planes stay.
    Small(usize),
}

/// The longest side of a render that takes [`Tier::Small`] (preset pictures, thumbnails; never an export).
pub const SMALL_EDGE: usize = 320;

impl Tier {
    /// The tier of a render: `draft` a drag's, `long` its output's longest side, `export` an export's (always [`Tier::Full`]).
    pub fn of(draft: bool, long: usize, export: bool) -> Tier {
        if export {
            Tier::Full
        } else if long <= SMALL_EDGE {
            Tier::Small(long)
        } else if draft {
            Tier::Draft
        } else {
            Tier::Full
        }
    }
}

/// [`lit`] at tier `tier`.
pub fn lit_for(info: &SourceInfo, s: &DevelopSettings, tier: Tier) -> Option<Lit> {
    let full = info.relight.as_ref()?;
    let scene = scene_of(s)?;
    let halve = match tier {
        Tier::Full => false,
        Tier::Draft => true,
        Tier::Small(long) => full.field.width.max(full.field.height) / 2 >= 2 * long,
    };
    let half = if halve { halved(full) } else { None };
    let input = half.as_ref().unwrap_or(full);
    let (draft, keep) = (tier != Tier::Full, !matches!(tier, Tier::Small(_)));
    let ev = s.light.exposure;
    let (_, shading) = shading_in(input, &scene, ev, draft, keep);
    let water = water_for(input, &scene.water, ev, keep);
    let beams = beams_in(input, &scene, draft, keep);
    // (issue #330: a sun's rays in the sky, on the same field as the rest: a drag's are traced on the half-size field)
    let rays = if scene.skies.iter().any(crate::relight_sky::SkyLight::is_on) {
        let tone = crate::finish::base_tone(s, info, 1.0);
        let guide = crate::finish::base_tone(&DevelopSettings::default(), info, 1.0);
        let room = crate::relight_sky::Headroom::of(&tone, &guide, ev);
        crate::relight_sky::rays_of(input, &scene, &room)
    } else {
        Vec::new()
    };
    Some(Lit { shading, beams, fades: fades_in(input, &scene, ev, keep), rays, field: input.field.clone(), water })
}

/// `input`'s field at half its size, under a key of its own (so nothing made on it is taken for the whole field's), made
/// once per photo and kept; `None` for a field too small to halve.
fn halved(input: &Input) -> Option<Input> {
    let key = hashed(|h| std::hash::Hash::hash(&(10u8, input.key), h));
    if let Some(field) = HALVED.get(key) {
        return Some(Input { field, key });
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let field = Arc::new(input.field.halved()?);
    if let Some(t) = t {
        eprintln!("    relight halved ({} × {}): {:.1} ms", field.width, field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    HALVED.put(key, field.clone());
    Some(Input { field, key })
}

/// The water's colour planes on `input`'s field for a photo at exposure `ev`; made once and kept (`None`: no water).
fn water_for(input: &Input, water: &Water, ev: f64, keep: bool) -> Option<Arc<WaterPlanes>> {
    if !water.is_on() {
        return None;
    }
    let key =
        hashed(|h| std::hash::Hash::hash(&(6u8, input.key, ev.to_bits(), water.amount.to_bits(), water.clarity.to_bits(), water.tint.to_bits()), h));
    if let Some(p) = WATER.get(key) {
        return Some(p);
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let planes = Arc::new(input.field.water_planes(water, ev as f32)?);
    if let Some(t) = t {
        eprintln!("    relight water ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    WATER.put_if(keep, key, planes.clone());
    Some(planes)
}

pub(crate) fn hashed(parts: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts(&mut h);
    std::hash::Hasher::finish(&h)
}

/// The key naming `light`'s lobe on `input`'s field: its place, depth, Softness and Wrap and its pattern (not Amount,
/// Warmth, a gel, haze or Darken Surroundings, which the lobe doesn't depend on).
fn lobe_key(input: &Input, light: &Light, effects: &Effects, ev: f64, coarse: bool) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(0u8, input.key), h);
        for v in [light.x, light.y, light.depth, light.softness, light.wrap] {
            std::hash::Hash::hash(&v.to_bits(), h);
        }
        // (a point hashes as it did before light types: the same key, the same kept lobe)
        if light.kind != LightKind::Point {
            std::hash::Hash::hash(&light.kind.key_bits(), h);
        }
        // (a fill reads the luminance the photo shows, its exposure included)
        if matches!(light.kind, LightKind::Fill(_)) {
            std::hash::Hash::hash(&ev.to_bits(), h);
        }
        // (a light without shadows hashes as it did before them: the same key, the same kept lobe)
        if let Some(amount) = shadow_amount(light, effects) {
            std::hash::Hash::hash(&(b's', amount.to_bits()), h);
        }
        // (a coarse one, made for a drag, is another lobe)
        if coarse {
            std::hash::Hash::hash(&b'c', h);
        }
        // (a light without a pattern hashes as it did before patterns: the same key, the same kept lobe)
        if effects.pattern != Pattern::None {
            std::hash::Hash::hash(&effects.pattern, h);
            for v in [effects.scale, effects.rotation, effects.softness, effects.open] {
                std::hash::Hash::hash(&v.to_bits(), h);
            }
        }
    })
}

/// The key naming the shading of `scene` on `input`'s field for a photo at exposure `ev`: the field and every number the
/// shading depends on.
pub fn shading_key(input: &Input, scene: &Scene, ev: f64, draft: bool) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(1u8, input.key, ev.to_bits(), scene.darken.to_bits(), scene.lights.len()), h);
        for l in &scene.lights {
            for v in [l.x, l.y, l.depth, l.amount, l.softness, l.warmth, l.wrap] {
                std::hash::Hash::hash(&v.to_bits(), h);
            }
        }
        // each light's type and its numbers with the light's place in the list (a scene of points hashes as it did before types)
        for (i, l) in scene.lights.iter().enumerate().filter(|(_, l)| l.kind != LightKind::Point) {
            std::hash::Hash::hash(&(i, l.kind.key_bits()), h);
        }
        // each light's effects with the light's place in the list (a scene whose lights have none hashes as it did before
        // them; the same effect on another light is another scene)
        // (a drag's shading, whose shadows and patterns are coarse, is not the full one; a scene with neither is the same
        // for both)
        if draft && scene.drafts_differ() {
            std::hash::Hash::hash(&b'd', h);
        }
        // (not the air's numbers: the beams and the fade are not in the shading, issues #317 and #331; nor the source's, issue #334, nor the edge glow's, issue #332: points and glows of light added on top)
        let shaded = |e: &Effects| Effects {
            haze: 0.0,
            scatter: 0.5,
            dust: 0.0,
            mist: 0.0,
            mist_height: 0.3,
            fade: 0.0,
            source: 0.0,
            edge_glow: 0.0,
            sunburst: 0.0,
            ..*e
        };
        for (i, e) in scene.effects.iter().map(shaded).enumerate().filter(|(_, e)| *e != Effects::default()) {
            std::hash::Hash::hash(&(i, e.gel_hue.to_bits(), e.gel_strength.to_bits(), e.pattern, e.scale.to_bits()), h);
            std::hash::Hash::hash(&(e.rotation.to_bits(), e.softness.to_bits(), e.haze.to_bits(), e.shadows.to_bits(), e.open.to_bits()), h);
        }
    })
}

/// The shadow amount (0..1) `light` casts with `effects`: `None` for a light with none, or of a type that takes none (a rim,
/// a fill), which keeps its lobe as it was before shadows. A light with a pattern always casts full shadows (issue #316):
/// the photo's own things block the pattern the way they block real light through a window or leaves.
fn shadow_amount(light: &Light, effects: &Effects) -> Option<f64> {
    if !light.kind.takes_shadows() {
        return None;
    }
    if effects.pattern != Pattern::None {
        return Some(1.0);
    }
    (effects.shadows.is_finite() && effects.shadows > 0.0).then(|| effects.shadows.min(1.0))
}

/// The key naming `light`'s shadow visibility on `input`'s field: only what the march reads. A point and a spot: the
/// place, depth and Softness (the size); a sun: its direction and Softness, not a place; a softbox: the place, depth and
/// rectangle and aim, not Softness; a patterned lamp (`far`, marched from `Field::pattern_rig`) adds its Wrap and Softness. Not
/// Amount, a spot's cone or the shadow amount. `coarse`: the drag's march.
fn shadow_key(input: &Input, light: &Light, coarse: bool, far: bool) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(2u8, input.key, coarse), h);
        // (a patterned lamp's shadows are marched from its far source, `Field::pattern_rig`, which the middle of its pool
        // sets: Wrap moves the pool, and Softness its size, for every type)
        if far {
            std::hash::Hash::hash(&(b'f', light.wrap.to_bits(), light.softness.to_bits()), h);
        }
        let bits = |v: f64| v.to_bits();
        match light.kind {
            LightKind::Sun(s) => std::hash::Hash::hash(&(b'u', bits(s.azimuth), bits(s.elevation), bits(light.softness)), h),
            LightKind::Softbox(b) => {
                std::hash::Hash::hash(&(b'b', bits(light.x), bits(light.y), bits(light.depth), (bits(b.width), bits(b.height), bits(b.rotation))), h);
                // (an aimed box turns its rectangle, and a patterned one marches from along its aim, issue #335; an unaimed one
                // hashes as it did before)
                if b.aim_h != 0.0 || b.aim_v != 0.0 {
                    std::hash::Hash::hash(&(b'a', bits(b.aim_h), bits(b.aim_v)), h);
                }
            }
            _ => std::hash::Hash::hash(&(b'p', bits(light.x), bits(light.y), bits(light.depth), bits(light.softness)), h),
        }
    })
}

/// `light`'s shadow visibility on `input`'s field; made once and kept. A drag (`draft`) takes the full one when it is
/// kept (the other lights of the scene, a light that is not moving) and else makes a coarse one, so the full ones are
/// not made again at the release.
fn shadows_for(input: &Input, light: &Light, rig: &Rig, draft: bool, far: bool, keep: bool) -> Option<Arc<Visibility>> {
    let full = shadow_key(input, light, false, far);
    if let Some(v) = SHADOWS.get(full) {
        return Some(v);
    }
    let key = shadow_key(input, light, draft, far);
    if draft && let Some(v) = SHADOWS.get(key) {
        return Some(v);
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let vis = Arc::new(input.field.visibility(rig, light, draft)?);
    if let Some(t) = t {
        eprintln!("    relight shadows ({} × {}, draft {draft}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    SHADOWS.put_if(keep, key, vis.clone());
    Some(vis)
}

/// Whether `light` shows a beam with `effects` (issue #317): it has haze or mist (issue #331), and a place it shines from (not
/// a rim or a fill).
fn has_beam(light: &Light, effects: &Effects) -> bool {
    let some = |v: f64| v.is_finite() && v > 0.0;
    light.kind.has_place() && (some(effects.haze) || some(effects.mist))
}

/// The key naming the rays from the air towards `light` on `input`'s field (issue #317): what they and the air's light read,
/// the light's place, depth, Softness, Wrap (its peak) and type numbers. Not Amount, Warmth, a gel or the haze's numbers.
fn shafts_key(input: &Input, light: &Light, effects: &Effects, coarse: bool) -> u64 {
    hashed(|h| {
        // (a patterned lamp's rays go to its far source, issue #316, which the light's pool sets: its place and numbers
        // already here)
        std::hash::Hash::hash(&(3u8, input.key, coarse, effects.pattern != Pattern::None), h);
        for v in [light.x, light.y, light.depth, light.softness, light.wrap] {
            std::hash::Hash::hash(&v.to_bits(), h);
        }
        std::hash::Hash::hash(&light.kind.key_bits(), h);
    })
}

/// The key naming `light`'s beam: its rays' numbers, the haze's and the pattern's.
fn beam_key(input: &Input, light: &Light, effects: &Effects, coarse: bool) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(4u8, shafts_key(input, light, effects, coarse)), h);
        std::hash::Hash::hash(&(effects.haze.to_bits(), effects.scatter.to_bits(), effects.dust.to_bits()), h);
        // (the mist, issue #331: a beam with none hashes as it did before it, Mist Height not read)
        if effects.mist != 0.0 {
            std::hash::Hash::hash(&(b'm', effects.mist.to_bits(), effects.mist_height.to_bits()), h);
        }
        // (the light's pattern cuts its beam too)
        std::hash::Hash::hash(
            &(effects.pattern, effects.scale.to_bits(), effects.rotation.to_bits(), effects.softness.to_bits(), effects.open.to_bits()),
            h,
        );
    })
}

/// `light`'s beam through its haze on `input`'s field ([`Field::beam_plane`]; `None`: no beam). The rays are made once per
/// place of the light and kept, the beam once per set of the haze's numbers. A drag (`draft`) takes the full ones when they are
/// kept, else makes coarse ones.
fn beam_for(input: &Input, light: &Light, effects: &Effects, rig: &Rig, draft: bool, keep: bool) -> Option<(Arc<Lobe>, bool)> {
    if !has_beam(light, effects) {
        return None;
    }
    let plane_key = |coarse: bool| hashed(|h| std::hash::Hash::hash(&(5u8, beam_key(input, light, effects, coarse)), h));
    if let Some(l) = BEAM_LOBES.get(plane_key(false)) {
        return Some((l, false));
    }
    let t = crate::profiling().then(std::time::Instant::now);
    // (the full rays when kept, also for a drag: a drag of the Haze slider weighs them again and marches nothing)
    let full = SHAFTS.get(shafts_key(input, light, effects, false));
    let coarse = draft && full.is_none();
    if coarse && let Some(l) = BEAM_LOBES.get(plane_key(true)) {
        return Some((l, true));
    }
    let beam = match BEAMS.get(beam_key(input, light, effects, coarse)) {
        Some(b) => b,
        None => {
            let shafts = match full {
                Some(s) => s,
                None => match SHAFTS.get(shafts_key(input, light, effects, coarse)) {
                    Some(s) => s,
                    None => {
                        // (a patterned lamp shines from far along its direction, as its lobe and shadows do)
                        let far = input.field.pattern_rig(rig, effects);
                        let s = Arc::new(input.field.shafts(far.as_ref().unwrap_or(rig), light, coarse)?);
                        SHAFTS.put_if(keep, shafts_key(input, light, effects, coarse), s.clone());
                        s
                    }
                },
            };
            let b = Arc::new(input.field.beam(&shafts, rig, effects));
            BEAMS.put_if(keep, beam_key(input, light, effects, coarse), b.clone());
            b
        }
    };
    let plane = Arc::new(input.field.beam_plane(&beam));
    if let Some(t) = t {
        eprintln!("    relight beam ({} × {}, draft {coarse}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    BEAM_LOBES.put_if(keep, plane_key(coarse), plane.clone());
    Some((plane, coarse))
}

/// The key naming `light`'s source glow on `input`'s field (issue #334): the place it is drawn at (x, y and depth: the depth
/// hides it behind nearer things) and its strength. Not Amount, Warmth or a gel (the colour is added later), nor the
/// light's type numbers, Softness or the haze's.
fn source_key(input: &Input, light: &Light, effects: &Effects) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(7u8, input.key, effects.source.to_bits()), h);
        for v in [light.x, light.y, light.depth] {
            std::hash::Hash::hash(&v.to_bits(), h);
        }
    })
}

/// Whether `light` shows a glow past an edge with `effects` (issue #332): it has a place, and Edge Glow or Sunburst.
fn has_glow(light: &Light, effects: &Effects) -> bool {
    let on = |v: f64| v.is_finite() && v > 0.0;
    light.kind.has_place() && (on(effects.edge_glow) || on(effects.sunburst))
}

/// The key naming `light`'s glow on `input`'s field: what the glow reads (its place, depth and Softness, and its two numbers),
/// not Amount, Warmth, a gel, the haze or the light's type.
fn glow_key(input: &Input, light: &Light, effects: &Effects) -> u64 {
    hashed(|h| {
        std::hash::Hash::hash(&(8u8, input.key), h);
        for v in [light.x, light.y, light.depth, light.softness, effects.edge_glow, effects.sunburst] {
            std::hash::Hash::hash(&v.to_bits(), h);
        }
    })
}

/// `light`'s source seen as a bright point (issue #334; `None`: it shows none), made once per place and strength and kept.
fn source_for(input: &Input, light: &Light, effects: &Effects, rig: &Rig, keep: bool) -> Option<Arc<Lobe>> {
    if !light.kind.shows_source() || !(effects.source.is_finite() && effects.source > 0.0) {
        return None;
    }
    let key = source_key(input, light, effects);
    if let Some(l) = BEAM_LOBES.get(key) {
        return Some(l);
    }
    let plane = Arc::new(input.field.source(light, rig, effects.source)?);
    BEAM_LOBES.put_if(keep, key, plane.clone());
    Some(plane)
}

/// `light`'s glow past an edge on `input`'s field ([`Field::edge_glow`]; `None`: none), made once per set of its numbers and kept.
fn glow_for(input: &Input, light: &Light, effects: &Effects, keep: bool) -> Option<Arc<Lobe>> {
    if !has_glow(light, effects) {
        return None;
    }
    let key = glow_key(input, light, effects);
    if let Some(l) = GLOW_LOBES.get(key) {
        return Some(l);
    }
    let t = crate::profiling().then(std::time::Instant::now);
    let glow = Arc::new(input.field.edge_glow(light, effects)?);
    if let Some(t) = t {
        eprintln!("    relight glow ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    GLOW_LOBES.put_if(keep, key, glow.clone());
    Some(glow)
}

/// The scene's added light (issues #317, #332): each light with haze or a glow past an edge, its plane and the light it adds per unit of it, in its own colour
/// (Warmth and gel, as the finish stage's temperature and tint colour a grey of the same luminance) and strength; and (issue
/// #334) each light whose source shows as a point, as a plane of its own.
pub fn beams_of(input: &Input, scene: &Scene, draft: bool) -> Vec<Beamed> {
    beams_in(input, scene, draft, true)
}

/// [`beams_of`], keeping what it makes when `keep`.
fn beams_in(input: &Input, scene: &Scene, draft: bool, keep: bool) -> Vec<Beamed> {
    let none = Effects::default();
    let mut out = Vec::new();
    for (i, l) in scene.lights.iter().take(MAX_LIGHTS).enumerate() {
        let e = scene.effects.get(i).unwrap_or(&none);
        let shows_source = l.kind.shows_source() && e.source.is_finite() && e.source > 0.0;
        if !has_beam(l, e) && !shows_source && !has_glow(l, e) {
            continue;
        }
        // (the rig's place and peak don't depend on Darken Surroundings or the exposure: a light with a place is no fill)
        let rig = input.field.rig(l, 0.0, 0.0);
        let (t, s) = e.light_colour(l.warmth);
        let c = [1.0 + 0.3 * t, 1.0 - 0.22 * s, (1.0 - 0.3 * t).max(0.0)];
        let lum = (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6);
        let k = rig.gain() * crate::tone::GREY / lum;
        let colour = c.map(|v| if (v * k).is_finite() { (v * k).max(0.0) } else { 0.0 });
        // (issue #332: the glow past an edge is added into the beam's plane, the same light in the same colour)
        let beam = beam_for(input, l, e, &rig, draft, keep);
        let glow = glow_for(input, l, e, keep);
        let plane = match (beam, glow) {
            (Some((b, coarse)), Some(g)) => {
                // (both: the sum, kept while neither changes; the beam's key takes the flag the beam was made with, which is a drag's
                // only when its rays were not kept in full)
                let key = hashed(|h| std::hash::Hash::hash(&(9u8, beam_key(input, l, e, coarse), glow_key(input, l, e)), h));
                Some(ADDED_LOBES.get(key).unwrap_or_else(|| {
                    let sum = Arc::new(b.added(&g));
                    ADDED_LOBES.put_if(keep, key, sum.clone());
                    sum
                }))
            }
            (Some((p, _)), None) | (None, Some(p)) => Some(p),
            (None, None) => None,
        };
        if let Some(plane) = plane {
            out.push(Beamed { plane, colour, slot: i as u32, shafts: false });
        }
        if let Some(plane) = source_for(input, l, e, &rig, keep) {
            out.push(Beamed { plane, colour, slot: (MAX_LIGHTS + i) as u32, shafts: false });
        }
    }
    // (issue #333: the water's shafts, a light that only shows its beam; no limit: the water has one)
    for (l, e) in &scene.beams {
        if !has_beam(l, e) {
            continue;
        }
        let rig = input.field.rig(l, 0.0, 0.0);
        let (t, s) = e.light_colour(l.warmth);
        let c = [1.0 + 0.3 * t, 1.0 - 0.22 * s, (1.0 - 0.3 * t).max(0.0)];
        let lum = (0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2]).max(1e-6);
        let k = rig.gain() * crate::tone::GREY / lum;
        let colour = c.map(|v| if (v * k).is_finite() { (v * k).max(0.0) } else { 0.0 });
        if let Some((plane, _)) = beam_for(input, l, e, &rig, draft, keep) {
            out.push(Beamed { plane, colour, slot: 0, shafts: true });
        }
    }
    out
}

/// The key naming a Distance Fade on `input`'s field: the air's numbers it reads (Haze, Mist, Mist Height, Distance Fade).
fn fade_key(input: &Input, e: &Effects) -> u64 {
    hashed(|h| std::hash::Hash::hash(&(6u8, input.key, [e.haze, e.mist, e.mist_height, e.fade].map(f64::to_bits)), h))
}

/// The scene's Distance Fade (issue #331): the fog is the photo's air, not a light's, so there is one per photo, at most. It
/// takes the strongest Distance Fade among the lights (the first of equals) with that light's Haze, Mist and Mist Height; its
/// plane is made once per set of those numbers and kept (the lights' places, types and a drag don't change it). The air's colour
/// at exposure `ev` is the photo's far colour with some of the summed colour (Warmth and gel) of every light with Distance Fade
/// on, a rim or a fill as much as any other. One light gives what it gave alone.
pub fn fades_of(input: &Input, scene: &Scene, ev: f64) -> Vec<Faded> {
    fades_in(input, scene, ev, true)
}

/// [`fades_of`], keeping what it makes when `keep`.
fn fades_in(input: &Input, scene: &Scene, ev: f64, keep: bool) -> Vec<Faded> {
    let none = Effects::default();
    let on = |e: &Effects| e.fade.is_finite() && e.fade > 0.0;
    let faded: Vec<(&Light, &Effects)> =
        scene.lights.iter().take(MAX_LIGHTS).enumerate().map(|(i, l)| (l, scene.effects.get(i).unwrap_or(&none))).filter(|(_, e)| on(e)).collect();
    // (the strongest: the first of equals, so one light, or two alike, give the first one's air)
    let Some((_, e)) = faded.iter().copied().reduce(|a, b| if b.1.fade > a.1.fade { b } else { a }) else { return Vec::new() };
    let key = fade_key(input, e);
    let plane = match FADES.get(key) {
        Some(p) => p,
        None => {
            let Some(p) = input.field.fade(e) else { return Vec::new() };
            let p = Arc::new(p);
            FADES.put_if(keep, key, p.clone());
            p
        }
    };
    let colour = |(l, e): &(&Light, &Effects)| {
        let (t, s) = e.light_colour(l.warmth);
        [1.0 + 0.3 * t, 1.0 - 0.22 * s, (1.0 - 0.3 * t).max(0.0)]
    };
    let mut light = faded.first().map_or([1.0; 3], colour);
    for c in faded.iter().skip(1).map(colour) {
        light = [light[0] + c[0], light[1] + c[1], light[2] + c[2]];
    }
    let gain = if ev.is_finite() { 2f32.powf(ev.clamp(-10.0, 10.0) as f32) } else { 1.0 };
    let colour = input.field.air_colour(light).map(|c| c * gain);
    vec![Faded { plane, colour }]
}

/// `light`'s lobes on `input`'s field (`rig`: the light set up on it), seen through its pattern and with the shadows it
/// casts; made once and kept. A drag (`draft`) takes the full lobes when they are kept and else makes them with coarse
/// shadows and a coarse pattern (a light with neither has one set for both).
fn lobe_for(input: &Input, light: &Light, effects: &Effects, rig: &Rig, ev: f64, draft: bool, keep: bool) -> Arc<Lobes> {
    let coarse = draft && (shadow_amount(light, effects).is_some() || effects.pattern != Pattern::None);
    if let Some(l) = LOBES.get(lobe_key(input, light, effects, ev, false)) {
        return l;
    }
    let key = lobe_key(input, light, effects, ev, coarse);
    if coarse && let Some(l) = LOBES.get(key) {
        return l;
    }
    let t = crate::profiling().then(std::time::Instant::now);
    // (a patterned lamp shines from far along its direction, like daylight through a window; the pattern's coordinates
    // still come from the light as placed)
    let far = input.field.pattern_rig(rig, effects);
    let shines = far.as_ref().unwrap_or(rig);
    let (mut lobe, mut through) = input.field.patterned(rig, input.field.lobe(shines), effects, coarse);
    // (the shadows multiply the lobes before the lights add up)
    if let Some(amount) = shadow_amount(light, effects)
        && let Some(vis) = shadows_for(input, light, shines, coarse, far.is_some(), keep)
    {
        lobe = input.field.shadowed(lobe, &vis, amount);
        through = through.map(|t| input.field.shadowed(t, &vis, amount));
    }
    let lobe = Arc::new(Lobes { light: lobe, through });
    if let Some(t) = t {
        eprintln!("    relight lobe ({} × {}): {:.1} ms", input.field.width, input.field.height, t.elapsed().as_secs_f64() * 1e3);
    }
    LOBES.put_if(keep, key, lobe.clone());
    lobe
}

/// The shading of `scene` on `input`'s field for a photo at exposure `ev`, and the key naming it. Made once per set of
/// lights and kept, from one lobe per light that is made once and kept too: the render ([`lit`]) and the ⌥-drag preview
/// (`fork/relight.md`) take the same one, and dragging one light makes one lobe and one shading, not one per light.
///
/// `draft`: a slider drag's render: the shadows are marched coarser (a quarter of the cost; a light whose full shadows are
/// kept uses those), and the release makes the full ones. The export is never a draft.
pub fn shading_for(input: &Input, scene: &Scene, ev: f64, draft: bool) -> (u64, Arc<Shading>) {
    shading_in(input, scene, ev, draft, true)
}

/// [`shading_for`], keeping what it makes when `keep`.
fn shading_in(input: &Input, scene: &Scene, ev: f64, draft: bool, keep: bool) -> (u64, Arc<Shading>) {
    let key = shading_key(input, scene, ev, draft);
    let _making = match MEMO.claim(key) {
        Claim::Kept(sh) => return (key, sh),
        Claim::Make(m) => m,
    };
    let t = crate::profiling().then(std::time::Instant::now);
    let rigs: Vec<Rig> = scene.lights.iter().take(MAX_SCENE).map(|l| input.field.rig(l, scene.darken, ev)).collect();
    let none = Effects::default();
    let effects = |i: usize| scene.effects.get(i).unwrap_or(&none);
    let lobes: Vec<Arc<Lobes>> =
        scene.lights.iter().zip(&rigs).enumerate().map(|(i, (l, r))| lobe_for(input, l, effects(i), r, ev, draft, keep)).collect();
    let mut parts: Vec<Part<'_>> = Vec::with_capacity(2 * rigs.len());
    for (i, ((l, r), lobe)) in scene.lights.iter().zip(&rigs).zip(&lobes).enumerate() {
        let (tilt, tint) = effects(i).light_colour(l.warmth);
        parts.push(Part { lobe: &lobe.light, gain: r.gain(), tilt, tint });
        // (what a pattern's material passes: the same light, in the pattern's own tint)
        if let Some(through) = &lobe.through {
            let (tilt, tint) = effects(i).through_colour(l.warmth);
            parts.push(Part { lobe: through, gain: r.gain(), tilt, tint });
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
    MEMO.put_if(keep, key, shading.clone());
    (key, shading)
}

/// One of the light's planes as a mask the finish stage applies, with `adjust` per unit of it.
pub(crate) fn evaluated(id: u32, width: usize, height: usize, data: Vec<f32>, adjust: LocalAdjustments) -> Evaluated {
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
    // (issue #317: each light's beam, light added on top in its own colour)
    for b in &lit.beams {
        let mut plane = lit.field.plane_at(&b.plane, w, h, &|x, y| {
            let q = map.apply(Point::new(x, y));
            (q.x, q.y)
        });
        let top = plane.par_iter().copied().reduce(|| 0f32, f32::max);
        if top <= 1e-6 || !top.is_finite() {
            continue;
        }
        // (in place: a second full-size plane per beam is 96 MB more at 24 MP; the mask keeps only the non-zero part of it, so a
        // source's small glow stays small)
        plane.par_iter_mut().for_each(|v| *v = (*v / top).clamp(0.0, 1.0));
        let alpha = plane;
        let added = b.colour.map(|c| f64::from(c * top));
        let id = if b.shafts { SHAFT_ID } else { BEAM_IDS - b.slot };
        out.push(evaluated(id, w, h, alpha, LocalAdjustments { fork_added: added, ..LocalAdjustments::default() }));
    }
    // (issue #331: the photo's Distance Fade: the photo's light dimmed by the air in front of it, an exposure, and the air's own
    // glow in its colour, light added on top; so a pixel becomes `c · T + air · (1 − T)`, the beams on top of that)
    for (k, f) in lit.fades.iter().enumerate() {
        let tau = lit.field.plane_at(&f.plane, w, h, &|x, y| {
            let q = map.apply(Point::new(x, y));
            (q.x, q.y)
        });
        let top = tau.par_iter().copied().reduce(|| 0f32, f32::max);
        if top <= 1e-6 || !top.is_finite() {
            continue;
        }
        let dim = tau.par_iter().map(|v| (v / top).clamp(0.0, 1.0)).collect();
        let glow_top = 1.0 - (-top).exp();
        let glow = tau.par_iter().map(|v| ((1.0 - (-v.max(0.0)).exp()) / glow_top).clamp(0.0, 1.0)).collect();
        let id = FADE_IDS - 2 * k as u32;
        out.push(evaluated(id, w, h, dim, LocalAdjustments { exposure: -f64::from(top) / std::f64::consts::LN_2, ..LocalAdjustments::default() }));
        let added = f.colour.map(|c| f64::from(c * glow_top));
        out.push(evaluated(id - 1, w, h, glow, LocalAdjustments { fork_added: added, ..LocalAdjustments::default() }));
    }
    // (issue #333: the water's colour by distance)
    if let Some(water) = &lit.water {
        out.extend(water_masks(&lit.field, water, w, h, &|x, y| {
            let q = map.apply(Point::new(x, y));
            (q.x, q.y)
        }));
    }
    // (issue #330: each sun's rays between clouds)
    out.extend(crate::relight_sky::masks(&lit.field, &lit.rays, w, h, &|x, y| {
        let q = map.apply(Point::new(x, y));
        (q.x, q.y)
    }));
    if let Some(t) = t {
        eprintln!("    relight planes ({w} × {h}): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    out
}

/// A plane's values over the render's pixels as the planes of the finish stage's terms: the strongest value `top` (of the sign
/// asked for) is the term, the plane is each pixel's share of it. `None` while the plane holds no such value.
fn share(values: &[f32], sign: f32) -> Option<(Vec<f32>, f64)> {
    // (on every core: the water's eight planes are each a full-size render plane, issue #374)
    let top = values.par_iter().map(|v| v * sign).filter(|v| v.is_finite()).reduce(|| 0f32, f32::max);
    if top <= 1e-6 {
        return None;
    }
    Some((values.par_iter().map(|v| (v * sign / top).clamp(0.0, 1.0)).collect(), f64::from(top)))
}

/// The water's planes (issue #333) as masks: the exposure and the temperature and tint (the finish stage's own, which together
/// are one gain per colour), and the veil of the water's light, added on top per colour. See [`lightcraft_relight::water`].
fn water_masks(field: &Field, p: &WaterPlanes, w: usize, h: usize, to_photo: &(dyn Fn(f64, f64) -> (f64, f64) + Sync)) -> Vec<Evaluated> {
    let at = |l: &Lobe| field.plane_at(l, w, h, to_photo);
    let mut out = Vec::new();
    let mut id = WATER_IDS;
    let mut push = |plane: Option<(Vec<f32>, f64)>, adjust: &dyn Fn(f64) -> LocalAdjustments| {
        id -= 1;
        if let Some((alpha, top)) = plane {
            out.push(evaluated(id, w, h, alpha, adjust(top)));
        }
    };
    // (a gain below one: the exposure is a stop count under zero)
    push(share(&at(&p.stops), -1.0), &|top| LocalAdjustments { exposure: -top, ..LocalAdjustments::default() });
    // (red against blue: the finish stage's temperature, negative towards blue; green against the two: its tint, negative towards green)
    let temp = at(&p.temp);
    push(share(&temp, -1.0), &|top| LocalAdjustments { temp: -100.0 * top, ..LocalAdjustments::default() });
    push(share(&temp, 1.0), &|top| LocalAdjustments { temp: 100.0 * top, ..LocalAdjustments::default() });
    let tint = at(&p.tint);
    push(share(&tint, -1.0), &|top| LocalAdjustments { tint: -100.0 * top, ..LocalAdjustments::default() });
    push(share(&tint, 1.0), &|top| LocalAdjustments { tint: 100.0 * top, ..LocalAdjustments::default() });
    // (the water's light, one plane per colour: far things take its colour by how far each colour has been absorbed)
    for c in 0..3 {
        let veil = at(&p.veil[c]);
        push(share(&veil, 1.0), &|top| {
            let mut added = [0.0; 3];
            added[c] = top;
            LocalAdjustments { fork_added: added, ..LocalAdjustments::default() }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_is_the_most_recently_used_so_a_drag_keeps_the_other_lights() {
        // eight lights, then one of them moved at each step: the seven others are asked for every step
        let memo: Memo<u8> = Memo::new(12);
        (0..8u64).for_each(|k| memo.put(k, Arc::new(0)));
        for step in 0..9u64 {
            (1..8u64).for_each(|k| assert!(memo.get(k).is_some(), "light {k} was made again at step {step}"));
            memo.put(100 + step, Arc::new(0));
        }
        memo.clear();
        assert!(memo.get(1).is_none());
    }

    #[test]
    fn a_value_being_made_is_waited_for_and_a_dropped_claim_frees_its_key() {
        // issue #374: two renders asking for the same value make it once; one that keeps nothing lets the other make it
        let memo: Arc<Memo<u8>> = Arc::new(Memo::new(4));
        let waiter = |memo: &Arc<Memo<u8>>| {
            let (memo, (tx, rx)) = (memo.clone(), std::sync::mpsc::channel());
            std::thread::spawn(move || {
                let got = match memo.claim(7) {
                    Claim::Kept(v) => Some(*v),
                    Claim::Make(_) => None,
                };
                let _ = tx.send(got);
            });
            rx
        };
        let Claim::Make(making) = memo.claim(7) else { panic!("nothing is kept yet") };
        let rx = waiter(&memo);
        memo.put(7, Arc::new(42));
        drop(making);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(30)), Ok(Some(42)), "the second takes what the first made");
        let Claim::Make(making) = memo.claim(8) else { panic!("nothing is kept for 8") };
        let (memo8, (tx, rx)) = (memo.clone(), std::sync::mpsc::channel());
        std::thread::spawn(move || {
            let made = matches!(memo8.claim(8), Claim::Make(_));
            let _ = tx.send(made);
        });
        drop(making);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(30)), Ok(true), "a claim dropped without a value frees the key");
    }

    #[test]
    fn a_patterned_lights_shadows_follow_its_wrap() {
        // a patterned lamp's shadows are marched from its far source, which the middle of its pool sets, and Wrap moves
        // the pool: another Wrap is another shadow map (the kept one used to be reused)
        let (w, h) = (160, 100);
        let lz = (0..w * h).map(|i| if (60..100).contains(&(i % w)) && (30..80).contains(&(i / w)) { -0.6 } else { 0.0 }).collect();
        let lum = (0..w * h).map(|i| if i % w < w / 2 { -1.0 } else { -3.0 }).collect();
        let field = Arc::new(Field::from_parts(w, h, lz, lum).unwrap());
        let input = Input { field: field.clone(), key: 316_101 };
        let effects = Effects { pattern: Pattern::Blinds, ..Effects::default() };
        let lamp = |wrap: f64| Light { x: 0.2, y: 0.3, depth: 0.1, amount: 0.8, softness: 0.4, warmth: 0.0, wrap, kind: LightKind::Point };
        let marched = |l: &Light| {
            let far = field.pattern_rig(&field.rig(l, 0.0, 0.0), &effects).unwrap();
            let kept = shadows_for(&input, l, &far, false, true, true).unwrap();
            let fresh = field.visibility(&far, l, false).unwrap();
            (field.visibility_plane(&kept), field.visibility_plane(&fresh))
        };
        let (a, _) = marched(&lamp(0.0));
        let (b, b_fresh) = marched(&lamp(1.0));
        assert!(a != b_fresh, "Wrap doesn't move the far source here: the test shows nothing");
        assert!(b == b_fresh, "the shadows of Wrap 0 were kept for Wrap 1");
    }

    #[test]
    fn a_softboxs_shadows_follow_its_aim() {
        // (issue #335) the shadow key holds the aim: another aim is another key, and a patterned softbox, which marches its shadows
        // from a far source along its aim, gets another shadow map (the kept one of the other aim used to be reused)
        let (w, h) = (160, 100);
        let lz = (0..w * h).map(|i| if (60..100).contains(&(i % w)) && (30..80).contains(&(i / w)) { -0.6 } else { 0.0 }).collect();
        let lum = (0..w * h).map(|i| if i % w < w / 2 { -1.0 } else { -3.0 }).collect();
        let field = Arc::new(Field::from_parts(w, h, lz, lum).unwrap());
        let input = Input { field: field.clone(), key: 335_101 };
        let effects = Effects { pattern: Pattern::Blinds, ..Effects::default() };
        let boxed = |aim_h: f64| Light {
            x: 0.2,
            y: 0.3,
            depth: 0.1,
            amount: 0.8,
            softness: 0.4,
            warmth: 0.0,
            wrap: 0.4,
            kind: LightKind::Softbox(Softbox { width: 0.2, height: 0.2, rotation: 0.0, aim_h, aim_v: 0.0 }),
        };
        for far in [false, true] {
            assert_ne!(shadow_key(&input, &boxed(0.0), false, far), shadow_key(&input, &boxed(50.0), false, far));
        }
        let marched = |l: &Light| {
            let far = field.pattern_rig(&field.rig(l, 0.0, 0.0), &effects).unwrap();
            let kept = shadows_for(&input, l, &far, false, true, true).unwrap();
            let fresh = field.visibility(&far, l, false).unwrap();
            (field.visibility_plane(&kept), field.visibility_plane(&fresh))
        };
        let (a, _) = marched(&boxed(0.0));
        let (b, b_fresh) = marched(&boxed(50.0));
        assert!(a != b_fresh, "the aim doesn't move the far source here: the test shows nothing");
        assert!(b == b_fresh, "the shadows of the straight-in softbox were kept for the aimed one");
    }
}
