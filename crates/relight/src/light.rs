//! The light: a small round source at a point in the scene. Each pixel's surface point (from the field's depth,
//! through a 60° lens) gets light by its 3D distance to the source, as from a sphere of the Softness's size (an
//! inverse square that stays finite near it), and by how its surface faces it (a gentle term: a surface turned
//! away keeps part of it, so the depth model's small errors never show as lighting texture). The result is a gain
//! on scene-linear light: `ambient + amount · light`, normalized so the most lit point of the photo gets the
//! Amount; the gain rolls off towards white (a bright pixel can't be pushed past it) and is held back in the
//! deepest shadows (their noise is not lifted). The light's colour is a temperature (Warmth), given only to the
//! light added.

use rayon::prelude::*;

use crate::field::Field;

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
    /// How much the rest of the photo darkens, 0..1 (1: by more than three stops).
    pub darken: f64,
    /// How far light wraps around shapes, 0 (follows them closely) .. 1 (ignores them).
    pub wrap: f64,
}

impl Light {
    /// Whether it changes nothing.
    pub fn is_off(&self) -> bool {
        !(self.amount > 0.0 || self.darken > 0.0)
    }
}

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

/// A light set up on a field: the numbers every pixel's shading needs.
#[derive(Clone, Debug)]
pub struct Rig {
    pos: [f32; 3],
    /// The source's radius, squared.
    r2: f32,
    /// How fast the light falls off past the radius (an exponent: a tight light falls off faster than a broad one).
    tail: f32,
    amount: f32,
    ambient: f32,
    /// How much of the light the facing term decides, and how far it wraps past 90°.
    face: f32,
    wrap: f32,
    /// The largest `falloff · facing` of the photo (the Amount is given to it).
    peak: f32,
    /// The photo's exposure (stops): the guide's luminance plus this is the pixel's.
    ev: f32,
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
    /// Set up `light` on this field, for a photo at exposure `ev` (stops).
    pub fn rig(&self, light: &Light, ev: f64) -> Rig {
        let f = |v: f64, lo: f64, hi: f64, d: f32| if v.is_finite() { v.clamp(lo, hi) as f32 } else { d };
        let (x, y) = (f(light.x, -0.5, 1.5, 0.5), f(light.y, -0.5, 1.5, 0.5));
        let depth = f(light.depth, 0.0, 1.0, 0.3);
        // from in front of the nearest thing (55 % of its distance) to the farthest
        let near = self.near - 0.6;
        let lz = near + depth * (self.far - near).max(0.1);
        let pos = self.point(x, y, lz);
        let zl = lz.exp();
        let soft = f(light.softness, 0.0, 1.0, 0.5);
        let radius = zl * (0.03 + 1.4 * soft * soft);
        let wrap = f(light.wrap, 0.0, 1.0, 0.4);
        let mut rig = Rig {
            pos,
            r2: radius * radius,
            amount: f(light.amount, 0.0, 1.0, 0.0) * PEAK_GAIN,
            tail: 1.0 + 1.5 * (1.0 - soft),
            ambient: 1.0 - DARKEN_MAX * f(light.darken, 0.0, 1.0, 0.0),
            face: 1.0 - wrap,
            wrap: 0.1 + 0.9 * wrap,
            peak: 1.0,
            ev: if ev.is_finite() { ev.clamp(-10.0, 10.0) as f32 } else { 0.0 },
        };
        // the most lit point, on a grid of the photo
        let (gw, gh) = (64usize, 48usize);
        let peak = (0..gw * gh)
            .into_par_iter()
            .map(|i| {
                let (u, v) = (((i % gw) as f32 + 0.5) / gw as f32, ((i / gw) as f32 + 0.5) / gh as f32);
                let (lz, n, _) = self.at(u, v);
                self.light_at(&rig, u, v, lz, n)
            })
            .reduce(|| 0.0, f32::max);
        rig.peak = peak.max(1e-6);
        rig
    }

