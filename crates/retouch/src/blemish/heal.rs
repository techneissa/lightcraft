//! Healing one spot, in two layers split at the pore scale ([`TEXTURE`], the split Skin Smoothness uses):
//!
//! - **Tone** (the skin blurred at that scale): rebuilt inside the spot from the ring of skin [`FEATHER`] radii out.
//!   Each pixel is interpolated along eight lines through it between the two points where each meets the ring,
//!   weighted towards the lines whose two ends match: a crease or a gradient running through the spot carries on
//!   through it, and a flat ring gives a flat fill. Ring points on another spot or off the skin are filled from their
//!   neighbours on the ring.
//! - **Texture** (the skin over its tone: pores, fine lines, grain): the donor patch's ([`Spot::donor`]), or without
//!   one the spot's own finest grain, so the healed skin has the pores of the skin next to it.
//!
//! Inside [`CORE`] radii the healed skin replaces the spot; out to [`FEATHER`] its texture fades back into the
//! skin's own, and its tone meets the ring there, so no edge shows.

use lightcraft_raster::Rgb32f;

use super::{CORE, FEATHER, Placed};
use crate::{Part, Scene};

/// The split between tone and texture, a Gaussian's sigma as a fraction of the face: the same as Skin Smoothness's
/// pores (`skin::PORES`).
pub const TEXTURE: f64 = 0.002;
/// The texture is borrowed as a ratio to its own tone, held within this factor either way (a speck of dust in the
/// donor can't print a black or white dot).
const TEXTURE_RANGE: f32 = 1.6;
/// Points on the ring the tone is rebuilt from.
const RING_POINTS: usize = 32;
/// At least this share of them must be skin clear of other spots.
const RING_LEAST: usize = RING_POINTS / 4;
/// Lines each pixel's tone is interpolated along.
const LINES: usize = 8;
/// How much two ring points may differ (squared natural log, per colour, summed) and still count as matching.
const MATCH: f32 = 0.002;
/// A donor picked at render time ([`Healer::near_donor`]) lies this many spot radii away (past the spot's healed
/// core, so its patch never takes in the spot).
const NEAR_DONOR: f64 = 3.4;
/// The donor's texture is made at most this much stronger or weaker to match the skin around the spot.
const GAIN_RANGE: f32 = 1.25;
/// Luminance weights (Rec. 2020) for measuring texture.
const LUMA: [f32; 3] = [0.2627, 0.678, 0.0593];
/// Mask at least this is skin for the ring and the own grain.
const SKIN: f32 = 0.5;
/// The own grain (without a donor) is measured against a window of this share of the spot's radius either way, one
/// pixel at least and [`GRAIN_MOST`] at most: well inside the spot, so it keeps pores and not the spot.
const GRAIN_SPOT: f64 = 1.0 / 6.0;
const GRAIN_MOST: usize = 4;

/// One repaired pixel: its index, its repaired colour and how much of it to blend in.
pub(super) type Healed = (usize, [f32; 3], f32);

/// What healing one face's spots reads besides the image.
pub(super) struct Healer<'a> {
    pub(super) scene: &'a Scene<'a>,
    pub(super) face: usize,
    /// The mask's box: nothing outside it is read or written.
    pub(super) b: [usize; 4],
    /// Every spot, placed (a ring point inside another spot is filled from its neighbours).
    pub(super) all: &'a [Option<Placed>],
    /// [`TEXTURE`] in the image's pixels.
    pub(super) pores: f64,
}

/// The skin's tone over a box of the image: its colour times the mask and the mask, box-blurred three times (about a
/// Gaussian), so hair or background next to the skin counts for nothing.
struct Tone {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    data: Vec<[f32; 4]>,
}

