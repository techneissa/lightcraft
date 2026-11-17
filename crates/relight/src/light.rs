//! The light: a small round source at a point in the scene. Each pixel's surface point (from the field's depth,
//! through a 60° lens) gets light by its 3D distance to the source, as from a sphere of the Softness's size (an
//! inverse square that stays finite near it), and by how its surface faces it (a gentle term: a surface turned
//! away keeps part of it, so the depth model's small errors never show as lighting texture). The result is a gain
//! on scene-linear light: `ambient + amount · light`, normalized so the most lit point of the photo gets the
//! Amount; the gain rolls off towards white (a bright pixel can't be pushed past it) and is held back in the
//! deepest shadows (their noise is not lifted). The light's colour is a temperature (Warmth), given only to the
//! light added.
//!
//! Several lights (issue #299): each light's `min(falloff · facing / peak, 1)` over the field is its [`Lobe`], made
//! once per light and kept by the caller (it doesn't depend on Amount, Warmth or Darken Surroundings). The lobes add
//! up in scene-linear light (`ambient + Σ amountᵢ · lobeᵢ`), the roll-off and the shadow hold work on that total
//! (so two lights never push a pixel past the ceiling), and each pixel's colour is the mix of the lights' colours
//! weighted by how much of the added light each gave it ([`Part::tilt`], [`Shading::split`] hands the finish stage
//! one plane for warm tilts and one for cool ones).

use rayon::prelude::*;

use crate::field::Field;
use crate::kinds::{LightKind, Shape};

/// The light's settings, as plain numbers (the develop settings convert to this).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// Where it is, in normalized photo coordinates (0..1, before the crop and the lens corrections).
    pub x: f64,
    pub y: f64,
    /// How far away: 0 in front of the nearest thing in the photo, 1 at the farthest.
    pub depth: f64,
    /// How strong, 0..1 (1: the most lit point brightens by three stops).
    pub amount: f64,
    /// How large the source is, 0 (a tight pool) .. 1 (broad, window-like light).
    pub softness: f64,
    /// The light's colour, −1 (cool, about 16 000 K) .. 0 (the photo's white) .. 1 (warm, about 2 700 K).
    pub warmth: f64,
    /// How far light wraps around shapes, 0 (follows them closely) .. 1 (ignores them).
    pub wrap: f64,
    /// What the light is (issue #300): a point, a spot, a sun, a softbox, a rim or a fill.
    pub kind: LightKind,
}

impl Light {
    /// Whether it adds no light.
    pub fn is_off(&self) -> bool {
        !matches!(self.amount.partial_cmp(&0.0), Some(std::cmp::Ordering::Greater))
    }
}

/// The most lights a photo holds (the settings, the render and the commands keep to it).
pub const MAX_LIGHTS: usize = 8;

/// The brightest the light makes the most lit point: `1 + PEAK_GAIN` times (three stops).
pub const PEAK_GAIN: f32 = 7.0;
/// Log2 luminance (relative to 18 % grey) the light never takes a pixel past: 2.3 stops over grey (0.89 in
/// scene-linear light), just under white so a lit face keeps its highlights.
const CEILING: f32 = 2.3;
/// How far under [`CEILING`] the roll-off begins (stops).
const KNEE: f32 = 1.3;
/// Log2 luminance (relative to 18 % grey) below which the roll-off is off (`.0`) and from which it holds (`.1`): the
/// pixels in between take it in part, so a mid-grey at the most lit point gains about 2.25 stops of the 3 at full
/// Amount (nothing goes past the ceiling), and everything from `.1` (where the roll-off begins, [`CEILING`] − [`KNEE`]) up is rolled off as it was.
const FULL: (f32, f32) = (-0.3, 1.0);
/// The knee of the cut that keeps even those pixels under [`CEILING`] (stops): a mid-grey gains about 2.25 of its 3.
const FREE_KNEE: f32 = 0.3;
/// The darkest the rest of the photo gets at full Darken Surroundings: `1 − DARKEN_MAX` of its light (3.3 stops).
const DARKEN_MAX: f32 = 0.9;
/// Where shadows begin to be held back (log2 luminance below 18 % grey): fully at `SHADOW.0`, not at `SHADOW.1`.
const SHADOW: (f32, f32) = (-8.0, -5.0);

/// The share of its own light a pixel keeps at Darken Surroundings `darken` (0..1; not a number: 1).
pub fn ambient(darken: f64) -> f32 {
    if darken.is_finite() { 1.0 - DARKEN_MAX * darken.clamp(0.0, 1.0) as f32 } else { 1.0 }
}

/// A light set up on a field: the numbers every pixel's shading needs. `darken` (the whole photo's, not the light's)
/// and the photo's exposure come with it, so one light can be asked about a point ([`Field::shade`]).
#[derive(Clone, Debug)]
pub struct Rig {
    pub(crate) pos: [f32; 3],
    /// The source's radius, squared.
    pub(crate) r2: f32,
    /// How fast the light falls off past the radius (an exponent: a tight light falls off faster than a broad one).
    pub(crate) tail: f32,
    amount: f32,
    ambient: f32,
    /// How much of the light the facing term decides, and how far it wraps past 90°.
    pub(crate) face: f32,
    pub(crate) wrap: f32,
    /// What kind of light it is, set up on the field (issue #300).
    pub(crate) shape: Shape,
    /// The largest `falloff · facing` of the photo (the Amount is given to it).
    pub(crate) peak: f32,
    /// The photo's exposure (stops): the guide's luminance plus this is the pixel's.
    pub(crate) ev: f32,
    /// The light's Softness, 0..1 (a pattern's edges take the light's size from it).
    pub(crate) soft: f32,
}

