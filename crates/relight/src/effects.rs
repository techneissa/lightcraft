//! A light's effects (issue #302, `fork/relight.md`): a gel, a pattern and haze. All of it is original work, drawn
//! in code from numbers; no file is read.
//!
//! - **Gel:** a colour for the light, any hue and a strength. It joins the light's Warmth: [`Effects::light_colour`]
//!   multiplies the two colours (scene-linear, the way a gel in front of a warm lamp does) and gives the finish
//!   stage's own local temperature and tint, so a gel travels the same planes Warmth does, on the CPU and the GPU
//!   alike (the finish stage has one kernel for both).
//! - **Pattern:** the light shines through blinds, window panes, leaves or dapple (issue #316: [`crate::projector`]
//!   says where it lands, inside which opening and how blurred; [`crate::pattern`] draws it), laid onto the surfaces
//!   like a projector's slide ([`Field::pattern_planes`]), so the stripes run straight over a wall, converge over a
//!   floor and jump where depth steps. The pattern multiplies the light's lobe, so the facing term, the roll-off and
//!   the colour work as before; what the pattern's own material passes (a leaf, the slats' bounce) is a second lobe of
//!   the same light with a tint of its own ([`Effects::through_colour`]).
//! - **Haze:** the air glows where the light reaches it, up to each pixel's own depth and cut into shafts by what blocks
//!   the light, brightest looking into the light (issue #317, [`crate::beam`]); the pattern cuts the air too
//!   ([`AirPattern`]). The glow is light added on top in the light's own colour.

use rayon::prelude::*;

use crate::field::Field;
use crate::kinds::Shape;
use crate::light::{Lobe, Rig, warmth_tilt};
use crate::projector::Projector;

/// What a pattern is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    /// No pattern: the light is as the lobe made it.
    None,
    /// Horizontal slats.
    Blinds,
    /// A grid of panes with bars between.
    Panes,
    /// Leaf shadows with gaps of light.
    Leaves,
    /// Dapple: small spots of light in shade.
    Dapple,
    /// Light through moving water (issue #333): a net of bright lines on what faces up, shafts in the air. Made by the
    /// water's own sun, never a control's choice ([`Pattern::from_index`] does not give it).
    Caustics,
}

impl Pattern {
    /// The pattern a control value names (0 none, 1 blinds, 2 panes, 3 leaves, 4 dapple; not a number: none).
    pub fn from_index(v: f64) -> Pattern {
        if !v.is_finite() {
            return Pattern::None;
        }
        match v.round() as i64 {
            1 => Pattern::Blinds,
            2 => Pattern::Panes,
            3 => Pattern::Leaves,
            4 => Pattern::Dapple,
            _ => Pattern::None,
        }
    }
}

/// A light's effects, as plain numbers (the develop settings convert to this); every number is kept in range where
/// it is used, a number that is not a number takes its default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    /// The gel's hue, degrees (0 red, 120 green, 240 blue).
    pub gel_hue: f64,
    /// The gel's strength, 0 (no gel) .. 1.
    pub gel_strength: f64,
    pub pattern: Pattern,
    /// The pattern's size, 0 (fine) .. 1 (large).
    pub scale: f64,
    /// The pattern's turn, degrees.
    pub rotation: f64,
    /// How soft the pattern's edges are, 0 .. 1.
    pub softness: f64,
    /// The haze's amount, 0 .. 1.
    pub haze: f64,
    /// How much the haze scatters onward rather than every way alike (issue #317), 0 .. 1 (the phase function's `g` from 0 to
    /// [`crate::beam::G_MAX`]).
    pub scatter: f64,
    /// How much the haze's density varies through the scene (issue #317), 0 (even) .. 1.
    pub dust: f64,
    /// How much of the light the photo's own things block (issue #315, [`Field::shadowed`]), 0 (none: the light as the
    /// lobe made it) .. 1 (a surface behind something nearer gets none of it).
    pub shadows: f64,
    /// How open the pattern is (issue #316), 0 .. 1: how far the blinds' slats are tilted open, how thin a window's
    /// mullions are, how small the leaves are, how many gaps a canopy has.
    pub open: f64,
    /// Low mist (issue #331, [`crate::mist`]): how thick the air is near the ground, 0 (none) .. 1.
    pub mist: f64,
    /// How high the mist reaches, 0 (a thin layer on the ground) .. 1 (above the camera).
    pub mist_height: f64,
    /// Distance Fade (issue #331): how much far things fade into the air, 0 (none) .. 1 (as the air's density says).
    pub fade: f64,
    /// How bright and big the light's own source shows as a point when it sits inside the photo (issue #334,
    /// [`Field::source`]), 0 (not at all) .. 1.
    pub source: f64,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            gel_hue: 0.0,
            gel_strength: 0.0,
            pattern: Pattern::None,
            scale: 0.5,
            rotation: 0.0,
            softness: 0.3,
            haze: 0.0,
            scatter: 0.5,
            dust: 0.0,
            shadows: 0.0,
            open: 0.5,
            mist: 0.0,
            mist_height: 0.3,
            fade: 0.0,
            source: 0.0,
        }
    }
}

