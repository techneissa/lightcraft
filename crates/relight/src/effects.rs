//! A light's effects (issue #302, `fork/relight.md`): a gel, a pattern and haze. All of it is original work, drawn
//! in code from numbers; no file is read.
//!
//! - **Gel:** a colour for the light, any hue and a strength. It joins the light's Warmth: [`Effects::light_colour`]
//!   multiplies the two colours (scene-linear, the way a gel in front of a warm lamp does) and gives the finish
//!   stage's own local temperature and tint, so a gel travels the same planes Warmth does, on the CPU and the GPU
//!   alike (the finish stage has one kernel for both).
//! - **Pattern:** the light shines through blinds, window panes, leaves or dapple, laid onto the surfaces like a
//!   projector's slide ([`Field::pattern_plane`]): a surface point takes the pattern's value at the direction it
//!   lies from the light, so the stripes run straight over a wall, converge over a floor and jump where depth steps.
//!   The pattern multiplies the light's lobe, so the facing term, the roll-off and the colour work as before.
//! - **Haze:** a soft glow around the light that grows with the distance in depth ([`Field::haze_lobe`]): the light
//!   scatters in the air, and a thicker stretch of air lies in front of what is far. It is one more lobe of the same
//!   light (and the same colour), so it adds to the light the way a second light does.

use rayon::prelude::*;

use crate::field::Field;
use crate::light::{Light, Lobe, Rig, warmth_tilt};

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
}