/// How much of a surface's light its facing decides: `cos` is the cosine between its normal and the direction to the light.
#[inline]
pub(crate) fn facing(face: f32, wrap: f32, cos: f32) -> f32 {
    let t = ((cos + wrap) / (1.0 + wrap)).clamp(0.0, 1.0);
    1.0 - face + face * t * t * t
}

/// A round source's `falloff · facing` at camera-space point `p` with normal `n` (the point light's, and the spot's
/// before its cone).
#[inline]
pub(crate) fn point_term(rig: &Rig, p: [f32; 3], n: [f32; 3]) -> f32 {
    let d = [rig.pos[0] - p[0], rig.pos[1] - p[1], rig.pos[2] - p[2]];
    let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let falloff = (rig.r2 / (rig.r2 + r2)).powf(rig.tail);
    let r = r2.sqrt().max(1e-9);
    let cos = (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / r;
    falloff * facing(rig.face, rig.wrap, cos)
}

/// What the light does at a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shade {
    /// The gain, in stops (negative where the surroundings darken).
    pub stops: f32,
    /// How much of the pixel's light is the new light's (0..1): its colour is given this much.
    pub share: f32,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Field {
    /// Set up `light` on this field with Darken Surroundings `darken` (0..1), for a photo at exposure `ev` (stops).
    pub fn rig(&self, light: &Light, darken: f64, ev: f64) -> Rig {
        let f = |v: f64, lo: f64, hi: f64, d: f32| if v.is_finite() { v.clamp(lo, hi) as f32 } else { d };
        let (x, y) = (f(light.x, -0.5, 1.5, 0.5), f(light.y, -0.5, 1.5, 0.5));
        let depth = f(light.depth, 0.0, 1.0, 0.3);
        // from in front of the nearest thing (55 % of its distance) to the farthest
        let near = self.near - crate::probe::NEAR_MARGIN;
        let lz = near + depth * (self.far - near).max(0.1);
        let pos = self.point(x, y, lz);
        let zl = lz.exp();
        let soft = f(light.softness, 0.0, 1.0, 0.5);
        let radius = zl * (0.03 + 1.4 * soft * soft);
        let wrap = f(light.wrap, 0.0, 1.0, 0.4);
        let shape = Shape::new(&light.kind, self, pos, lz, soft);
        let mut rig = Rig {
            pos,
            r2: radius * radius,
            amount: f(light.amount, 0.0, 1.0, 0.0) * PEAK_GAIN,
            tail: 1.0 + 1.5 * (1.0 - soft),
            ambient: ambient(darken),
            face: 1.0 - wrap,
            wrap: 0.1 + 0.9 * wrap,
            peak: 1.0,
            shape,
            ev: if ev.is_finite() { ev.clamp(-10.0, 10.0) as f32 } else { 0.0 },
            soft,
        };
        // the most lit point, on a grid of the photo (a shape with a fixed peak has no use for it: asking would make a Rim's
        // map for nothing)
        if !rig.shape.fixed_peak() {
            rig.peak = self.peak_of(&rig);
        }
        rig
    }

    /// The most lit point of `rig`'s light (`falloff · facing`), on a grid of the photo.
    pub(crate) fn peak_of(&self, rig: &Rig) -> f32 {
        let (gw, gh) = (64usize, 48usize);
        let peak = (0..gw * gh)
            .into_par_iter()
            .map(|i| {
                let (u, v) = (((i % gw) as f32 + 0.5) / gw as f32, ((i / gw) as f32 + 0.5) / gh as f32);
                let (lz, n, _) = self.at(u, v);
                self.light_at(rig, u, v, lz, n)
            })
            .reduce(|| 0.0, f32::max);
        peak.max(1e-6)
    }

    /// `falloff · facing` at `(u, v)` with log depth `lz` and normal `n`, before the peak's normalization.
    #[inline]
    pub(crate) fn light_at(&self, rig: &Rig, u: f32, v: f32, lz: f32, n: [f32; 3]) -> f32 {
        if !matches!(rig.shape, Shape::Point) {
            return crate::kinds::light_at(self, rig, (u, v), lz, n);
        }
        let v = point_term(rig, self.point(u, v, lz), n);
        if v.is_finite() { v } else { 0.0 }
    }

    /// What the light does at normalized photo coordinates `(u, v)`.
    #[inline]
    pub fn shade(&self, rig: &Rig, u: f32, v: f32) -> Shade {
        let (lz, n, lum) = self.at(u, v);
        self.shade_with(rig, (u, v), lz, n, lum)
    }

    /// [`Field::shade`] at `(u, v)` with the field's values there already read.
    #[inline]
    fn shade_with(&self, rig: &Rig, (u, v): (f32, f32), lz: f32, n: [f32; 3], lum: f32) -> Shade {
        let added = rig.amount * (self.light_at(rig, u, v, lz, n) / rig.peak).min(1.0);
        grade(rig.ambient, rig.ev, added, lum)
    }
}

/// What the lights do at a pixel with luminance `lum` (log2, relative to the photo's exposure `ev`): `added` is the
/// light they add together (`Σ amountᵢ · lobeᵢ`, in units of the pixel's own light) over `ambient` (1 less what
/// Darken Surroundings takes).
#[inline]
fn grade(ambient: f32, ev: f32, added: f32, lum: f32) -> Shade {
    let gain = ambient + added;
    let raw = gain.max(1e-3).log2();
    let l = lum + ev;
    // how much the deepest shadows are held back
    let held = 0.35 + 0.65 * smoothstep(SHADOW.0, SHADOW.1, l);
    let stops = if raw > 0.0 {
        // towards white: the pixel's new brightness rolls off as it nears the ceiling (a pixel already past it
        // gets nothing more)
        let target = l + raw;
        let start = CEILING - KNEE;
        let limited = if target <= start { raw } else { (start + KNEE * (1.0 - (-(target - start) / KNEE).exp()) - l).clamp(0.0, raw) };
        // (the roll-off holds from `FULL.1` up; below `FULL.0` the light gets all its stops, so mid-grey at the
        // peak gains most of the Amount: 2.25 of 3 stops, never past the ceiling)
        let free = 1.0 - smoothstep(FULL.0, FULL.1, l);
        // (but never past the ceiling: the stops it gets are cut to what fits under it, by a tighter knee)
        let start = CEILING - FREE_KNEE;
        let cut = if target <= start { raw } else { (start + FREE_KNEE * (1.0 - (-(target - start) / FREE_KNEE).exp()) - l).clamp(0.0, raw) };
        let rolled = limited + (cut.max(limited) - limited) * free;
        // and less in the deepest shadows
        rolled * held
    } else {
        raw
    };
    // the light's colour fades by the pixel's own brightness alone (out towards white, held back in the deepest
    // shadows), the same where the gain brightens and where Darken Surroundings darkens, so it never jumps where
    // the gain crosses 1
    let lit = if gain > 0.0 { added / gain } else { 0.0 };
    // (the light's colour reaches further than its brightness does: a pixel with half its light from the new
    // light takes three quarters of its colour)
    let colour = 1.0 - (1.0 - lit) * (1.0 - lit);
    let share = colour * held * (1.0 - smoothstep(CEILING - KNEE, CEILING, l));
    Shade { stops, share: if share.is_finite() { share.clamp(0.0, 1.0) } else { 0.0 } }
}

/// The light's colour for Warmth `w` (−1..1) as the strength of a red–blue tilt `t`, the one the finish stage's
/// local temperature applies (`(1 + 0.3 t, 1, 1 − 0.3 t)`, luminance kept): the ratio of red to blue of a
/// black body against the photo's white (D65), from about 16 000 K (cool) to 2 700 K (warm).
pub fn warmth_tilt(w: f64) -> f32 {
    if !w.is_finite() || w == 0.0 {
        return 0.0;
    }
    let w = w.clamp(-1.0, 1.0);
    let mired = 1e6 / 6504.0 + if w > 0.0 { 210.0 * w } else { 90.0 * w };
    let xy = lightcraft_color::cct::temp_tint_to_xy(1e6 / mired, 0.0);
    let rgb = lightcraft_color::REC2020.from_xyz().apply(xy.to_xyz());
    let q = rgb[0] / rgb[2].max(1e-6);
    ((q - 1.0) / (0.3 * (q + 1.0))) as f32
}

/// One light's [`Light`]-independent part of the shading: `min(falloff · facing / peak, 1)` (0..1) at each field pixel,
/// before Amount. It depends on the light's place, depth, Softness and Wrap and on the photo's field, and not on
/// Amount, Warmth or Darken Surroundings, so a caller makes one per light and keeps it (dragging one light makes
/// one new lobe; the others stay).
#[derive(Clone, Debug)]
pub struct Lobe {
    pub(crate) m: Vec<f32>,
}

impl Lobe {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.m.len() * 4
    }
}