impl Tone {
    /// The tone over `x0..x1` × `y0..y1` (clamped to the healer's box) at sigma `sigma` pixels.
    fn new(img: &Rgb32f, healer: &Healer<'_>, [x0, y0, x1, y1]: [isize; 4], sigma: f64) -> Option<Tone> {
        let [bx0, by0, bx1, by1] = healer.b;
        let x0 = x0.max(bx0 as isize).max(0) as usize;
        let y0 = y0.max(by0 as isize).max(0) as usize;
        let x1 = (x1.max(0) as usize).min(bx1).min(img.width);
        let y1 = (y1.max(0) as usize).min(by1).min(img.height);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut data: Vec<[f32; 4]> = Vec::with_capacity(w * h);
        for y in y0..y1 {
            for x in x0..x1 {
                let m = healer.mask(x as f64 + 0.5, y as f64 + 0.5);
                let p = img.data.get(y * img.width + x).copied().unwrap_or_default();
                data.push([p[0] * m, p[1] * m, p[2] * m, m]);
            }
        }
        // three boxes of width 2·half+1 make a Gaussian of about this sigma (one pixel either way at least: the
        // finest grain is texture at any size)
        let half = (((4.0 * sigma * sigma + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
        let mut t = Tone { x0, y0, w, h, data };
        for _ in 0..3 {
            t.boxed(half);
        }
        Some(t)
    }

    /// One box blur of half-width `half`, rows then columns (edges clamped).
    fn boxed(&mut self, half: usize) {
        if half == 0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let n = (2 * half + 1) as f32;
        let mut line: Vec<[f32; 4]> = Vec::new();
        let pass = |line: &mut Vec<[f32; 4]>, len: usize, get: &dyn Fn(usize) -> [f32; 4]| {
            line.clear();
            let at = |i: isize| get(i.clamp(0, len as isize - 1) as usize);
            let mut acc = [0.0f32; 4];
            for i in -(half as isize)..=half as isize {
                acc = add(acc, at(i));
            }
            for i in 0..len as isize {
                line.push(acc.map(|v| v / n));
                acc = sub(add(acc, at(i + half as isize + 1)), at(i - half as isize));
            }
        };
        for y in 0..h {
            let row: Vec<[f32; 4]> = self.data.get(y * w..(y + 1) * w).map(<[_]>::to_vec).unwrap_or_default();
            pass(&mut line, w, &|i| row.get(i).copied().unwrap_or_default());
            if let Some(dst) = self.data.get_mut(y * w..(y + 1) * w) {
                dst.copy_from_slice(&line);
            }
        }
        for x in 0..w {
            let col: Vec<[f32; 4]> = (0..h).map(|y| self.data.get(y * w + x).copied().unwrap_or_default()).collect();
            pass(&mut line, h, &|i| col.get(i).copied().unwrap_or_default());
            for (y, v) in line.iter().enumerate() {
                if let Some(d) = self.data.get_mut(y * w + x) {
                    *d = *v;
                }
            }
        }
    }

    /// The tone at pixel (`x`, `y`), with the skin's weight there; `None` outside the box or off the skin.
    fn px(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        let (lx, ly) = (x.checked_sub(self.x0)?, y.checked_sub(self.y0)?);
        if lx >= self.w || ly >= self.h {
            return None;
        }
        let p = self.data.get(ly * self.w + lx)?;
        (p[3] > 1e-3).then(|| [p[0] / p[3], p[1] / p[3], p[2] / p[3]])
    }

    /// The tone at pixel position (`x`, `y`) (centres at +0.5), the nearest pixel's.
    fn at(&self, x: f64, y: f64) -> Option<[f32; 3]> {
        if !(x >= 0.0 && y >= 0.0) {
            return None;
        }
        self.px(x as usize, y as usize)
    }
}

fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + b[i])
}

fn sub(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The ring's tone at angle `a` (radians), from `ring` (the tone at [`RING_POINTS`] evenly spaced angles from 0).
fn ring_at(ring: &[[f32; 3]; RING_POINTS], a: f64) -> [f32; 3] {
    let t = (a / std::f64::consts::TAU).rem_euclid(1.0) * RING_POINTS as f64;
    let i = (t.floor() as usize) % RING_POINTS;
    let f = (t - t.floor()) as f32;
    let (p, q) = (ring[i], ring[(i + 1) % RING_POINTS]);
    std::array::from_fn(|c| p[c] + (q[c] - p[c]) * f)
}

impl Healer<'_> {
    /// The skin mask at pixel position (`x`, `y`).
    fn mask(&self, x: f64, y: f64) -> f32 {
        if !(x >= 0.0 && y >= 0.0) {
            return 0.0;
        }
        self.scene.mask_at(self.face, Part::FacialSkin, x as usize, y as usize)
    }

    /// Whether point `q` lies inside a spot other than the `me`-th.
    fn in_other(&self, q: (f64, f64), me: usize) -> bool {
        self.all
            .iter()
            .enumerate()
            .any(|(k, o)| k != me && o.as_ref().is_some_and(|o| ((o.c.0 - q.0).powi(2) + (o.c.1 - q.1).powi(2)).sqrt() < o.r * CORE))
    }

    /// The tone on the ring of radius `r` around `c`, with points off the skin or on another spot filled from their
    /// neighbours; `None` with too few good points.
    fn ring(&self, tone: &Tone, c: (f64, f64), r: f64, me: usize) -> Option<[[f32; 3]; RING_POINTS]> {
        let got: Vec<Option<[f32; 3]>> = (0..RING_POINTS)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / RING_POINTS as f64;
                let q = (c.0 + r * a.cos(), c.1 + r * a.sin());
                if self.mask(q.0, q.1) < SKIN || self.in_other(q, me) {
                    return None;
                }
                tone.at(q.0, q.1)
            })
            .collect();
        if got.iter().flatten().count() < RING_LEAST {
            return None;
        }
        // each missing point: between the nearest good ones either way round
        Some(std::array::from_fn(|i| {
            if let Some(v) = got.get(i).copied().flatten() {
                return v;
            }
            let find = |step: isize| {
                (1..RING_POINTS as isize).find_map(|k| {
                    let j = (i as isize + step * k).rem_euclid(RING_POINTS as isize) as usize;
                    got.get(j).copied().flatten().map(|v| (k, v))
                })
            };
            match (find(1), find(-1)) {
                (Some((ka, a)), Some((kb, b))) => {
                    let f = kb as f32 / (ka + kb) as f32;
                    std::array::from_fn(|ch| b[ch] + (a[ch] - b[ch]) * f)
                }
                (Some((_, v)), None) | (None, Some((_, v))) => v,
                (None, None) => [0.0; 3],
            }
        }))
    }

    /// Heal spot `p` (the `me`-th) with weight `w`, reading `img` and writing inside the mask's box.
    pub(super) fn heal(&self, img: &Rgb32f, p: &Placed, w: f32, me: usize) -> Vec<Healed> {
        let (c, r) = (p.c, p.r);
        let outer = r * FEATHER;
        let margin = (3.0 * self.pores).ceil() + 2.0;
        let around = |c: (f64, f64)| {
            let e = outer + margin;
            [(c.0 - e).floor() as isize, (c.1 - e).floor() as isize, (c.0 + e).ceil() as isize, (c.1 + e).ceil() as isize]
        };
        let Some(tone) = Tone::new(img, self, around(c), self.pores) else { return Vec::new() };
        let Some(ring) = self.ring(&tone, c, outer, me) else { return Vec::new() };
        let donor = p
            .offset
            .or_else(|| self.near_donor(c, r, me))
            .and_then(|o| Tone::new(img, self, around((c.0 + o.0, c.1 + o.1)), self.pores).map(|t| (o, t)));
        let half = ((r * GRAIN_SPOT).floor() as usize).clamp(1, GRAIN_MOST);
        // the donor's pores as strong as the skin's around the spot: the cleanest patch nearby is often smoother
        let gain = donor.as_ref().map_or(1.0, |(o, dt)| self.texture_gain(img, &tone, dt, (c, r, outer), *o));
        let [bx0, by0, bx1, by1] = self.b;
        let x0 = ((c.0 - outer).floor().max(bx0 as f64)) as usize;
        let y0 = ((c.1 - outer).floor().max(by0 as f64)) as usize;
        let x1 = ((c.0 + outer).ceil().max(0.0) as usize).min(bx1).min(img.width);
        let y1 = ((c.1 + outer).ceil().max(0.0) as usize).min(by1).min(img.height);
        let mut out: Vec<Healed> = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let q = (x as f64 + 0.5, y as f64 + 0.5);
                let d = (q.0 - c.0, q.1 - c.1);
                let dist = (d.0 * d.0 + d.1 * d.1).sqrt();
                if dist >= outer {
                    continue;
                }
                let k = w * self.mask(q.0, q.1);
                let i = y * img.width + x;
                let Some(orig) = img.data.get(i).copied() else { continue };
                if k <= 0.0 {
                    continue;
                }
                let Some(own_tone) = tone.px(x, y) else { continue };
                let new_tone = interpolate(&ring, outer, d);
                // texture: the donor's inside the spot, fading to the skin's own by the ring
                let own: [f32; 3] = std::array::from_fn(|ch| ratio(orig[ch], own_tone[ch]));
                let inside = match &donor {
                    Some((o, dt)) => {
                        let (dx, dy) = ((q.0 + o.0).floor(), (q.1 + o.1).floor());
                        let md = self.mask(dx + 0.5, dy + 0.5);
                        let px = (dx >= 0.0 && dy >= 0.0).then_some((dx as usize, dy as usize));
                        match px.and_then(|(px, py)| Some((img.data.get(py * img.width + px).copied()?, dt.px(px, py)?))) {
                            Some((s, t)) if md > 0.0 => std::array::from_fn(|ch| 1.0 + md.min(1.0) * (ratio(s[ch], t[ch]).powf(gain) - 1.0)),
                            _ => self.grain(img, x, y, half),
                        }
                    }
                    None => self.grain(img, x, y, half),
                };
                let f = smoothstep(r * CORE, outer, dist) as f32;
                let tex: [f32; 3] = std::array::from_fn(|ch| inside[ch] + (own[ch] - inside[ch]) * f);
                let new: [f32; 3] = std::array::from_fn(|ch| new_tone[ch] * tex[ch]);
                if new.iter().all(|v| v.is_finite()) {
                    out.push((i, new, k));
                }
            }
        }
        out
    }

    /// How much stronger the skin's texture (luminance over its tone) is on the ring between [`CORE`] and `outer`
    /// radii around the spot than in the donor's patch (offset `o`, inside [`CORE`] radii), held within
    /// [`GAIN_RANGE`]: the power the donor's texture is raised to.
    fn texture_gain(&self, img: &Rgb32f, tone: &Tone, donor: &Tone, (c, r, outer): ((f64, f64), f64, f64), o: (f64, f64)) -> f32 {
        let energy = |t: &Tone, centre: (f64, f64), from: f64, to: f64| {
            let (mut n, mut s, mut s2) = (0.0f64, 0.0f64, 0.0f64);
            let (x0, y0) = ((centre.0 - to).floor().max(0.0) as usize, (centre.1 - to).floor().max(0.0) as usize);
            let (x1, y1) = (((centre.0 + to).ceil().max(0.0)) as usize, ((centre.1 + to).ceil().max(0.0)) as usize);
            for y in y0..y1.min(img.height) {
                for x in x0..x1.min(img.width) {
                    let d = ((x as f64 + 0.5 - centre.0).powi(2) + (y as f64 + 0.5 - centre.1).powi(2)).sqrt();
                    if d < from || d >= to || self.mask(x as f64 + 0.5, y as f64 + 0.5) < SKIN {
                        continue;
                    }
                    let (Some(p), Some(tn)) = (img.data.get(y * img.width + x), t.px(x, y)) else { continue };
                    let l = |v: [f32; 3]| f64::from(LUMA[0] * v[0] + LUMA[1] * v[1] + LUMA[2] * v[2]);
                    let (lp, lt) = (l(*p), l(tn));
                    if lp > 1e-6 && lt > 1e-6 {
                        let v = (lp / lt).ln();
                        n += 1.0;
                        s += v;
                        s2 += v * v;
                    }
                }
            }
            (n >= 8.0).then(|| ((s2 / n - (s / n).powi(2)).max(0.0)).sqrt())
        };
        let skin = energy(tone, c, r * CORE, outer);
        let lent = energy(donor, (c.0 + o.0, c.1 + o.1), 0.0, r * CORE);
        match (skin, lent) {
            (Some(a), Some(b)) if b > 1e-6 => ((a / b) as f32).clamp(1.0 / GAIN_RANGE, GAIN_RANGE),
            _ => 1.0,
        }
    }

    /// A donor for a spot whose finder gave none (or one too far): the first of eight places [`NEAR_DONOR`] radii
    /// away, in whole pixels, whose patch is skin clear of other spots. `None` when there is none.
    fn near_donor(&self, c: (f64, f64), r: f64, me: usize) -> Option<(f64, f64)> {
        (0..8).find_map(|k| {
            let a = f64::from(k) * std::f64::consts::TAU / 8.0;
            let o = ((NEAR_DONOR * r * a.cos()).round(), (NEAR_DONOR * r * a.sin()).round());
            let q = (c.0 + o.0, c.1 + o.1);
            let clear = std::iter::once((0.0, 0.0))
                .chain((0..8).map(|j| f64::from(j) * std::f64::consts::TAU / 8.0).map(|b| (r * CORE * b.cos(), r * CORE * b.sin())))
                .all(|(dx, dy)| self.mask(q.0 + dx, q.1 + dy) >= SKIN && !self.in_other((q.0 + dx, q.1 + dy), me));
            clear.then_some(o)
        })
    }

    /// Pixel (`x`, `y`) as a ratio to the mean of the square `half` pixels either way around it: the finest grain;
    /// no texture (1) at the picture's edge or when any of them is off the skin, so no hair or background comes in.
    fn grain(&self, img: &Rgb32f, x: usize, y: usize, half: usize) -> [f32; 3] {
        let half = half.max(1);
        if x < half || y < half || x + half >= img.width || y + half >= img.height {
            return [1.0; 3];
        }
        let n = ((2 * half + 1) * (2 * half + 1)) as f32;
        let mut mean = [0.0f32; 3];
        for yy in y - half..=y + half {
            for xx in x - half..=x + half {
                let Some(p) = img.data.get(yy * img.width + xx) else { return [1.0; 3] };
                if self.mask(xx as f64 + 0.5, yy as f64 + 0.5) < SKIN {
                    return [1.0; 3];
                }
                mean = std::array::from_fn(|c| mean[c] + p[c] / n);
            }
        }
        let p = img.data.get(y * img.width + x).copied().unwrap_or_default();
        std::array::from_fn(|c| ratio(p[c], mean[c]))
    }
}