impl Default for Effects {
    fn default() -> Self {
        Self { gel_hue: 0.0, gel_strength: 0.0, pattern: Pattern::None, scale: 0.5, rotation: 0.0, softness: 0.3, haze: 0.0 }
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

    /// The pattern's cells per unit of tangent (the lens spans about 1.15): from 30 (Scale 0) to 3 (Scale 1).
    fn frequency(&self) -> f32 {
        30.0 * 10f32.powf(-unit(self.scale, 0.5))
    }

    /// The pattern's edge half-width in cells before the surface's tilt widens it.
    fn edge(&self) -> f32 {
        let s = unit(self.softness, 0.3);
        0.02 + 0.4 * s * s
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 1 inside distance `r`, 0 beyond `r + w`, soft between `r − w` and `r + w`.
fn soft_edge(d: f32, r: f32, w: f32) -> f32 {
    1.0 - smoothstep(r - w, r + w, d)
}

fn hash(ix: i32, iy: i32, salt: u32) -> u32 {
    let mut h = (ix as u32).wrapping_mul(0x9E37_79B1) ^ (iy as u32).wrapping_mul(0x85EB_CA77) ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// A number in 0..1 from a hash.
fn rand(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

/// How much of the light a pattern lets through at `(u, v)`, in cells (0 shadow .. 1 light), with edges `w` cells
/// wide. Drawn here; nothing is read.
pub fn transmission(pattern: Pattern, u: f32, v: f32, w: f32) -> f32 {
    let t = match pattern {
        Pattern::None => 1.0,
        Pattern::Blinds => soft_edge((v - v.floor() - 0.5).abs(), 0.25, w),
        Pattern::Panes => {
            // panes twice the size of the blinds' slats, with bars 0.14 of a pane wide
            let axis = |c: f32| soft_edge((c - c.floor() - 0.5).abs(), 0.43, (0.5 * w).min(0.07));
            axis(0.5 * u) * axis(0.5 * v)
        }
        Pattern::Leaves | Pattern::Dapple => cell_pattern(pattern, u, v, w, None),
    };
    if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 }
}

/// Leaves and dapple at `(u, v)` (cells): made of shapes scattered one or two to a cell, which is dear per pixel, so
/// the field's pattern bakes them into a texture ([`Baked`]) that repeats every `period` cells.
fn cell_pattern(pattern: Pattern, u: f32, v: f32, w: f32, period: Option<i32>) -> f32 {
    match pattern {
        Pattern::Leaves => 1.0 - cover(u, v, w, 2, period, leaf),
        _ => 0.08 + 0.92 * cover(u, v, w, 1, period, spot),
    }
}

/// Cells a baked texture repeats over, and its texels per cell (a texel is a 24th of a cell: an edge sharper than that
/// reads as that soft).
const TILE: usize = 16;
const TEXELS: usize = 24;

/// A leaves or dapple pattern baked into a tileable texture: at its own softness (`sharp`) and with edges three times as
/// wide (`wide`, which a surface seen at a grazing angle takes, in part), and its mean.
struct Baked {
    n: usize,
    sharp: Vec<f32>,
    wide: Vec<f32>,
    mean: f32,
}

static BAKED: std::sync::Mutex<Vec<((Pattern, u32), std::sync::Arc<Baked>)>> = std::sync::Mutex::new(Vec::new());

impl Baked {
    fn make(pattern: Pattern, edge: f32) -> Baked {
        let n = TILE * TEXELS;
        let draw = |w: f32| -> Vec<f32> {
            let mut t = vec![0f32; n * n];
            t.par_chunks_mut(n).enumerate().for_each(|(y, row)| {
                for (x, out) in row.iter_mut().enumerate() {
                    let (u, v) = ((x as f32 + 0.5) / TEXELS as f32, (y as f32 + 0.5) / TEXELS as f32);
                    *out = cell_pattern(pattern, u, v, w, Some(TILE as i32)).clamp(0.0, 1.0);
                }
            });
            t
        };
        let sharp = draw(edge);
        let mean = sharp.iter().sum::<f32>() / sharp.len().max(1) as f32;
        Baked { n, sharp, wide: draw(3.0 * edge), mean }
    }

    /// The one for `pattern` at `edge`, made once and kept (the newest four).
    fn of(pattern: Pattern, edge: f32) -> std::sync::Arc<Baked> {
        let key = (pattern, edge.to_bits());
        if let Some((_, b)) = BAKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|(k, _)| *k == key) {
            return b.clone();
        }
        let b = std::sync::Arc::new(Baked::make(pattern, edge));
        let mut kept = BAKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        kept.retain(|(k, _)| *k != key);
        kept.insert(0, (key, b.clone()));
        kept.truncate(4);
        b
    }

    /// The texture `tex` at `(u, v)` in cells, bilinear and repeating.
    fn sample(&self, tex: &[f32], u: f32, v: f32) -> f32 {
        let n = self.n;
        let period = TILE as f32;
        let wrapped = |c: f32| (c - period * (c / period).floor()) * TEXELS as f32 - 0.5;
        let (fx, fy) = (wrapped(u), wrapped(v));
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |x: f32, y: f32| {
            let (xi, yi) = ((x as i64).rem_euclid(n as i64) as usize, (y as i64).rem_euclid(n as i64) as usize);
            tex.get(yi * n + xi).copied().unwrap_or(1.0)
        };
        let top = at(x0, y0) + (at(x0 + 1.0, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y0 + 1.0) + (at(x0 + 1.0, y0 + 1.0) - at(x0, y0 + 1.0)) * tx;
        top + (bottom - top) * ty
    }
}

/// The largest of `per_cell` shapes' coverage from the cell of `(u, v)` and its eight neighbours. With `period`
/// the cells' shapes repeat every `period` cells (the neighbours' hashes wrap), which is how a pattern is baked into a
/// texture that tiles.
fn cover(u: f32, v: f32, w: f32, per_cell: u32, period: Option<i32>, shape: impl Fn([f32; 2], u32, f32) -> f32) -> f32 {
    let (ix, iy) = (u.floor() as i32, v.floor() as i32);
    let wrap = |i: i32| period.map_or(i, |p| i.rem_euclid(p.max(1)));
    let mut best = 0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (ix.wrapping_add(dx), iy.wrapping_add(dy));
            for k in 0..per_cell {
                let h = hash(wrap(cx), wrap(cy), k);
                let centre = [cx as f32 + 0.15 + 0.7 * rand(h), cy as f32 + 0.15 + 0.7 * rand(h.rotate_left(11))];
                best = best.max(shape([u - centre[0], v - centre[1]], hash(h as i32, k as i32, 7), w));
            }
        }
    }
    best
}

/// A leaf (pointed at both ends, 1.25 cells long) at offset `p` from its centre, turned and sized by `h`.
fn leaf(p: [f32; 2], h: u32, w: f32) -> f32 {
    let (angle, size) = (rand(h) * std::f32::consts::PI, 0.8 + 0.5 * rand(h.rotate_left(13)));
    let (s, c) = angle.sin_cos();
    let q = [(p[0] * c + p[1] * s) / size, (-p[0] * s + p[1] * c) / size];
    let (a, b) = (0.62, 0.26);
    let x = q[0].abs();
    let sd = if x < a { q[1].abs() - b * (1.0 - (x / a) * (x / a)) } else { (x - a).hypot(q[1]) };
    1.0 - smoothstep(-w, w, sd * size)
}

/// A round spot of light of radius 0.16..0.42 cells at offset `p` from its centre.
fn spot(p: [f32; 2], h: u32, w: f32) -> f32 {
    let r = 0.16 + 0.26 * rand(h);
    soft_edge(p[0].hypot(p[1]), r, w)
}

impl Field {
    /// The pattern of effects `e` over the field, laid on the surfaces as the light would throw it (`rig`: the light
    /// set up on this field): 0 where it shades, 1 where it lets the light through, one value per field pixel. All
    /// 1 for no pattern. A surface beside or behind the light's plane gets the pattern's mean coverage.
    pub fn pattern_plane(&self, rig: &Rig, e: &Effects) -> Vec<f32> {
        let (w, h) = (self.width, self.height);
        let n = w * h;
        if e.pattern == Pattern::None {
            return vec![1.0; n];
        }
        let pos = rig.position();
        // aimed into the picture, or out of it for a light behind most of the scene
        let step = (n / 2048).max(1);
        let (sum, count) = (0..n).step_by(step).fold((0f32, 0f32), |(s, c), i| (s + self.at_px(i).0.exp(), c + 1.0));
        let aim = if sum / count.max(1.0) >= pos[2] { 1.0f32 } else { -1.0 };
        let (freq, edge) = (e.frequency(), e.edge());
        let turn = if e.rotation.is_finite() { (e.rotation % 360.0).to_radians() as f32 } else { 0.0 };
        let (sin, cos) = turn.sin_cos();
        let baked = matches!(e.pattern, Pattern::Leaves | Pattern::Dapple).then(|| Baked::of(e.pattern, edge));
        // (past the light's plane the pattern fades to its own mean coverage: half for blinds, the bars' share for
        // panes, what the baked texture holds for the others)
        let mean = match (&baked, e.pattern) {
            (Some(b), _) => b.mean,
            (None, Pattern::Panes) => 0.86 * 0.86,
            _ => 0.5,
        };
        let mut plane = vec![1f32; n];
        plane.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, out) in row.iter_mut().enumerate() {
                let (lz, normal, _) = self.at_px(y * w + x);
                let p = self.point((x as f32 + 0.5) / w as f32, v, lz);
                let d = [p[0] - pos[0], p[1] - pos[1], p[2] - pos[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
                let dz = aim * d[2];
                // (beside or behind the light's own plane the slide's picture is stretched past use: fade to its mean)
                let ahead = smoothstep(0.03, 0.2, dz / len);
                let (tx, ty) = ((d[0] / dz.max(1e-6)).clamp(-16.0, 16.0), (d[1] / dz.max(1e-6)).clamp(-16.0, 16.0));
                let (cu, cv) = ((tx * cos - ty * sin) * freq, (tx * sin + ty * cos) * freq);
                // (a surface seen at a grazing angle to the light smears the pattern: its edges widen)
                let incidence = (-(normal[0] * d[0] + normal[1] * d[1] + normal[2] * d[2]) / len).abs().clamp(0.35, 1.0);
                let t = match &baked {
                    Some(b) => {
                        let widen = ((1.0 / incidence - 1.0) / 1.86).clamp(0.0, 1.0);
                        let sharp = b.sample(&b.sharp, cu, cv);
                        sharp + (b.sample(&b.wide, cu, cv) - sharp) * widen
                    }
                    None => transmission(e.pattern, cu, cv, edge / incidence),
                };
                let value = mean + (t - mean) * ahead;
                *out = if value.is_finite() { value } else { 1.0 };
            }
        });
        plane
    }

    /// `lobe` seen through the pattern of effects `e` (see [`Field::pattern_plane`]).
    pub fn patterned(&self, rig: &Rig, mut lobe: Lobe, e: &Effects) -> Lobe {
        if e.pattern == Pattern::None || lobe.m.len() != self.width * self.height {
            return lobe;
        }
        for (m, t) in lobe.m.iter_mut().zip(self.pattern_plane(rig, e)) {
            *m *= t;
        }
        lobe
    }

    /// The glow of `light`'s haze: a soft halo around where the light sits on the photo, as strong as
    /// the distance in depth in front of a pixel is long (nothing at the nearest depth, full at the farthest).
    /// 0..1 per field pixel; the Amount of the glow is the caller's ([`Effects::haze`], [`HAZE_GAIN`]).
    pub fn haze_lobe(&self, light: &Light) -> Lobe {
        let (w, h) = (self.width, self.height);
        let f = |v: f64, d: f32| if v.is_finite() { v.clamp(-0.5, 1.5) as f32 } else { d };
        let (lx, ly) = (f(light.x, 0.5), f(light.y, 0.5));
        let soft = unit(light.softness, 0.5);
        let radius = 0.06 + 0.3 * soft;
        let long = w.max(h).max(1) as f32;
        let span = (self.far - self.near).max(1e-3);
        let mut m = vec![0f32; w * h];
        m.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
            let dy = ((y as f32 + 0.5) / h as f32 - ly) * h as f32 / long;
            for (x, out) in row.iter_mut().enumerate() {
                let dx = ((x as f32 + 0.5) / w as f32 - lx) * w as f32 / long;
                let r = dx.hypot(dy) / radius;
                let glow = (1.0 + r * r).powf(-1.5);
                let (lz, _, _) = self.at_px(y * w + x);
                let far = ((lz - self.near) / span).clamp(0.0, 1.0);
                let fog = (1.0 - (-2.5 * far).exp()) / (1.0 - (-2.5f32).exp());
                let v = glow * fog;
                *out = if v.is_finite() { v } else { 0.0 };
            }
        });
        Lobe { m }
    }
}