/// One light's share in a [`Shading`]: its lobe, its strength (the Amount times [`PEAK_GAIN`], as in [`Rig`]) and its
/// colour as a red–blue tilt ([`warmth_tilt`]) and a green–magenta tint (a gel, `effects::Effects::light_colour`; 0
/// for a light with none).
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    pub lobe: &'a Lobe,
    pub gain: f32,
    pub tilt: f32,
    pub tint: f32,
}

impl Rig {
    /// The strength [`Part::gain`] takes: the Amount times [`PEAK_GAIN`].
    pub fn gain(&self) -> f32 {
        self.amount
    }

    /// The light's place in the field's camera space (the effects project a pattern from it).
    pub(crate) fn position(&self) -> [f32; 3] {
        self.pos
    }
}

/// The light over the whole field: each field pixel's gain and the lights' colour. Made once per change of the lights
/// (a drag of one light makes one new [`Lobe`] and one of these) and read by every render of it, whatever its size or
/// window.
///
/// Along a step in depth (the field's edge band) the light jumps from one side's to the other's; there it is
/// refined by a guided filter against the photo's luminance (a local linear fit over a small window), so it changes
/// where and as softly as the photo's own edge does: a sharp outline stays sharp, a soft one (out of focus, hair)
/// mixes the two sides' light as its pixels mix the two sides.
#[derive(Clone, Debug)]
pub struct Shading {
    pub width: usize,
    pub height: usize,
    /// Each pixel's gain (stops) and the share of its light that is the new lights' with a warm tilt and with a cool one.
    px: Vec<[f32; 3]>,
    /// The same for a gel's green–magenta tint: the share with a magenta tint and with a green one. Empty (no memory
    /// held) while no light has a tint.
    tn: Vec<[f32; 2]>,
    /// The largest brightening and darkening (stops, both ≥ 0).
    pub up_top: f32,
    pub down_top: f32,
    /// The strongest warm tilt among the lights, and the strongest cool one (as a positive number): the finish
    /// stage's temperature for the warm plane and for the cool plane (0: no such plane).
    pub tilt_up: f32,
    pub tilt_down: f32,
    /// The strongest magenta tint among the lights and the strongest green one (positive): the finish stage's tint for
    /// the magenta plane and for the green plane (0: no such plane).
    pub tint_up: f32,
    pub tint_down: f32,
}

