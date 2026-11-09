//! Dark Circles and Eye Bags: under one eye, on its skin, in the crescents of [`super::weights`].
//!
//! Both keep the skin's texture: each pixel is multiplied by a smooth gain. The gains are worked out on a grid in
//! the eye's own frame (cells a fixed share of the eye's width, so a draft, the loupe and an export work them out
//! alike, and a close-up costs no more than a small face), from blurs of the skin alone: the eye, dark lashes and
//! liner, and glasses frames are left out of every blur (normalised), so their colours never bleed into the skin's
//! and nothing halos at their edges. The gains are then laid on the pixels, which keep the eye, lashes and liner
//! out at the pixels' own resolution.
//! - Dark Circles: the gain takes the under-eye colour, blurred, toward the cheek's mean below it: lifting only,
//!   never darkening, and turning the colour only part of the way.
//! - Eye Bags: the gain is the coarse blur of the luminance over the fine one, which flattens the shading between
//!   those scales (the bag's bulge and crease) and keeps what is finer (pores, fine lines).
//!
//! Skin is the Facial Skin mask and, through glasses (the parser takes the lenses out of Facial Skin), the Others
//! mask, either only where a pixel's colour is the face's skin colour: a dark lash or frame and a pale one (white,
//! silver) are left alone.

use lightcraft_raster::blur::{gauss_taps, gaussian, gaussian_taps};
use lightcraft_raster::{Image, Rgb32f};

use super::Area;
use super::geom::{Eye, LID_MAX, smoothstep};
use super::weights::{self, LID_GAP, UNDER_BOX};
use crate::{Part, PartMask, Scene};

/// Blur of the under-eye colour Dark Circles corrects, in half eye widths.
const DARK_SIGMA: f64 = 0.12;
/// Most Dark Circles lifts the luminance (a factor: about 0.7 of a stop).
const MAX_LIFT: f32 = 1.6;
/// How far Dark Circles turns each channel against the luminance (a factor either way) …
const MAX_TINT: f32 = 1.2;
/// … and how much of the way to the cheek's colour it goes (make-up under the eye is not a dark circle).
const TINT_SHARE: f32 = 0.6;
/// Eye Bags' fine and coarse blurs, in half eye widths.
const BAG_FINE: f64 = 0.05;
const BAG_COARSE: f64 = 0.45;
/// How much of the shading Eye Bags removes at 100.
const BAG_STRENGTH: f32 = 0.9;
/// Most Eye Bags changes a pixel's luminance (a factor either way).
const BAG_RANGE: f32 = 1.6;
/// The cheek patch must have at least this much skin (in square half widths) to give Dark Circles a target.
const MIN_CHEEK: f64 = 0.02;
/// How far from the skin's chromaticity a pixel through glasses still counts as skin (wholly, then not at all).
const SEEN_HUE: (f64, f64) = (0.045, 0.1);
/// A pixel this much darker than the skin (lashes, liner, a frame) is not skin (not at all, then wholly).
const DARK_OUT: (f64, f64) = (0.3, 0.5);
/// The blur of the skin before it weighs the effects, in half eye widths.
const GATE_SIGMA: f64 = 0.08;
/// Grid cells per half eye width.
const CELLS_PER_HALF: f64 = 16.0;
/// Most cells along a grid's side (a grid is about 90 a side).
const MAX_SIDE: usize = 256;
/// How far, in sigmas of the widest blur, the grid reaches past the under-eye areas.
pub const PAD_SIGMAS: f64 = 2.5;
/// Samples averaged per cell, per side.
const SUB: usize = 3;
/// Sigmas under this many pixels (or cells) use an exact kernel (the three-box blur can't make them).
const EXACT_BELOW: f32 = 2.5;

/// Luminance of linear Rec.2020 (the app's working space).
pub fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]
}