/// The strength haze 1 gives the glow, relative to its light's own strength (`Part::gain`): at full Amount and full haze
/// the glow's core over the farthest things lights about a stop and a quarter.
pub const HAZE_GAIN: f32 = 0.25;

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(f.pattern_plane(&rig, &e).iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        }
        assert!(f.haze_lobe(&Light { x: f64::NAN, y: f64::INFINITY, softness: f64::NAN, ..light() }).m.iter().all(|v| v.is_finite()));
        for pattern in [Pattern::Blinds, Pattern::Panes, Pattern::Leaves, Pattern::Dapple] {
            for (u, v) in [(f32::NAN, 0.0), (1e30, -1e30), (f32::INFINITY, 2.0)] {
                assert!(transmission(pattern, u, v, f32::NAN).is_finite());
            }
        }
    }

    /// The row indices where the plane at column `x` crosses 0.5 going up or down.
    fn edges(plane: &[f32], w: usize, h: usize, x: usize) -> Vec<usize> {
        (1..h).filter(|&y| (plane[(y - 1) * w + x] > 0.5) != (plane[y * w + x] > 0.5)).collect()
    }

    #[test]
    fn blinds_alternate_on_a_flat_wall_and_bend_on_a_tilted_floor() {
        let e = Effects { pattern: Pattern::Blinds, scale: 0.6, softness: 0.1, ..Effects::default() };
        // a wall facing the camera: the stripes are level and evenly spaced
        let wall = flat(|_, _| 0.0);
        let rig = wall.rig(&light(), 0.0, 0.0);
        let plane = wall.pattern_plane(&rig, &e);
        let (w, h) = (wall.width, wall.height);
        let ed = edges(&plane, w, h, w / 2);
        assert!(ed.len() >= 6, "{ed:?}");
        let gaps: Vec<usize> = ed.windows(2).map(|p| p[1] - p[0]).collect();
        let (lo, hi) = (gaps.iter().min().copied().unwrap_or(0), gaps.iter().max().copied().unwrap_or(0));
        assert!(hi <= lo + 2, "uneven stripes {gaps:?}");
        // level: the same rows (give or take one) at another column
        let other = edges(&plane, w, h, 3 * w / 4);
        assert!(ed.len() == other.len() && ed.iter().zip(&other).all(|(a, b)| a.abs_diff(*b) <= 1), "{ed:?} {other:?}");
        // lit and unlit both appear, in about equal shares
        let lit = plane.iter().filter(|v| **v > 0.5).count() as f32 / plane.len() as f32;
        assert!((0.35..0.65).contains(&lit), "{lit}");
        // a floor that recedes down the picture (the bottom is the far side): the stripes, level and evenly spaced on the
        // wall, spread out steadily towards the far side as the surface's distance from the light grows
        let floor = flat(|_, y| -0.5 + 1.0 * y as f32 / 128.0);
        let rig = floor.rig(&Light { depth: 0.0, ..light() }, 0.0, 0.0);
        let plane = floor.pattern_plane(&rig, &e);
        let ed = edges(&plane, w, h, w / 2);
        assert!(ed.len() >= 4, "{ed:?}");
        let gaps: Vec<usize> = ed.windows(2).map(|p| p[1] - p[0]).collect();
        let (first, last) = (gaps.first().copied().unwrap_or(0), gaps.last().copied().unwrap_or(0));
        assert!(last > first + first / 2, "the stripes don't spread out towards the far side {gaps:?}");
        assert!(gaps.windows(2).all(|p| p[1] + 1 >= p[0]), "the stripes' spacing doesn't grow steadily {gaps:?}");
    }

    #[test]
    fn a_step_in_depth_moves_the_stripes() {
        // the same wall with its right half 40 % farther: the stripes jump at the step
        let e = Effects { pattern: Pattern::Blinds, scale: 0.6, softness: 0.1, ..Effects::default() };
        let f = flat(|x, _| if x < 96 { 0.0 } else { 0.4 });
        let plane = f.pattern_plane(&f.rig(&light(), 0.0, 0.0), &e);
        let (w, h) = (f.width, f.height);
        // each side's stripes are level on their own, and the far side's are somewhere else
        let (near_a, near_b, far_a, far_b) = (edges(&plane, w, h, 20), edges(&plane, w, h, 80), edges(&plane, w, h, 120), edges(&plane, w, h, 180));
        let same = |a: &[usize], b: &[usize]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 1);
        assert!(same(&near_a, &near_b) && same(&far_a, &far_b), "{near_a:?} {near_b:?} {far_a:?} {far_b:?}");
        assert!(!same(&near_a, &far_a), "the step moved nothing: {near_a:?} {far_a:?}");
    }

    #[test]
    fn beside_the_lights_plane_a_pattern_fades_to_its_own_mean_coverage() {
        // the light at the far end of this scene sits at log depth 0 (the farthest thing), so the right half lies in the
        // light's own plane and gets each pattern's mean; the left half, nearer, gets the pattern
        let f = flat(|x, _| if x < 96 { -0.6 } else { 0.0 });
        let rig = f.rig(&Light { depth: 1.0, ..light() }, 0.0, 0.0);
        let at = |plane: &[f32], xs: std::ops::Range<usize>| -> Vec<f32> {
            (0..128).flat_map(|y| xs.clone().map(move |x| (x, y))).map(|(x, y)| plane[y * 192 + x]).collect()
        };
        let spread = |v: &[f32]| v.iter().copied().fold(0.0, f32::max) - v.iter().copied().fold(f32::MAX, f32::min);
        for (pattern, mean) in [(Pattern::Blinds, Some(0.5)), (Pattern::Panes, Some(0.86 * 0.86)), (Pattern::Leaves, None), (Pattern::Dapple, None)] {
            let plane = f.pattern_plane(&rig, &Effects { pattern, ..Effects::default() });
            let beside = at(&plane, 130..190);
            assert!(spread(&beside) < 1e-4, "{pattern:?}: the plane's own side isn't flat: {}", spread(&beside));
            match mean {
                Some(m) => assert!((beside[0] - m).abs() < 1e-3, "{pattern:?}: {} is not {m}", beside[0]),
                None => assert!((0.3..0.95).contains(&beside[0]), "{pattern:?}: {}", beside[0]),
            }
            assert!(spread(&at(&plane, 5..60)) > 0.5, "{pattern:?} shows no pattern in front of the light");
        }
    }

    #[test]
    fn every_pattern_lets_light_through_and_shades_some() {
        let f = flat(|_, _| 0.0);
        let rig = f.rig(&light(), 0.0, 0.0);
        for pattern in [Pattern::Blinds, Pattern::Panes, Pattern::Leaves, Pattern::Dapple] {
            let plane = f.pattern_plane(&rig, &Effects { pattern, ..Effects::default() });
            let lit = plane.iter().filter(|v| **v > 0.6).count() as f32 / plane.len() as f32;
            let shade = plane.iter().filter(|v| **v < 0.4).count() as f32 / plane.len() as f32;
            assert!(lit > 0.08 && shade > 0.08, "{pattern:?}: lit {lit}, shaded {shade}");
        }
        assert!(f.pattern_plane(&rig, &Effects::default()).iter().all(|v| *v == 1.0));
    }

    #[test]
    fn haze_brightens_far_pixels_near_the_light_more_than_near_ones() {
        // the left half near, the right half far; the light in the middle of the picture
        let f = flat(|x, _| if x < 96 { -0.5 } else { 0.5 });
        let lobe = f.haze_lobe(&light());
        let at = |x: usize, y: usize| lobe.m[y * f.width + x];
        // the same distance from the light on both sides
        let (near, far) = (at(96 - 12, 64), at(96 + 11, 64));
        assert!(far > 2.0 * near, "near {near}, far {far}");
        // it falls away from the light
        assert!(at(180, 64) < far && far <= 1.0);
        // and through a shading it brightens the far side more
        let rig = f.rig(&light(), 0.0, 0.0);
        let sh = f.shading_of(&[crate::Part { lobe: &lobe, gain: rig.gain() * HAZE_GAIN, tilt: 0.0, tint: 0.0 }], 1.0, 0.0);
        assert!(sh.at(0.5 + 11.0 / 192.0, 0.5).stops > sh.at(0.5 - 12.0 / 192.0, 0.5).stops);
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