    /// `falloff · facing` at `(u, v)` with log depth `lz` and normal `n`, before the peak's normalization.
    #[inline]
    fn light_at(&self, rig: &Rig, u: f32, v: f32, lz: f32, n: [f32; 3]) -> f32 {
        let p = self.point(u, v, lz);
        let d = [rig.pos[0] - p[0], rig.pos[1] - p[1], rig.pos[2] - p[2]];
        let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let falloff = (rig.r2 / (rig.r2 + r2)).powf(rig.tail);
        let r = r2.sqrt().max(1e-9);
        let cos = (n[0] * d[0] + n[1] * d[1] + n[2] * d[2]) / r;
        let t = ((cos + rig.wrap) / (1.0 + rig.wrap)).clamp(0.0, 1.0);
        let facing = 1.0 - rig.face + rig.face * t * t * t;
        let v = falloff * facing;
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
        let gain = rig.ambient + added;
        let raw = gain.max(1e-3).log2();
        let l = lum + rig.ev;
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

/// The light over the whole field: each field pixel's [`Shade`]. Made once per light (a drag makes one per
/// position) and read by every render of it, whatever its size or window.
///
/// Along a step in depth (the field's edge band) the light jumps from one side's to the other's; there it is
/// refined by a guided filter against the photo's luminance (a local linear fit over a small window), so it changes
/// where and as softly as the photo's own edge does: a sharp outline stays sharp, a soft one (out of focus, hair)
/// mixes the two sides' light as its pixels mix the two sides.
#[derive(Clone, Debug)]
pub struct Shading {
    pub width: usize,
    pub height: usize,
    /// Each pixel's gain (stops) and the light's share.
    px: Vec<[f32; 2]>,
    /// The largest brightening and darkening (stops, both ≥ 0).
    pub up_top: f32,
    pub down_top: f32,
}

/// A render's planes of the light, finished for the finish stage ([`Shading::split`]): the brightening gain over
/// [`Shading::up_top`], the darkening over [`Shading::down_top`] (each 0..1), and the light's share (0..1).
pub struct Split {
    pub width: usize,
    pub height: usize,
    pub up: Vec<f32>,
    pub down: Vec<f32>,
    pub share: Option<Vec<f32>>,
}

/// The guided filter's regularization along an edge (a variance of the cube-root luminance).
const EDGE_EPS: f32 = 1e-3;

impl Field {
    /// The light of `rig` over the field.
    pub fn shading(&self, rig: &Rig) -> Shading {
        let (w, h) = (self.width, self.height);
        let mut stops = vec![0f32; w * h];
        let mut share = vec![0f32; w * h];
        stops.par_chunks_mut(w.max(1)).zip(share.par_chunks_mut(w.max(1))).enumerate().for_each(|(y, (st, sh))| {
            let v = (y as f32 + 0.5) / h as f32;
            for (x, (a, b)) in st.iter_mut().zip(sh.iter_mut()).enumerate() {
                // (at the field's own pixels: its values as they are, nothing to interpolate)
                let (lz, n, lum) = self.at_px(y * w + x);
                let s = self.shade_with(rig, ((x as f32 + 0.5) / w as f32, v), lz, n, lum);
                *a = s.stops;
                *b = s.share;
            }
        });
        let r = crate::field::BAND_RADIUS;
        let refine = |p: &[f32]| -> Vec<(usize, f32)> {
            if self.band.is_empty() {
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
                    q.is_finite().then_some((i, q))
                })
                .collect()
        };
        for (i, q) in refine(&stops) {
            if let Some(v) = stops.get_mut(i) {
                *v = q;
            }
        }
        for (i, q) in refine(&share) {
            if let Some(v) = share.get_mut(i) {
                *v = q.clamp(0.0, 1.0);
            }
        }
        let up_top = stops.iter().copied().fold(0f32, f32::max);
        let down_top = stops.iter().copied().fold(0f32, |a, v| a.max(-v));
        let px = stops.into_iter().zip(share).map(|(a, b)| [a, b]).collect();
        Shading { width: w, height: h, px, up_top, down_top }
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
        self.px.len() * 8
    }

    /// The light at normalized photo coordinates `(u, v)` (bilinear, clamped at the borders).
    #[inline]
    pub fn at(&self, u: f32, v: f32) -> Shade {
        let [stops, share] = self.sample(u * self.width as f32, v * self.height as f32);
        Shade { stops, share }
    }

    /// Both values at field position `(x, y)` (pixel centres at `i + 0.5`), bilinear, clamped.
    #[inline]
    fn sample(&self, x: f32, y: f32) -> [f32; 2] {
        let (w, h) = (self.width, self.height);
        let (Some(xm), Some(ym)) = (w.checked_sub(1), h.checked_sub(1)) else { return [0.0; 2] };
        let fx = (x - 0.5).clamp(0.0, xm as f32);
        let fy = (y - 0.5).clamp(0.0, ym as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(xm), (y0 + 1).min(ym));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.px.get(y * w + x).copied().unwrap_or([0.0; 2]);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let lerp = |k: usize| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        };
        [lerp(0), lerp(1)]
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
    /// centre to normalized photo coordinates. `share`: whether the light's colour plane is wanted.
    pub fn split(&self, w: usize, h: usize, to_photo: &(dyn Fn(f64, f64) -> (f64, f64) + Sync), share: bool) -> Split {
        let n = w.saturating_mul(h);
        let (ku, kd) = (if self.up_top > 0.0 { 1.0 / self.up_top } else { 0.0 }, if self.down_top > 0.0 { 1.0 / self.down_top } else { 0.0 });
        let (fw, fh) = (self.width as f32, self.height as f32);
        let mut up = vec![0f32; n];
        let mut down = vec![0f32; n];
        let mut sh = if share { vec![0f32; n] } else { Vec::new() };
        let row = w.max(1);
        let fill = |y: usize, ur: &mut [f32], dr: &mut [f32], mut sr: Option<&mut [f32]>| {
            for x in 0..ur.len() {
                let (u, v) = to_photo(x as f64 + 0.5, y as f64 + 0.5);
                if !(u.is_finite() && v.is_finite()) {
                    continue;
                }
                let [stops, s] = self.sample(u as f32 * fw, v as f32 * fh);
                if let (Some(a), Some(b)) = (ur.get_mut(x), dr.get_mut(x)) {
                    (*a, *b) = ((stops * ku).clamp(0.0, 1.0), (-stops * kd).clamp(0.0, 1.0));
                }
                if let Some(c) = sr.as_deref_mut().and_then(|r| r.get_mut(x)) {
                    *c = s.clamp(0.0, 1.0);
                }
            }
        };
        if share {
            up.par_chunks_mut(row)
                .zip(down.par_chunks_mut(row))
                .zip(sh.par_chunks_mut(row))
                .enumerate()
                .for_each(|(y, ((ur, dr), sr))| fill(y, ur, dr, Some(sr)));
        } else {
            up.par_chunks_mut(row).zip(down.par_chunks_mut(row)).enumerate().for_each(|(y, (ur, dr))| fill(y, ur, dr, None));
        }
        Split { width: w, height: h, up, down, share: share.then_some(sh) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> Light {
        Light { x: 0.5, y: 0.5, depth: 0.0, amount: 1.0, softness: 0.4, warmth: 0.0, darken: 0.0, wrap: 0.0 }
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
        let rig = f.rig(&light(), 0.0);
        let (near, far) = (f.shade(&rig, 0.4, 0.5), f.shade(&rig, 0.6, 0.5));
        assert!(near.stops > far.stops + 0.3, "near {near:?} far {far:?}");
        // and farther from the light it falls off on the same plane
        let edge = f.shade(&rig, 0.05, 0.5);
        assert!(edge.stops < near.stops, "{edge:?} {near:?}");
        // a light placed far back lights the far half more
        let back = f.rig(&Light { depth: 1.0, softness: 0.1, ..light() }, 0.0);
        assert!(f.shade(&back, 0.6, 0.5).stops > f.shade(&back, 0.4, 0.5).stops);
    }

    #[test]
    fn surfaces_facing_the_light_brighten_more_than_those_turned_away() {
        // a ridge: log depth grows away from the middle column, so the left side faces left and the right side right
        let (w, h) = (96, 64);
        let lz: Vec<f32> = (0..w * h).map(|i| 0.02 * ((i % w) as f32 - w as f32 / 2.0).abs()).collect();
        let f = Field::from_parts(w, h, lz, vec![0.0; w * h]).unwrap();
        // the light far to the left, level with the ridge
        let rig = f.rig(&Light { x: -0.4, depth: 0.3, softness: 1.0, ..light() }, 0.0);
        let (left, right) = (f.shade(&rig, 0.45, 0.5), f.shade(&rig, 0.55, 0.5));
        assert!(left.stops > right.stops * 1.3, "{left:?} {right:?}");
        // with the light wrapping fully, the facing makes no difference
        let flat = f.rig(&Light { x: -0.4, depth: 0.3, softness: 1.0, wrap: 1.0, ..light() }, 0.0);
        let (l, r) = (f.shade(&flat, 0.45, 0.5), f.shade(&flat, 0.55, 0.5));
        assert!((l.stops - r.stops).abs() < 0.15 * l.stops.max(r.stops), "{l:?} {r:?}");
    }

    #[test]
    fn the_amount_goes_to_the_most_lit_point_and_white_is_not_passed() {
        let f = two_planes(0.5);
        let rig = f.rig(&light(), 0.0);
        let best = (0..96).map(|x| f.shade(&rig, (x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
        assert!((best - 3.0).abs() < 0.2, "{best}");
        // a mid-grey point at the peak gains most of it (not two stops: the roll-off is for highlights)
        let grey = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![0.0; 96 * 64]).unwrap();
        let rig = grey.rig(&light(), 0.0);
        let best = (0..96).map(|x| grey.shade(&rig, (x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
        assert!(best >= 2.2, "mid-grey gains {best} stops at full Amount");
        // and skin tones a half and a whole stop over grey stay at or under the ceiling
        for l in [0.5f32, 1.0] {
            let f = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![l; 96 * 64]).unwrap();
            let rig = f.rig(&light(), 0.0);
            let top = (0..96).map(|x| f.shade(&rig, (x as f32 + 0.5) / 96.0, 0.5).stops).fold(0f32, f32::max);
            assert!(l + top <= CEILING + 0.01, "a point {l} stops over grey reaches {} at full Amount", l + top);
        }
        // a pixel already near white gets little more
        let bright = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![2.2; 96 * 64]).unwrap();
        let rig = bright.rig(&light(), 0.0);
        assert!(bright.shade(&rig, 0.5, 0.5).stops < 0.6);
        // nor do the deepest shadows
        let dark = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![-9.0; 96 * 64]).unwrap();
        let rig = dark.rig(&light(), 0.0);
        assert!(dark.shade(&rig, 0.5, 0.5).stops < 1.2);
    }

    #[test]
    fn off_is_nothing_and_darken_darkens_what_the_light_misses() {
        let f = two_planes(0.5);
        let off = f.rig(&Light { amount: 0.0, ..light() }, 0.0);
        assert_eq!(f.shade(&off, 0.3, 0.3), Shade { stops: 0.0, share: 0.0 });
        let dark = f.rig(&Light { softness: 0.3, darken: 1.0, ..light() }, 0.0);
        assert!(f.shade(&dark, 0.98, 0.05).stops < -0.5, "{:?}", f.shade(&dark, 0.98, 0.05));
        assert!(f.shade(&dark, 0.5, 0.5).stops > 0.0);
    }

    #[test]
    fn the_lights_colour_fades_smoothly_where_darken_meets_the_light() {
        // a bright wall (between the roll-off's start and white): with Darken Surroundings, the gain crosses 1 on
        // the way out from the light, and the light's share (its colour) must not jump there
        let f = Field::from_parts(96, 64, vec![0.0; 96 * 64], vec![1.5; 96 * 64]).unwrap();
        let rig = f.rig(&Light { depth: 0.8, softness: 0.1, darken: 0.5, ..light() }, 0.0);
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
            &Light { x: f64::NAN, y: f64::INFINITY, depth: -5.0, amount: 1e9, softness: f64::NAN, warmth: 9.0, darken: -3.0, wrap: 7.0 },
            f64::NAN,
        );
        let sh = f.shading(&rig);
        let p = sh.planes(7, 5, &|x, y| (x / 7.0, y / 5.0));
        assert!(p.stops.iter().chain(&p.share).all(|v| v.is_finite()));
        let p = sh.planes(3, 2, &|_, _| (f64::NAN, 0.5));
        assert!(p.stops.iter().all(|v| *v == 0.0));
        assert_eq!(sh.planes(0, 0, &|x, y| (x, y)).stops.len(), 0);
    }
}