/// `a / b`, held within [`TEXTURE_RANGE`] either way; 1 when `b` is not a positive number.
fn ratio(a: f32, b: f32) -> f32 {
    if b > 1e-5 && a.is_finite() { (a / b).clamp(1.0 / TEXTURE_RANGE, TEXTURE_RANGE) } else { 1.0 }
}

/// The tone at offset `d` from the centre of a ring of radius `r`: along [`LINES`] lines through the point, between
/// the two ring points each meets, weighted by how well those two match.
fn interpolate(ring: &[[f32; 3]; RING_POINTS], r: f64, d: (f64, f64)) -> [f32; 3] {
    let dd = d.0 * d.0 + d.1 * d.1;
    let mut acc = [0.0f32; 3];
    let mut wsum = 0.0f32;
    for k in 0..LINES {
        let a = k as f64 * std::f64::consts::PI / LINES as f64;
        let u = (a.cos(), a.sin());
        let s = d.0 * u.0 + d.1 * u.1;
        let root = (s * s - dd + r * r).max(0.0).sqrt();
        let (t1, t2) = (-s + root, -s - root);
        if t1 - t2 <= 1e-9 {
            continue;
        }
        let e1 = (d.0 + t1 * u.0, d.1 + t1 * u.1);
        let e2 = (d.0 + t2 * u.0, d.1 + t2 * u.1);
        let (v1, v2) = (ring_at(ring, e1.1.atan2(e1.0)), ring_at(ring, e2.1.atan2(e2.0)));
        // linear between the two ends, by distance
        let along: [f32; 3] = std::array::from_fn(|ch| ((f64::from(v1[ch]) * -t2 + f64::from(v2[ch]) * t1) / (t1 - t2)) as f32);
        let differ: f32 = (0..3).map(|ch| if v1[ch] > 1e-6 && v2[ch] > 1e-6 { (v1[ch] / v2[ch]).ln().powi(2) } else { 1.0 }).sum();
        let wk = 1.0 / (differ + MATCH);
        acc = std::array::from_fn(|ch| acc[ch] + along[ch] * wk);
        wsum += wk;
    }
    if wsum > 0.0 { acc.map(|v| v / wsum) } else { ring_at(ring, 0.0) }
}