/// Blur `img` with a Gaussian of `sigma` pixels.
pub fn blur<T: lightcraft_raster::resample::Pixel>(img: &Image<T>, sigma: f64) -> Image<T> {
    let s = sigma as f32;
    if !(s.is_finite() && s > 0.0) {
        return img.clone();
    }
    if s < EXACT_BELOW { gaussian_taps(img, &gauss_taps(s)) } else { gaussian(img, s) }
}

/// Blur a grid's values (`w × h`, `N` channels) with a Gaussian of `sigma` cells: an exact kernel, on this
/// thread (a grid is a few thousand cells; handing it to the thread pool costs more than the blur).
fn grid_blur<const N: usize>(v: &[[f32; N]], w: usize, h: usize, sigma: f64) -> Vec<[f32; N]> {
    let s = sigma as f32;
    if !(s.is_finite() && s > 0.1) || v.len() != w * h || w == 0 {
        return v.to_vec();
    }
    if s >= EXACT_BELOW {
        // three box passes a side (running sums: the cost does not grow with the sigma)
        let mut out = v.to_vec();
        for r in lightcraft_raster::blur::box_radii(s) {
            out = box_pass(&out, w, h, r, true);
            out = box_pass(&out, w, h, r, false);
        }
        return out;
    }
    let taps = gauss_taps(s);
    let r = (taps.len() / 2) as isize;
    let pass = |src: &[[f32; N]], horizontal: bool| -> Vec<[f32; N]> {
        let mut out = vec![[0.0f32; N]; src.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; N];
                for (k, t) in taps.iter().enumerate() {
                    let d = k as isize - r;
                    let (sx, sy) = if horizontal {
                        ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                    } else {
                        (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                    };
                    if let Some(p) = src.get(sy * w + sx) {
                        for c in 0..N {
                            acc[c] += p[c] * t;
                        }
                    }
                }
                if let Some(o) = out.get_mut(y * w + x) {
                    *o = acc;
                }
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

/// A box of radius `r` along rows (`horizontal`) or columns of a `w × h` grid, edges held.
fn box_pass<const N: usize>(v: &[[f32; N]], w: usize, h: usize, r: usize, horizontal: bool) -> Vec<[f32; N]> {
    if r == 0 {
        return v.to_vec();
    }
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, k: usize| -> [f32; N] {
        let i = if horizontal { line * w + k } else { k * w + line };
        v.get(i).copied().unwrap_or([0.0; N])
    };
    let mut out = vec![[0.0f32; N]; v.len()];
    let n = (2 * r + 1) as f32;
    for line in 0..lines {
        let held = |k: isize| at(line, k.clamp(0, len as isize - 1) as usize);
        let mut sum = [0.0f32; N];
        for k in -(r as isize)..=(r as isize) {
            let p = held(k);
            sum = std::array::from_fn(|c| sum[c] + p[c]);
        }
        for k in 0..len {
            let i = if horizontal { line * w + k } else { k * w + line };
            if let Some(o) = out.get_mut(i) {
                *o = sum.map(|c| c / n);
            }
            let (add, sub) = (held(k as isize + r as isize + 1), held(k as isize - r as isize));
            sum = std::array::from_fn(|c| sum[c] + add[c] - sub[c]);
        }
    }
    out
}

/// The image at (`x`, `y`), bilinear between pixel centres; `None` outside it.
pub fn sample(img: &Rgb32f, x: f64, y: f64) -> Option<[f32; 3]> {
    let (w, h) = (img.width, img.height);
    if !(x >= 0.0 && y >= 0.0 && x <= w as f64 && y <= h as f64) || w == 0 || h == 0 {
        return None;
    }
    let (fx, fy) = ((x - 0.5).clamp(0.0, (w - 1) as f64), (y - 0.5).clamp(0.0, (h - 1) as f64));
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let at = |x: usize, y: usize| img.data.get(y * w + x).copied();
    let (a, b, c, d) = (at(x0, y0)?, at(x1, y0)?, at(x0, y1)?, at(x1, y1)?);
    Some(std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    }))
}

/// A grid in the eye's frame: cell (`i`, `j`) is centred on eye-local (`a0 + i·step`, `b0 + j·step`).
struct Grid {
    a0: f64,
    b0: f64,
    step: f64,
    w: usize,
    h: usize,
}

impl Grid {
    /// The grid around `eye`: the same cells, in the eye's frame, at every size and zoom ([`CELLS_PER_HALF`]), at
    /// most [`MAX_SIDE`] a side.
    fn new(eye: &Eye) -> Option<Grid> {
        let step = 1.0 / CELLS_PER_HALF;
        let pad = PAD_SIGMAS * BAG_COARSE.max(DARK_SIGMA);
        let [a0, a1, t0, t1] = UNDER_BOX;
        let (a0, a1, b0, b1) = (a0 - pad, a1 + pad, t0 - pad, t1 + eye.lid_drop().min(LID_MAX) + pad);
        let side = |span: f64| {
            let n = (span / step).ceil();
            (n.is_finite() && n >= 0.0 && n < MAX_SIDE as f64).then(|| (n as usize).checked_add(1)).flatten()
        };
        Some(Grid { a0, b0, step, w: side(a1 - a0)?, h: side(b1 - b0)? })
    }

    fn local(&self, i: usize, j: usize) -> (f64, f64) {
        (self.a0 + i as f64 * self.step, self.b0 + j as f64 * self.step)
    }

    /// `v` sampled at eye-local (`a`, `b`), bilinear between cell centres (held at the edge).
    fn at<T: Copy + Interp>(&self, v: &[T], a: f64, b: f64, none: T) -> T {
        let (fx, fy) = (((a - self.a0) / self.step).clamp(0.0, (self.w - 1) as f64), ((b - self.b0) / self.step).clamp(0.0, (self.h - 1) as f64));
        if !(fx.is_finite() && fy.is_finite()) {
            return none;
        }
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let g = |x: usize, y: usize| v.get(y * self.w + x).copied().unwrap_or(none);
        T::lerp(T::lerp(g(x0, y0), g(x1, y0), tx), T::lerp(g(x0, y1), g(x1, y1), tx), ty)
    }
}

trait Interp {
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl Interp for [f32; 3] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
    }
}