/// A render's planes of the light, finished for the finish stage ([`Shading::split`]): the brightening gain over
/// [`Shading::up_top`], the darkening over [`Shading::down_top`] (each 0..1), and the light's colour as one plane
/// of the share of warm-tilted light (applied with [`Shading::tilt_up`]) and one of cool-tilted light (with
/// −[`Shading::tilt_down`]), and for a gel one of magenta-tinted light (with [`Shading::tint_up`]) and one of
/// green-tinted light (with −[`Shading::tint_down`]), each 0..1.
pub struct Split {
    pub width: usize,
    pub height: usize,
    pub up: Vec<f32>,
    pub down: Vec<f32>,
    pub warm: Option<Vec<f32>>,
    pub cool: Option<Vec<f32>>,
    pub magenta: Option<Vec<f32>>,
    pub green: Option<Vec<f32>>,
}

/// The guided filter's regularization along an edge (a variance of the cube-root luminance).
const EDGE_EPS: f32 = 1e-3;

impl Field {
    /// The lobe of `rig`'s light over the field.
    pub fn lobe(&self, rig: &Rig) -> Lobe {
        let (w, h) = (self.width, self.height);
        rig.shape.prepare(self);
        let mut m = vec![0f32; w * h];
        m.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, a) in row.iter_mut().enumerate() {
                // (at the field's own pixels: its values as they are, nothing to interpolate)
                let (lz, n, _) = self.at_px(y * w + x);
                *a = (self.light_at(rig, (x as f32 + 0.5) / w as f32, v, lz, n) / rig.peak).min(1.0);
            }
        });
        Lobe { m }
    }

    /// The light of `rig` alone over the field (its lobe, made here).
    pub fn shading(&self, rig: &Rig) -> Shading {
        let lobe = self.lobe(rig);
        self.shading_of(&[Part { lobe: &lobe, gain: rig.amount, tilt: 1.0, tint: 0.0 }], rig.ambient, rig.ev)
    }

    /// The lights `parts` together over the field, with `ambient` (1 less what Darken Surroundings takes, [`Rig`]) for a
    /// photo at exposure `ev` (stops). A lobe that isn't of this field's size adds nothing.
    pub fn shading_of(&self, parts: &[Part<'_>], ambient: f32, ev: f32) -> Shading {
        let (w, h) = (self.width, self.height);
        let n = w * h;
        let parts: Vec<Part<'_>> = parts.iter().filter(|p| p.lobe.m.len() == n && p.gain.is_finite() && p.gain > 0.0).copied().collect();
        // the colour components: warm and cool (a tilt's sign), magenta and green (a tint's sign), each with the
        // strongest of the lights' values as its top
        let comp = |p: &Part<'_>, k: usize| -> f32 {
            let v = if k < 2 { p.tilt } else { p.tint };
            let v = if k.is_multiple_of(2) { v } else { -v };
            if v.is_finite() { v.max(0.0) } else { 0.0 }
        };
        let tops: [f32; 4] = std::array::from_fn(|k| parts.iter().map(|p| comp(p, k)).fold(0f32, f32::max));
        let (tilt_up, tilt_down, tint_up, tint_down) = (tops[0], tops[1], tops[2], tops[3]);
        // each light's weight in each component (its value over the strongest)
        let weights: Vec<[f32; 4]> =
            parts.iter().map(|p| std::array::from_fn(|k| if tops[k] > 0.0 { (comp(p, k) / tops[k]).clamp(0.0, 1.0) } else { 0.0 })).collect();
        let want_tint = tint_up > 0.0 || tint_down > 0.0;
        let ev = if ev.is_finite() { ev.clamp(-10.0, 10.0) } else { 0.0 };
        let mut stops = vec![0f32; n];
        let mut col = vec![[0f32; 4]; n];
        stops.par_chunks_mut(w.max(1)).zip(col.par_chunks_mut(w.max(1))).enumerate().for_each(|(y, (st, co))| {
            for (x, (a, c)) in st.iter_mut().zip(co.iter_mut()).enumerate() {
                let i = y * w + x;
                let (mut total, mut sums) = (0f32, [0f32; 4]);
                for (p, wk) in parts.iter().zip(&weights) {
                    let added = p.gain * p.lobe.m.get(i).copied().unwrap_or(0.0);
                    total += added;
                    for (s, k) in sums.iter_mut().zip(wk) {
                        *s += added * k;
                    }
                }
                let (_, _, lum) = self.at_px(i);
                let s = grade(ambient, ev, total, lum);
                *a = s.stops;
                if total > 0.0 {
                    *c = sums.map(|v| s.share * (v / total));
                }
            }
        });
        // (a colour plane that no light has a component for stays 0 and is refined for nothing)
        let active: [bool; 4] = std::array::from_fn(|k| tops[k] > 0.0 && (k < 2 || want_tint));
        // the gain and each colour plane some light has a component for, refined along the edge band as they always were
        // (each its own tables of running sums in f64, so an edit renders to the byte as before), the planes at once
        let wanted: Vec<usize> = (0..5).filter(|&q| q == 0 || active.get(q - 1).copied().unwrap_or(false)).collect();
        let fits: Vec<(usize, Vec<(usize, f32)>)> = if self.band.is_empty() {
            Vec::new()
        } else {
            wanted
                .par_iter()
                .map(|&q| {
                    let p: Vec<f32> = if q == 0 { stops.clone() } else { col.par_iter().map(|c| c.get(q - 1).copied().unwrap_or(0.0)).collect() };
                    (q, self.refined(&p))
                })
                .collect()
        };
        for (q, fit) in fits {
            for (i, v) in fit {
                if q == 0 {
                    if let Some(s) = stops.get_mut(i) {
                        *s = v;
                    }
                } else if let Some(c) = col.get_mut(i).and_then(|c| c.get_mut(q - 1)) {
                    *c = v.clamp(0.0, 1.0);
                }
            }
        }
        let (up_top, down_top) =
            stops.par_iter().fold(|| (0f32, 0f32), |(u, d), v| (u.max(*v), d.max(-*v))).reduce(|| (0.0, 0.0), |a, b| (a.0.max(b.0), a.1.max(b.1)));
        let tn = if want_tint { col.par_iter().map(|c| [c[2], c[3]]).collect() } else { Vec::new() };
        let px = stops.par_iter().zip(&col).map(|(a, c)| [*a, c[0], c[1]]).collect();
        Shading { width: w, height: h, px, tn, up_top, down_top, tilt_up, tilt_down, tint_up, tint_down }
    }
}