fn unit(v: f64, d: f64) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { d as f32 }
}

/// The least a gel lets through of any channel (a real gel is never one pure colour; it keeps the maths finite).
const GEL_FLOOR: f32 = 0.12;

/// How much of the gel's colour full strength gives (the exponent of [`hue_colour`] at strength 1).
const GEL_REACH: f32 = 0.8;

/// The gel's colour at full strength: the hue at full chroma, as the light's multiplier in scene-linear light.
fn hue_colour(hue: f64) -> [f32; 3] {
    let h = if hue.is_finite() { hue.rem_euclid(360.0) as f32 / 60.0 } else { 0.0 };
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let c = match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    };
    c.map(|v| v.max(GEL_FLOOR))
}

impl Effects {
    /// Whether the light has a gel.
    pub fn has_gel(&self) -> bool {
        unit(self.gel_strength, 0.0) > 0.0
    }

    /// The light's colour for Warmth `warmth` (−1..1) and this gel, as the red–blue tilt and the green–magenta tint
    /// the finish stage's local temperature and tint apply (`(1 + 0.3 t, 1 − 0.22 s, 1 − 0.3 t)`, luminance kept).
    /// Without a gel it is [`warmth_tilt`] and no tint, exactly.
    ///
    /// The gel is `hue_colour ^ (0.8 · strength^1.6)` per channel (no change at 0, a strong colour at 1, a gentle one
    /// at half), multiplied with Warmth's colour.
    pub fn light_colour(&self, warmth: f64) -> (f32, f32) {
        let tilt = warmth_tilt(warmth);
        let k = unit(self.gel_strength, 0.0);
        if k <= 0.0 {
            return (tilt, 0.0);
        }
        // (full strength is not the pure colour: a gel in front of a lamp over pale walls is strong at 0.8 of it)
        let gel = hue_colour(self.gel_hue).map(|c| c.powf(GEL_REACH * k.powf(1.6)));
        let warm = [(1.0 + 0.3 * tilt).max(0.05), 1.0, (1.0 - 0.3 * tilt).max(0.05)];
        let [r, g, b] = [gel[0] * warm[0], gel[1] * warm[1], gel[2] * warm[2]];
        let (sum, g2) = ((r + b).max(1e-6), 2.0 * g);
        // (the finish stage keeps r + b = 2: the red–blue split is the tilt, the green's share the tint)
        let temp = ((r - b) / sum / 0.3).clamp(-3.3, 3.3);
        let tint = ((1.0 - g2 / sum) / 0.22).clamp(-60.0, 4.5);
        (if temp.is_finite() { temp } else { 0.0 }, if tint.is_finite() { tint } else { 0.0 })
    }