/// The masks a pixel is judged by.
struct Masks<'a> {
    skin: &'a PartMask,
    others: Option<&'a PartMask>,
    whites: Option<&'a PartMask>,
    iris: Option<&'a PartMask>,
}

impl Masks<'_> {
    /// Facial Skin, Others and the eye (Eye Whites or Iris) at normalised (`nx`, `ny`).
    fn at(&self, nx: f64, ny: f64) -> (f32, f32, f32) {
        let s = |m: Option<&PartMask>| m.map_or(0.0, |m| m.sample(nx, ny));
        (self.skin.sample(nx, ny), s(self.others), s(self.whites).max(s(self.iris)))
    }
}

/// Dark Circles (`dark`) and Eye Bags (`bags`), 0…1 each, under `eye` of face `face`.
pub fn apply(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, eye: &Eye, dark: f32, bags: f32) {
    let Some(f) = scene.faces.get(face).map(|c| c.face) else { return };
    let Some(skin) = f.mask(Part::FacialSkin) else { return };
    let masks = Masks { skin, others: f.mask(Part::Others), whites: f.mask(Part::EyeWhites), iris: f.mask(Part::Iris) };
    let Some(grid) = Grid::new(eye) else { return };
    let Some(gains) = gains(img, scene, eye, &masks, &grid, dark, bags) else { return };
    lay(img, scene, eye, &masks, &grid, &gains.0, gains.1);
}