impl Field {
    /// Plane `p` (the field's size) refined along the edge band: at each band pixel, a local linear fit of `p` against the
    /// guide over the pixel's window, kept within the range the window holds. The changed pixels and their values.
    pub(crate) fn refined(&self, p: &[f32]) -> Vec<(usize, f32)> {
        let (w, h, r) = (self.width, self.height, crate::field::BAND_RADIUS);
        if self.band.is_empty() || p.iter().all(|v| *v == 0.0) {
            return Vec::new();
        }
        let gp: Vec<f32> = p.par_iter().zip(&self.gi).map(|(a, b)| a * b).collect();
        let (sp, sgp) = rayon::join(|| sat(p, w, h), || sat(&gp, w, h));
        let stride = w + 1;
        let window = |t: &[f64], x0: usize, y0: usize, x1: usize, y1: usize| -> f64 {
            let at = |x: usize, y: usize| t.get(y * stride + x).copied().unwrap_or(0.0);
            at(x1, y1) - at(x0, y1) - at(x1, y0) + at(x0, y0)
        };
        self.band
            .par_iter()
            .filter_map(|&(i, mean_g, var_g)| {
                let i = i as usize;
                let (x, y) = (i % w, i / w);
                let (x0, y0, x1, y1) = (x.saturating_sub(r), y.saturating_sub(r), (x + r + 1).min(w), (y + r + 1).min(h));
                let k = ((x1 - x0) * (y1 - y0)) as f64;
                if k < 1.0 {
                    return None;
                }
                let (mp, mgp) = ((window(&sp, x0, y0, x1, y1) / k) as f32, (window(&sgp, x0, y0, x1, y1) / k) as f32);
                let a = (mgp - mean_g * mp) / (var_g + EDGE_EPS);
                let q = a * self.gi.get(i).copied()? + (mp - a * mean_g);
                // (the local linear fit can swing past both sides where the photo's luminance and the light
                // disagree: the light stays within the range the window holds, the two sides')
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for row in y0..y1 {
                    for v in p.get(row * w + x0..row * w + x1)? {
                        lo = lo.min(*v);
                        hi = hi.max(*v);
                    }
                }
                (q.is_finite() && lo <= hi).then(|| (i, q.clamp(lo, hi)))
            })
            .collect()
    }
}

/// The summed-area table of `p` (`w × h`): `(w + 1) × (h + 1)` sums, in f64 (exact enough for any window).
fn sat(p: &[f32], w: usize, h: usize) -> Vec<f64> {
    let stride = w + 1;
    let mut t = vec![0f64; stride * (h + 1)];
    // each row's running sum, then each row adds the one above it
    t.par_chunks_mut(stride).skip(1).enumerate().for_each(|(y, row)| {
        let mut acc = 0f64;
        for (x, v) in row.iter_mut().skip(1).enumerate() {
            acc += f64::from(p.get(y * w + x).copied().unwrap_or(0.0));
            *v = acc;
        }
    });
    for y in 1..=h {
        let (above, rest) = t.split_at_mut(y * stride);
        if let (Some(prev), Some(row)) = (above.get((y - 1) * stride..), rest.get_mut(..stride)) {
            row.iter_mut().zip(prev).for_each(|(v, a)| *v += a);
        }
    }
    t
}

/// The light's planes over a render: for each pixel, its gain in stops and the light's share.
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub stops: Vec<f32>,
    pub share: Vec<f32>,
}