    /// The light the pattern's own material passes (a leaf, a slat's bounce, a window's fill) for Warmth `warmth`: the
    /// light's colour ([`Effects::light_colour`]) tinted green-yellow through leaves and a canopy, towards the sky's blue
    /// in a window's shade.
    pub fn through_colour(&self, warmth: f64) -> (f32, f32) {
        let (tilt, tint) = self.light_colour(warmth);
        let (dt, ds) = match self.pattern {
            Pattern::Leaves | Pattern::Dapple => LEAF_TINT,
            Pattern::Blinds | Pattern::Panes => SKY_TINT,
            Pattern::None | Pattern::Caustics => (0.0, 0.0),
        };
        (tilt + dt, tint + ds)
    }
}

/// The colour leaves pass, as the finish stage's tilt and tint (warmer and greener: green-yellow).
const LEAF_TINT: (f32, f32) = (0.4, -0.9);
/// The colour of a window's shade (the sky's fill): cooler.
const SKY_TINT: (f32, f32) = (-0.7, 0.0);

/// A light's pattern over the field: the share of the light that passes straight through (`light`) and the share its
/// material passes, tinted ([`Effects::through_colour`]), one value each per field pixel.
#[derive(Clone, Debug)]
pub struct PatternPlanes {
    pub light: Vec<f32>,
    pub through: Vec<f32>,
}

/// `f` at every field pixel of a `w × h` field, or (`coarse`, a drag's) at every second pixel each way and brought to
/// the field's size bilinearly.
fn over_field(w: usize, h: usize, coarse: bool, f: impl Fn(usize, usize) -> (f32, f32) + Sync) -> (Vec<f32>, Vec<f32>) {
    let step = if coarse && w >= 8 && h >= 8 { 2 } else { 1 };
    let (gw, gh) = (w.div_ceil(step).max(1), h.div_ceil(step).max(1));
    let mut grid = vec![(0f32, 0f32); gw * gh];
    grid.par_chunks_mut(gw).enumerate().for_each(|(gy, row)| {
        for (gx, out) in row.iter_mut().enumerate() {
            *out = f((gx * step).min(w.saturating_sub(1)), (gy * step).min(h.saturating_sub(1)));
        }
    });
    if step == 1 {
        return grid.into_iter().unzip();
    }
    let (mut a, mut b) = (vec![0f32; w * h], vec![0f32; w * h]);
    a.par_chunks_mut(w.max(1)).zip(b.par_chunks_mut(w.max(1))).enumerate().for_each(|(y, (ra, rb))| {
        let fy = y as f32 / step as f32;
        let (y0, ty) = ((fy.floor() as usize).min(gh - 1), fy - fy.floor());
        let y1 = (y0 + 1).min(gh - 1);
        for (x, (oa, ob)) in ra.iter_mut().zip(rb.iter_mut()).enumerate() {
            let fx = x as f32 / step as f32;
            let (x0, tx) = ((fx.floor() as usize).min(gw - 1), fx - fx.floor());
            let x1 = (x0 + 1).min(gw - 1);
            let at = |gx: usize, gy: usize| grid.get(gy * gw + gx).copied().unwrap_or((0.0, 0.0));
            let mix = |p: (f32, f32), q: (f32, f32), t: f32| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
            let v = mix(mix(at(x0, y0), at(x1, y0), tx), mix(at(x0, y1), at(x1, y1), tx), ty);
            (*oa, *ob) = v;
        }
    });
    (a, b)
}