/// Each cell's gain (per channel), and the skin's colour.
fn gains(img: &Rgb32f, scene: &Scene<'_>, eye: &Eye, masks: &Masks<'_>, grid: &Grid, dark: f32, bags: f32) -> Option<(Vec<[f32; 3]>, [f32; 3])> {
    let n = grid.w.checked_mul(grid.h)?;
    let view = scene.view();
    let cell_px = grid.step * eye.half;
    // (a cell smaller than a pixel needs one sample: the image has no more in it)
    let sub = if cell_px < 1.0 { 1 } else { SUB };
    // each cell's samples (SUB × SUB over it), masks and place
    let mut cells: Vec<([[f32; 3]; SUB * SUB], usize)> = vec![([[0.0; 3]; SUB * SUB], 0); n];
    let mut at = vec![(0.0f32, 0.0f32, 0.0f32, 0.0f64, 0.0f64); n];
    for j in 0..grid.h {
        for i in 0..grid.w {
            let k = j * grid.w + i;
            let (a, b) = grid.local(i, j);
            let (px, py) = eye.to_px(a, b);
            let Some((samples, got)) = cells.get_mut(k) else { continue };
            for sj in 0..sub {
                for si in 0..sub {
                    let (da, db) = ((si as f64 + 0.5) / sub as f64 - 0.5, (sj as f64 + 0.5) / sub as f64 - 0.5);
                    let (sa, sb) = (da * cell_px, db * cell_px);
                    let (sx, sy) = (px + sa * eye.across.0 + sb * eye.down.0, py + sa * eye.across.1 + sb * eye.down.1);
                    if let (Some(p), Some(slot)) = (sample(img, sx, sy), samples.get_mut(*got)) {
                        *slot = p;
                        *got += 1;
                    }
                }
            }
            let (nx, ny) = view.to_norm(px, py);
            let (s, o, e) = masks.at(nx, ny);
            if let Some(v) = at.get_mut(k) {
                *v = (s, o, e, a, eye.below_lid(a, b));
            }
        }
    }
    // the skin's colour: the mean of the plain skin's samples, then again without the dark ones (lashes, liner)
    let skin_of = |like: Option<[f32; 3]>| -> Option<[f32; 3]> {
        let (mut sum, mut count) = ([0.0f64; 3], 0.0f64);
        for ((samples, got), &(s, o, e, _, t)) in cells.iter().zip(&at) {
            if !(s >= 0.5 && o < 0.5 && e <= 0.0 && t > LID_GAP) {
                continue;
            }
            for p in samples.iter().take(*got) {
                if like.is_none_or(|c| not_dark(*p, c) >= 0.5) {
                    for ch in 0..3 {
                        sum[ch] += f64::from(p[ch]);
                    }
                    count += 1.0;
                }
            }
        }
        (count > 0.0).then(|| sum.map(|v| (v / count) as f32)).filter(|c| luma(*c) > 1e-6)
    };
    let first = skin_of(None)?;
    let skin = skin_of(Some(first)).unwrap_or(first);
    // each cell's colour: the mean of its samples that are not dark, and how much of it they are
    let mut colour = vec![[0.0f32; 3]; n];
    let mut cover = vec![0.0f32; n];
    for ((samples, got), (c, cv)) in cells.iter().zip(colour.iter_mut().zip(cover.iter_mut())) {
        let (mut acc, mut wsum) = ([0.0f32; 3], 0.0f32);
        for p in samples.iter().take(*got) {
            let w = not_dark(*p, skin);
            acc = std::array::from_fn(|ch| acc[ch] + p[ch] * w);
            wsum += w;
        }
        if wsum > 0.0 {
            *c = acc.map(|v| v / wsum);
            *cv = wsum / (*got).max(1) as f32;
        }
    }
    // a cell beside a dark one holds the dark one's edge (its samples blend into it): it counts as little
    let cover: Vec<f32> = (0..n)
        .map(|k| {
            let (i, j) = ((k % grid.w) as isize, (k / grid.w) as isize);
            let mut m = 1.0f32;
            for (di, dj) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let (x, y) = (i + di, j + dj);
                if x >= 0 && y >= 0 && (x as usize) < grid.w && (y as usize) < grid.h {
                    m = m.min(cover.get(y as usize * grid.w + x as usize).copied().unwrap_or(0.0));
                }
            }
            m
        })
        .collect();
    // skin per cell: what the blurs read
    let mut gate = vec![0.0f32; n];
    for (k, g) in gate.iter_mut().enumerate() {
        let (Some(&(s, o, e, _, t)), Some(&c), Some(&cv)) = (at.get(k), colour.get(k), cover.get(k)) else { continue };
        if t <= 0.0 || cv <= 0.0 {
            continue;
        }
        // (Facial Skin or a lens, either one only where the colour is skin's: a pale frame, a white or silver one, is not)
        *g = (s * (1.0 - o)).max(o) * likeness(c, skin) * cv * (1.0 - e) * smoothstep(0.0, LID_GAP, t) as f32;
    }
    // the effects' weights: the skin smoothed (a patchy one, through glasses, gives no blotches), kept off the eye
    let smooth = grid_blur(&gate.iter().map(|g| [*g]).collect::<Vec<_>>(), grid.w, grid.h, GATE_SIGMA / grid.step);
    let weight = |area: Area, k: usize| -> f32 {
        let (Some(&(_, _, e, a, t)), Some(&[g])) = (at.get(k), smooth.get(k)) else { return 0.0 };
        if t <= 0.0 { 0.0 } else { weights::of(area, a, t) * g * (1.0 - e) * smoothstep(0.0, LID_GAP, t) as f32 }
    };
    let mut gain = vec![[1.0f32; 3]; n];
    if dark > 0.0 {
        dark_circles(grid, &colour, &gate, &at, &mut gain, |k| dark * weight(Area::DarkCircles, k));
    }
    if bags > 0.0 {
        eye_bags(grid, &colour, &gate, &mut gain, |k| bags * BAG_STRENGTH * weight(Area::EyeBags, k));
    }
    Some((gain, skin))
}