impl Shading {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.px.len() * 12 + self.tn.len() * 8
    }

    /// The light at normalized photo coordinates `(u, v)` (bilinear, clamped at the borders); its `share` is the
    /// strongest of the colour planes.
    #[inline]
    pub fn at(&self, u: f32, v: f32) -> Shade {
        let [stops, warm, cool, magenta, green] = self.sample(u * self.width as f32, v * self.height as f32);
        Shade { stops, share: warm.max(cool).max(magenta).max(green).min(1.0) }
    }

    /// The values at field position `(x, y)` (pixel centres at `i + 0.5`): the gain, then the warm, cool, magenta and
    /// green planes; bilinear, clamped.
    #[inline]
    fn sample(&self, x: f32, y: f32) -> [f32; 5] {
        let (w, h) = (self.width, self.height);
        let (Some(xm), Some(ym)) = (w.checked_sub(1), h.checked_sub(1)) else { return [0.0; 5] };
        let fx = (x - 0.5).clamp(0.0, xm as f32);
        let fy = (y - 0.5).clamp(0.0, ym as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(xm), (y0 + 1).min(ym));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| {
            let [a, b, c] = self.px.get(y * w + x).copied().unwrap_or([0.0; 3]);
            let [d, e] = self.tn.get(y * w + x).copied().unwrap_or([0.0; 2]);
            [a, b, c, d, e]
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let lerp = |k: usize| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        };
        [lerp(0), lerp(1), lerp(2), lerp(3), lerp(4)]
    }

    /// [`Planes`] over `w × h` pixels; `to_photo` maps a pixel centre to normalized photo coordinates (a pixel it
    /// maps to no number gets no light).
    pub fn planes(&self, w: usize, h: usize, to_photo: &(dyn Fn(f64, f64) -> (f64, f64) + Sync)) -> Planes {
        let n = w.saturating_mul(h);
        let mut stops = vec![0f32; n];
        let mut share = vec![0f32; n];
        stops.par_chunks_mut(w.max(1)).zip(share.par_chunks_mut(w.max(1))).enumerate().for_each(|(y, (st, sh))| {
            for (x, (a, b)) in st.iter_mut().zip(sh.iter_mut()).enumerate() {
                let (u, v) = to_photo(x as f64 + 0.5, y as f64 + 0.5);
                if !(u.is_finite() && v.is_finite()) {
                    continue;
                }
                let s = self.at(u as f32, v as f32);
                *a = s.stops;
                *b = s.share;
            }
        });
        Planes { width: w, height: h, stops, share }
    }

    /// The render's planes, finished (see [`Split`]), in one pass over its `w × h` pixels; `to_photo` maps a pixel
    /// centre to normalized photo coordinates. A colour plane is made only when some light has that tilt or tint.
    pub fn split(&self, w: usize, h: usize, to_photo: &(dyn Fn(f64, f64) -> (f64, f64) + Sync)) -> Split {
        let n = w.saturating_mul(h);
        let (ku, kd) = (if self.up_top > 0.0 { 1.0 / self.up_top } else { 0.0 }, if self.down_top > 0.0 { 1.0 / self.down_top } else { 0.0 });
        let (fw, fh) = (self.width as f32, self.height as f32);
        let want = [self.tilt_up > 0.0, self.tilt_down > 0.0, self.tint_up > 0.0, self.tint_down > 0.0];
        let mut up = vec![0f32; n];
        let mut down = vec![0f32; n];
        let mut planes: [Vec<f32>; 4] = std::array::from_fn(|k| vec![0f32; if want[k] { n } else { 0 }]);
        let row = w.max(1);
        // (a colour plane that isn't wanted has no rows: its chunks are empty)
        let mut rows: Vec<[Option<&mut [f32]>; 4]> = (0..h).map(|_| [None, None, None, None]).collect();
        for (k, plane) in planes.iter_mut().enumerate() {
            for (slot, chunk) in rows.iter_mut().zip(plane.chunks_mut(row)) {
                if let Some(s) = slot.get_mut(k) {
                    *s = Some(chunk);
                }
            }
        }
        up.par_chunks_mut(row).zip(down.par_chunks_mut(row)).zip(rows.into_par_iter()).enumerate().for_each(|(y, ((ur, dr), mut cr))| {
            for x in 0..ur.len() {
                let (u, v) = to_photo(x as f64 + 0.5, y as f64 + 0.5);
                if !(u.is_finite() && v.is_finite()) {
                    continue;
                }
                let [stops, a, b, c, d] = self.sample(u as f32 * fw, v as f32 * fh);
                if let (Some(p), Some(q)) = (ur.get_mut(x), dr.get_mut(x)) {
                    (*p, *q) = ((stops * ku).clamp(0.0, 1.0), (-stops * kd).clamp(0.0, 1.0));
                }
                for (slot, value) in cr.iter_mut().zip([a, b, c, d]) {
                    if let Some(cell) = slot.as_deref_mut().and_then(|r| r.get_mut(x)) {
                        *cell = value.clamp(0.0, 1.0);
                    }
                }
            }
        });
        let [warm, cool, magenta, green] = planes;
        Split {
            width: w,
            height: h,
            up,
            down,
            warm: want[0].then_some(warm),
            cool: want[1].then_some(cool),
            magenta: want[2].then_some(magenta),
            green: want[3].then_some(green),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> Light {
        Light { x: 0.5, y: 0.5, depth: 0.0, amount: 1.0, softness: 0.4, warmth: 0.0, wrap: 0.0, kind: LightKind::Point }
    }

    /// A wall facing the camera (log depth 0) with the left half `near` nearer.
    fn two_planes(near: f32) -> Field {
        let (w, h) = (96, 64);
        let lz: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { -near } else { 0.0 }).collect();
        // (a mid-dark scene: two stops under grey, far from white and from the deepest shadows)
        Field::from_parts(w, h, lz, vec![-2.0; w * h]).unwrap()
    }

    #[test]
    fn near_things_catch_more_light_than_far_ones() {
        // the light in the middle, in front of everything: the near half is closer to it
        let f = two_planes(0.7);
        let rig = f.rig(&light(), 0.0, 0.0);
        let (near, far) = (f.shade(&rig, 0.4, 0.5), f.shade(&rig, 0.6, 0.5));
        assert!(near.stops > far.stops + 0.3, "near {near:?} far {far:?}");
        // and farther from the light it falls off on the same plane
        let edge = f.shade(&rig, 0.05, 0.5);
        assert!(edge.stops < near.stops, "{edge:?} {near:?}");
        // a light placed far back lights the far half more
        let back = f.rig(&Light { depth: 1.0, softness: 0.1, ..light() }, 0.0, 0.0);
        assert!(f.shade(&back, 0.6, 0.5).stops > f.shade(&back, 0.4, 0.5).stops);
    }

    #[test]
    fn surfaces_facing_the_light_brighten_more_than_those_turned_away() {
        // a ridge: log depth grows away from the middle column, so the left side faces left and the right side right
        let (w, h) = (96, 64);
        let lz: Vec<f32> = (0..w * h).map(|i| 0.02 * ((i % w) as f32 - w as f32 / 2.0).abs()).collect();
        let f = Field::from_parts(w, h, lz, vec![0.0; w * h]).unwrap();
        // the light far to the left, level with the ridge
        let rig = f.rig(&Light { x: -0.4, depth: 0.3, softness: 1.0, ..light() }, 0.0, 0.0);
        let (left, right) = (f.shade(&rig, 0.45, 0.5), f.shade(&rig, 0.55, 0.5));
        assert!(left.stops > right.stops * 1.3, "{left:?} {right:?}");
        // with the light wrapping fully, the facing makes no difference
        let flat = f.rig(&Light { x: -0.4, depth: 0.3, softness: 1.0, wrap: 1.0, ..light() }, 0.0, 0.0);
        let (l, r) = (f.shade(&flat, 0.45, 0.5), f.shade(&flat, 0.55, 0.5));
        assert!((l.stops - r.stops).abs() < 0.15 * l.stops.max(r.stops), "{l:?} {r:?}");
    }

    #[test]
    fn the_amount_goes_to_the_most_lit_point_and_white_is_not_passed() {
        let f = two_planes(0.5);
        let rig = f.rig(&light(), 0.0, 0.0);
        let best = (0..96).map(|x| f.shade(&rig, (x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
        assert!((best - 3.0).abs() < 0.2, "{best}");
        // a mid-grey point at the peak gains most of it (not two stops: the roll-off is for highlights)
        let grey = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![0.0; 96 * 64]).unwrap();
        let rig = grey.rig(&light(), 0.0, 0.0);
        let best = (0..96).map(|x| grey.shade(&rig, (x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
        assert!(best >= 2.2, "mid-grey gains {best} stops at full Amount");
        // and skin tones a half and a whole stop over grey stay at or under the ceiling
        for l in [0.5f32, 1.0] {
            let f = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![l; 96 * 64]).unwrap();
            let rig = f.rig(&light(), 0.0, 0.0);
            let top = (0..96).map(|x| f.shade(&rig, (x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
            assert!(l + top <= CEILING + 0.01, "a point {l} stops over grey reaches {} at full Amount", l + top);
        }
        // a pixel already near white gets little more
        let bright = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![2.2; 96 * 64]).unwrap();
        let rig = bright.rig(&light(), 0.0, 0.0);
        assert!(bright.shade(&rig, 0.5, 0.5).stops < 0.6);
        // nor do the deepest shadows
        let dark = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![-9.0; 96 * 64]).unwrap();
        let rig = dark.rig(&light(), 0.0, 0.0);
        assert!(dark.shade(&rig, 0.5, 0.5).stops < 1.2);
    }

    #[test]
    fn off_is_nothing_and_darken_darkens_what_the_light_misses() {
        let f = two_planes(0.5);
        let off = f.rig(&Light { amount: 0.0, ..light() }, 0.0, 0.0);
        assert_eq!(f.shade(&off, 0.3, 0.3), Shade { stops: 0.0, share: 0.0 });
        let dark = f.rig(&Light { softness: 0.3, ..light() }, 1.0, 0.0);
        assert!(f.shade(&dark, 0.98, 0.05).stops < -0.5, "{:?}", f.shade(&dark, 0.98, 0.05));
        assert!(f.shade(&dark, 0.5, 0.5).stops > 0.0);
    }

    #[test]
    fn the_lights_colour_fades_smoothly_where_darken_meets_the_light() {
        // a bright wall (between the roll-off's start and white): with Darken Surroundings, the gain crosses 1 on
        // the way out from the light, and the light's share (its colour) must not jump there
        let f = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![1.5; 96 * 64]).unwrap();
        let rig = f.rig(&Light { depth: 0.8, softness: 0.1, ..light() }, 0.5, 0.0);
        let row: Vec<Shade> = (0..=400).map(|i| f.shade(&rig, 0.5 + 0.5 * i as f32 / 400.0, 0.5)).collect();
        assert!(row.first().is_some_and(|s| s.stops > 0.0) && row.last().is_some_and(|s| s.stops < 0.0), "{row:?}");
        let jump = row.windows(2).map(|p| (p[1].share - p[0].share).abs()).fold(0f32, f32::max);
        assert!(jump < 0.02, "the share jumps by {jump}");
    }

    #[test]
    fn warmth_is_a_temperature() {
        assert_eq!(warmth_tilt(0.0), 0.0);
        let (warm, cool) = (warmth_tilt(1.0), warmth_tilt(-1.0));
        assert!(warm > 0.8 && warm < 2.5, "{warm}");
        assert!(cool < -0.2 && cool > -1.5, "{cool}");
        assert!(warmth_tilt(0.5) > 0.0 && warmth_tilt(0.5) < warm);
        assert_eq!(warmth_tilt(f64::NAN), 0.0);
    }

    #[test]
    fn hostile_numbers_give_finite_planes() {
        let f = two_planes(0.5);
        let rig = f.rig(
            &Light { x: f64::NAN, y: f64::INFINITY, depth: -5.0, amount: 1e9, softness: f64::NAN, warmth: 9.0, wrap: 7.0, kind: LightKind::Point },
            -3.0,
            f64::NAN,
        );
        let sh = f.shading(&rig);
        let p = sh.planes(7, 5, &|x, y| (x / 7.0, y / 5.0));
        assert!(p.stops.iter().chain(&p.share).all(|v| v.is_finite()));
        let p = sh.planes(3, 2, &|_, _| (f64::NAN, 0.5));
        assert!(p.stops.iter().all(|v| *v == 0.0));
        assert_eq!(sh.planes(0, 0, &|x, y| (x, y)).stops.len(), 0);
    }

    /// A flat wall (two stops under grey) with `lights` (place, Warmth) at Amount 0.5 each, shaded together.
    fn together(f: &Field, lights: &[(f64, f64)]) -> Shading {
        let rigs: Vec<Rig> =
            lights.iter().map(|(x, w)| f.rig(&Light { x: *x, amount: 0.5, warmth: *w, softness: 0.2, ..light() }, 0.0, 0.0)).collect();
        let lobes: Vec<Lobe> = rigs.iter().map(|r| f.lobe(r)).collect();
        let parts: Vec<Part<'_>> = rigs
            .iter()
            .zip(&lobes)
            .zip(lights)
            .map(|((r, l), (_, w))| Part { lobe: l, gain: r.gain(), tilt: warmth_tilt(w / 100.0), tint: 0.0 })
            .collect();
        f.shading_of(&parts, 1.0, 0.0)
    }

    #[test]
    fn two_lights_add_up_and_a_warm_and_a_cool_one_keep_their_own_colour() {
        let f = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![-2.0; 96 * 64]).unwrap();
        let (left, right) = (together(&f, &[(0.2, 100.0)]), together(&f, &[(0.8, -100.0)]));
        let both = together(&f, &[(0.2, 100.0), (0.8, -100.0)]);
        // between them the sum is brighter than either alone, and the lights' own spots keep their colour
        let mid = (both.at(0.5, 0.5), left.at(0.5, 0.5), right.at(0.5, 0.5));
        assert!(mid.0.stops > mid.1.stops.max(mid.2.stops) + 0.05, "{mid:?}");
        assert!(both.tilt_up > 0.0 && both.tilt_down > 0.0);
        let (u, v) = (0.2, 0.5);
        // (at the warm light the warm plane dominates, at the cool one the cool plane)
        let at = |sh: &Shading, u: f32| sh.sample(u * sh.width as f32, v * sh.height as f32);
        let (a, b) = (at(&both, u), at(&both, 0.8));
        assert!(a[1] > 3.0 * a[2] && b[2] > 3.0 * b[1], "{a:?} {b:?}");
        // a lone light gets one plane only
        assert!(left.tilt_down == 0.0 && right.tilt_up == 0.0);
    }

    #[test]
    fn one_light_in_the_set_shades_as_the_light_alone_and_the_total_stays_under_the_ceiling() {
        let f = two_planes(0.5);
        let rig = f.rig(&Light { amount: 0.6, ..light() }, 0.3, 0.0);
        let alone = f.shading(&rig);
        let lobe = f.lobe(&rig);
        let set = f.shading_of(&[Part { lobe: &lobe, gain: rig.gain(), tilt: 1.0, tint: 0.0 }], rig.ambient, rig.ev);
        assert_eq!(alone.px, set.px);
        // many lights on top of each other never take a pixel past the ceiling
        let eight = f.shading_of(&[Part { lobe: &lobe, gain: rig.gain(), tilt: 0.0, tint: 0.0 }; MAX_LIGHTS], 1.0, 0.0);
        let top = (0..96).map(|x| eight.at((x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
        assert!(-2.0 + top <= CEILING + 0.01, "{top}");
        // a lobe of another size adds nothing
        let small = Field::from_parts(8, 8, vec![0.0; 64], vec![-2.0; 64]).unwrap();
        assert_eq!(small.shading_of(&[Part { lobe: &lobe, gain: 7.0, tilt: 0.0, tint: 0.0 }], 1.0, 0.0).up_top, 0.0);
    }

    #[test]
    fn a_point_light_is_the_formula_it_always_was() {
        // (the point light's `falloff · facing` written out as it was before light types: its lobe matches it to the bit)
        let f = two_planes(0.5);
        let rig = f.rig(&Light { softness: 0.3, wrap: 0.4, ..light() }, 0.0, 0.0);
        let lobe = f.lobe(&rig);
        let mut worst = 0u32;
        for i in (0..96 * 64).step_by(7) {
            let (u, v) = (((i % 96) as f32 + 0.5) / 96.0, ((i / 96) as f32 + 0.5) / 64.0);
            let (lz, n, _) = f.at_px(i);
            let p = f.point(u, v, lz);
            let d = [rig.pos[0] - p[0], rig.pos[1] - p[1], rig.pos[2] - p[2]];
            let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let falloff = (rig.r2 / (rig.r2 + r2)).powf(rig.tail);
            let cos = (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / r2.sqrt().max(1e-9);
            let t = ((cos + rig.wrap) / (1.0 + rig.wrap)).clamp(0.0, 1.0);
            let want = (falloff * (1.0 - rig.face + rig.face * t * t * t) / rig.peak).min(1.0);
            worst = worst.max(want.to_bits().abs_diff(lobe.m[i].to_bits()));
        }
        assert_eq!(worst, 0);
    }
}