impl Field {
    /// The pattern of effects `e` over the field, thrown by the light (`rig`: the light set up on this field) as
    /// [`crate::projector`] says, drawn as [`crate::pattern`] says: `None` for no pattern. `coarse` (a drag's): drawn at
    /// every second pixel each way and brought to the field's size.
    pub fn pattern_planes(&self, rig: &Rig, e: &Effects, coarse: bool) -> Option<PatternPlanes> {
        if e.pattern == Pattern::None {
            return None;
        }
        let (w, h) = (self.width, self.height);
        if e.pattern == Pattern::Caustics {
            return self.caustic_planes(rig, e, coarse);
        }
        let projector = Projector::new(self, rig, e);
        let open = unit(e.open, 0.5);
        // (baked here, before the pixels run in parallel: see `pattern::passed`)
        let leaves = (e.pattern == Pattern::Leaves).then(crate::leaves::Leaves::get);
        let (light, through) = over_field(w, h, coarse, |x, y| {
            let (lz, normal, _) = self.at_px(y * w + x);
            let p = self.point((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, lz);
            let s = projector.sample(p, normal);
            let passed = crate::pattern::passed(e.pattern, &s, &projector.opening, open, leaves.as_deref());
            (passed.light, passed.through)
        });
        Some(PatternPlanes { light, through })
    }

    /// A field pixel's size per unit of depth.
    pub(crate) fn pixel(&self) -> f32 {
        crate::field::LENS / self.width.max(self.height).max(1) as f32
    }

    /// The water's net over the field (issue #333, [`crate::caustics`]) for the sun `rig`: how much of the sun's light each
    /// pixel gets, 0.5 on average, up to 4 on a line (more than 1: a net gathers light into lines). `None` for a light that is not a sun.
    fn caustic_planes(&self, rig: &Rig, e: &Effects, coarse: bool) -> Option<PatternPlanes> {
        let (w, h) = (self.width, self.height);
        let surface = crate::caustics::Surface::new(rig, e, self.pixel())?;
        let (light, _) = over_field(w, h, coarse, |x, y| {
            let (lz, _, _) = self.at_px(y * w + x);
            let p = self.point((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, lz);
            (surface.at(p), 0.0)
        });
        Some(PatternPlanes { light, through: Vec::new() })
    }

    /// `lobe` seen through the pattern of effects `e` (see [`Field::pattern_planes`]): the light that passes straight
    /// through, and the light the pattern's material passes (`None` without a pattern), which takes
    /// [`Effects::through_colour`].
    pub fn patterned(&self, rig: &Rig, lobe: Lobe, e: &Effects, coarse: bool) -> (Lobe, Option<Lobe>) {
        if lobe.m.len() != self.width * self.height {
            return (lobe, None);
        }
        let Some(planes) = self.pattern_planes(rig, e, coarse) else { return (lobe, None) };
        // (no material of its own passes any: water's light is the net alone)
        let through = (!planes.through.is_empty()).then(|| Lobe { m: lobe.m.iter().zip(&planes.through).map(|(m, t)| m * t).collect() });
        let light = Lobe { m: lobe.m.iter().zip(&planes.light).map(|(m, t)| m * t).collect() };
        (light, through)
    }

    /// The pattern of effects `e` in the air (issue #317) for light `rig` shining from `from` (`rig` itself, or a patterned
    /// lamp's far source, [`Field::pattern_rig`]): `None` for no pattern.
    pub(crate) fn air_pattern(&self, rig: &Rig, from: &Rig, e: &Effects) -> Option<AirPattern> {
        if e.pattern == Pattern::None {
            return None;
        }
        let projector = Projector::new(self, rig, e);
        let water = (e.pattern == Pattern::Caustics).then(|| crate::caustics::Surface::new(from, e, self.pixel())).flatten();
        let ray = match from.shape {
            // (a spot throws the pattern from its own place: the projector reads the air as it is)
            Shape::Spot { .. } => Ray::Own,
            Shape::Sun { to, .. } => Ray::Along(to),
            _ => Ray::From(from.position()),
        };
        let leaves = (e.pattern == Pattern::Leaves).then(crate::leaves::Leaves::get);
        Some(AirPattern { plane: projector.plane_depth(), projector, ray, pattern: e.pattern, open: unit(e.open, 0.5), leaves, water })
    }
}

/// How a point of the air is carried to the plane where the pattern lands on the surfaces.
#[derive(Clone, Copy, Debug)]
enum Ray {
    /// Along the sun's parallel rays.
    Along([f32; 3]),
    /// Along the ray from the light's place.
    From([f32; 3]),
    /// Not at all (a spot is its own projector).
    Own,
}

/// A light's pattern in the air (issue #317, [`Field::air_pattern`]): a point of the haze takes what the pattern passes where
/// the light's ray through it meets the plane the pattern lands on (the middle of the light's pool), so blinds and window
/// panes make shafts of their shape that run the light's way, and leaves dapple the beam.
pub(crate) struct AirPattern {
    projector: Projector,
    ray: Ray,
    plane: f32,
    pattern: Pattern,
    open: f32,
    leaves: Option<std::sync::Arc<crate::leaves::Leaves>>,
    /// The water's surface, for Caustics: the point is carried up the sun's ray to it, not along `ray` to the pattern's plane.
    water: Option<crate::caustics::Surface>,
}

impl AirPattern {
    /// The share of the light the pattern passes to camera-space point `q` of the air (0 .. 1), straight through and through
    /// its material alike (the beam has one colour), at its widest edges (the haze spreads a shaft's edge).
    pub(crate) fn at(&self, q: [f32; 3]) -> f32 {
        if let Some(water) = &self.water {
            return water.column(q);
        }
        let along = |o: [f32; 3], d: [f32; 3]| -> Option<[f32; 3]> {
            let t = (self.plane - o[2]) / d[2];
            (d[2].abs() > 1e-4 && t.is_finite()).then(|| [o[0] + d[0] * t, o[1] + d[1] * t, self.plane])
        };
        let p = match self.ray {
            Ray::Own => Some(q),
            Ray::Along(to) => along(q, to),
            Ray::From(l) => along(l, [q[0] - l[0], q[1] - l[1], q[2] - l[2]]),
        };
        let Some(p) = p.filter(|p| p.iter().all(|c| c.is_finite())) else { return 1.0 };
        // (no normal: the surface's incidence reads as the most grazing, the widest edges)
        let s = self.projector.sample(p, [0.0; 3]);
        let passed = crate::pattern::passed(self.pattern, &s, &self.projector.opening, self.open, self.leaves.as_deref());
        let v = passed.light + passed.through;
        if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::Light;

    fn flat(lz: impl Fn(usize, usize) -> f32) -> Field {
        let (w, h) = (192, 128);
        let lz: Vec<f32> = (0..w * h).map(|i| lz(i % w, i / w)).collect();
        Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap()
    }

    fn light() -> Light {
        use crate::kinds::LightKind;
        Light { x: 0.5, y: 0.5, depth: 0.0, amount: 1.0, softness: 0.5, warmth: 0.0, wrap: 0.4, kind: LightKind::Point }
    }

    /// The finish stage's local temperature and tint on `rgb`, as `finish.rs` applies them.
    fn tinted(rgb: [f32; 3], (t, s): (f32, f32)) -> [f32; 3] {
        let lum = |c: [f32; 3]| 0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2];
        let c = [rgb[0] * (1.0 + 0.3 * t), rgb[1] * (1.0 - 0.22 * s), rgb[2] * (1.0 - 0.3 * t).max(0.0)];
        let k = lum(rgb) / lum(c).max(1e-9);
        c.map(|v| v * k)
    }

    #[test]
    fn a_red_gel_makes_grey_red_and_keeps_it_finite_and_none_changes_warmth_not_at_all() {
        let grey = [0.2; 3];
        let red = Effects { gel_hue: 0.0, gel_strength: 1.0, ..Effects::default() };
        let c = tinted(grey, red.light_colour(0.0));
        assert!(c.iter().all(|v| v.is_finite()) && c[0] > 2.0 * c[1] && c[0] > 2.0 * c[2], "{c:?}");
        // a teal gel is green-blue, a magenta one is red-blue
        let teal = tinted(grey, Effects { gel_hue: 180.0, gel_strength: 1.0, ..Effects::default() }.light_colour(0.0));
        assert!(teal[0] < teal[1] && teal[0] < teal[2], "{teal:?}");
        let magenta = tinted(grey, Effects { gel_hue: 300.0, gel_strength: 1.0, ..Effects::default() }.light_colour(0.0));
        assert!(magenta[1] < magenta[0] && magenta[1] < magenta[2], "{magenta:?}");
        // half the strength is redder than none and less red than full (red over green, so the grey's own ratio is 1)
        let half = tinted(grey, Effects { gel_strength: 0.5, ..red }.light_colour(0.0));
        let ratio = |c: [f32; 3]| c[0] / c[1];
        assert!(ratio(half) > 1.05 && ratio(half) < ratio(c), "{half:?} {c:?}");
        // without a gel the colour is Warmth's tilt and no tint, to the bit
        for w in [-1.0, -0.3, 0.0, 0.6, 1.0] {
            assert_eq!(Effects::default().light_colour(w), (warmth_tilt(w), 0.0));
        }
        // a warm light behind a red gel is redder than a neutral one
        let warm = tinted(grey, red.light_colour(1.0));
        assert!(warm[0] / warm[2] > c[0] / c[2], "{warm:?} {c:?}");
    }

    #[test]
    fn hostile_numbers_give_finite_colours_and_planes() {
        let bad = Effects {
            gel_hue: f64::NAN,
            gel_strength: 1e9,
            pattern: Pattern::from_index(f64::NAN),
            scale: f64::INFINITY,
            rotation: f64::NAN,
            softness: -4.0,
            haze: f64::NAN,
            scatter: f64::NAN,
            dust: f64::INFINITY,
            shadows: f64::NAN,
            open: f64::NAN,
            mist: f64::INFINITY,
            mist_height: f64::NAN,
            fade: -3.0,
            source: f64::NAN,
        };
        for w in [f64::NAN, 9.0, -9.0] {
            let (t, s) = bad.light_colour(w);
            assert!(t.is_finite() && s.is_finite(), "{t} {s}");
        }
        assert_eq!(Pattern::from_index(1e30), Pattern::None);
        let f = flat(|_, _| 0.0);
        let rig = f.rig(&Light { x: f64::NAN, depth: -3.0, ..light() }, 0.0, 0.0);
        for pattern in [Pattern::Blinds, Pattern::Panes, Pattern::Leaves, Pattern::Dapple] {
            let e = Effects { pattern, ..bad };
            for coarse in [false, true] {
                let p = f.pattern_planes(&rig, &e, coarse).unwrap();
                assert!(p.light.iter().chain(&p.through).all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
            }
        }
    }

    #[test]
    fn a_gel_travels_as_a_tint_plane_and_stays_out_of_the_way_without_one() {
        let f = flat(|_, _| 0.0);
        let rig = f.rig(&light(), 0.0, 0.0);
        let lobe = f.lobe(&rig);
        let teal = Effects { gel_hue: 180.0, gel_strength: 1.0, ..Effects::default() };
        let (tilt, tint) = teal.light_colour(0.0);
        let sh = f.shading_of(&[crate::Part { lobe: &lobe, gain: rig.gain(), tilt, tint }], 1.0, 0.0);
        let s = sh.split(24, 16, &|x, y| (x / 24.0, y / 16.0));
        // teal: cool and green (more green than the mean of its red and blue: a negative tint), no warm, no magenta
        assert!(s.cool.is_some() && s.green.is_some() && s.warm.is_none() && s.magenta.is_none());
        assert!(sh.tilt_down > 0.0 && sh.tint_down > 0.0 && sh.tint_up == 0.0, "{} {} {}", sh.tilt_down, sh.tint_up, sh.tint_down);
        // the planes hold the light: strong where the light is, and the green plane is as wide as the cool one
        let top = |p: &Option<Vec<f32>>| p.as_ref().map_or(0.0, |v| v.iter().copied().fold(0.0, f32::max));
        assert!(top(&s.cool) > 0.3 && top(&s.green) > 0.3 && (top(&s.cool) - top(&s.green)).abs() < 0.05, "{} {}", top(&s.cool), top(&s.green));
        let plain = f.shading_of(&[crate::Part { lobe: &lobe, gain: rig.gain(), tilt: 0.3, tint: 0.0 }], 1.0, 0.0);
        // a light with a tilt and no gel makes a warm plane only, and holds no tint memory
        let p = plain.split(8, 8, &|x, y| (x / 8.0, y / 8.0));
        assert!(plain.bytes() == 192 * 128 * 12 && p.warm.is_some() && p.cool.is_none() && p.magenta.is_none() && p.green.is_none());
    }
}