type At = (f32, f32, f32, f64, f64);

fn dark_circles(grid: &Grid, colour: &[[f32; 3]], gate: &[f32], at: &[At], gain: &mut [[f32; 3]], weight: impl Fn(usize) -> f32) {
    // the target: the cheek's mean colour
    let (mut sum, mut total) = ([0.0f64; 3], 0.0f64);
    for ((c, g), &(_, _, _, a, t)) in colour.iter().zip(gate).zip(at) {
        let w = weights::cheek(a, t) * g;
        if w > 0.0 {
            for ch in 0..3 {
                sum[ch] += f64::from(c[ch] * w);
            }
            total += f64::from(w);
        }
    }
    if total.is_nan() || total < MIN_CHEEK / (grid.step * grid.step) {
        return;
    }
    let target = sum.map(|v| (v / total) as f32);
    let ty = luma(target);
    if !(ty.is_finite() && ty > 0.0) {
        return;
    }
    let packed: Vec<[f32; 4]> = colour.iter().zip(gate).map(|(c, g)| [c[0] * g, c[1] * g, c[2] * g, *g]).collect();
    let low = grid_blur(&packed, grid.w, grid.h, DARK_SIGMA / grid.step);
    for (k, (out, l)) in gain.iter_mut().zip(&low).enumerate() {
        let w = weight(k);
        if !(w > 0.0 && l[3] > 1e-4) {
            continue;
        }
        let l = [l[0] / l[3], l[1] / l[3], l[2] / l[3]];
        let ly = luma(l);
        if ly.is_nan() || ly <= 1e-6 {
            continue;
        }
        let lift = (ty / ly).clamp(1.0, MAX_LIFT);
        for c in 0..3 {
            // the channel's share of the luminance, turned toward the cheek's
            let tint = if l[c] > 1e-6 && target[c] > 0.0 { ((target[c] / ty) / (l[c] / ly)).clamp(1.0 / MAX_TINT, MAX_TINT) } else { 1.0 };
            // (lifting only: no channel darkens)
            let g = (lift * (1.0 + TINT_SHARE * (tint - 1.0))).max(1.0);
            if g.is_finite() {
                out[c] *= 1.0 + w * (g - 1.0);
            }
        }
    }
}

fn eye_bags(grid: &Grid, colour: &[[f32; 3]], gate: &[f32], gain: &mut [[f32; 3]], weight: impl Fn(usize) -> f32) {
    // on the colours as Dark Circles left them
    let packed: Vec<[f32; 2]> =
        colour.iter().zip(gate).zip(gain.iter()).map(|((c, g), d)| [luma([c[0] * d[0], c[1] * d[1], c[2] * d[2]]) * g, *g]).collect();
    let (fine, coarse) = (grid_blur(&packed, grid.w, grid.h, BAG_FINE / grid.step), grid_blur(&packed, grid.w, grid.h, BAG_COARSE / grid.step));
    for (k, (out, (f, c))) in gain.iter_mut().zip(fine.iter().zip(&coarse)).enumerate() {
        let w = weight(k);
        if !(w > 0.0 && f[1] > 1e-4 && c[1] > 1e-4) {
            continue;
        }
        let (fy, cy) = (f[0] / f[1], c[0] / c[1]);
        if !(fy > 1e-6 && cy > 1e-6) {
            continue;
        }
        let m = 1.0 + w * ((cy / fy).clamp(1.0 / BAG_RANGE, BAG_RANGE) - 1.0);
        if m.is_finite() {
            *out = out.map(|v| v * m);
        }
    }
}

/// Lay the cells' gains on the pixels the under-eye areas cover, keeping the eye, the lashes and liner out at the
/// pixels' resolution. Every other pixel stays as it was.
fn lay(img: &mut Rgb32f, scene: &Scene<'_>, eye: &Eye, masks: &Masks<'_>, grid: &Grid, gain: &[[f32; 3]], skin: [f32; 3]) {
    let Some([x0, y0, x1, y1]) = eye.pixel_box(weights::EFFECT_BOX, 1.0, img.width, img.height) else { return };
    let view = scene.view();
    for y in y0..y1 {
        for x in x0..x1 {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let (a, b) = eye.local(px, py);
            let t = eye.below_lid(a, b);
            if t <= 0.0 || (weights::of(Area::DarkCircles, a, t) <= 0.0 && weights::of(Area::EyeBags, a, t) <= 0.0) {
                continue;
            }
            let g = grid.at(gain, a, b, [1.0; 3]);
            if g == [1.0; 3] {
                continue;
            }
            let i = y * img.width + x;
            let Some(&p) = img.data.get(i) else { continue };
            let (nx, ny) = view.to_norm(px, py);
            let (_, _, e) = masks.at(nx, ny);
            let keep = (1.0 - e) * smoothstep(0.0, LID_GAP, t) as f32 * not_dark(p, skin) * likeness(p, skin);
            if keep.is_nan() || keep <= 0.0 {
                continue;
            }
            if let Some(q) = img.data.get_mut(i) {
                *q = std::array::from_fn(|c| p[c] * (1.0 + keep * (g[c] - 1.0)));
            }
        }
    }
}

/// 1 for a colour about as light as the skin or lighter, falling to 0 for lashes, liner or a dark frame.
fn not_dark(p: [f32; 3], skin: [f32; 3]) -> f32 {
    smoothstep(DARK_OUT.0, DARK_OUT.1, f64::from(luma(p) / luma(skin).max(1e-6))) as f32
}

/// How much `p` looks like skin of colour `skin`: about as light (a shadow under the eye is darker) and of nearly
/// the same hue and saturation. 0…1.
fn likeness(p: [f32; 3], skin: [f32; 3]) -> f32 {
    let (sp, ss) = (p[0] + p[1] + p[2], skin[0] + skin[1] + skin[2]);
    if !(sp > 1e-6 && ss > 1e-6) {
        return 0.0;
    }
    let ratio = f64::from(luma(p) / luma(skin).max(1e-6));
    let (dr, dg) = (f64::from(p[0] / sp - skin[0] / ss), f64::from(p[1] / sp - skin[1] / ss));
    let hue = (dr * dr + dg * dg).sqrt();
    ((1.0 - smoothstep(1.8, 2.6, ratio)) * (1.0 - smoothstep(SEEN_HUE.0, SEEN_HUE.1, hue))) as f32
}
