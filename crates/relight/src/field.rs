//! The field: what no control changes, made once per photo from its depth and a guide picture of it, and kept.
//! Log depth brought up to the guide's size edge-aware (joint-bilateral upsampling against the photo's colours,
//! so depth stops at outlines and hair), surface normals from a copy of it smoothed at a scale tied to the photo
//! (self-guided, so the steps between near and far stay steps and the depth model's small blocks and ripples go;
//! before that, small steps on a surface of one colour, the depth model's terraces, are smoothed away),
//! and the guide's luminance (for highlight roll-off and shadow protection). Along each step between near and far
//! (the edge band) it keeps the guide's local statistics, so the light can follow the photo's own edge there.

use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

use crate::filter::{Guide, Upsample, area_down, bilinear, box_mean, guided, joint_bilateral_up};
use crate::{Error, Result};

/// The camera the depth is placed in: `2 · tan(fov / 2)` over the photo's long side (60°, a normal lens; depth
/// models give no field of view, and the light's look barely depends on it).
pub const LENS: f32 = 1.154_700_5;
/// The largest field (pixels on a side).
pub const MAX_SIDE: usize = 8192;
/// The smallest depth map read (pixels on a side).
pub const MIN_DEPTH_SIDE: usize = 2;
/// How far apart the depth map's and the guide's shapes may be (relative difference of their aspect ratios); a depth
/// model's map also gets [`SHAPE_ROUNDING`].
pub const SHAPE_TOLERANCE: f64 = 0.02;
/// Pixels each side of a depth model's map may be off the photo's shape by ([`Field::prepare_model`]; a file's own
/// map gets none): the model rounds each side of its input to whole patches (Depth Anything's are 14 pixels, so up to 7 off), which bends a 16:10 or 2.35:1 photo's shape by a
/// few percent. The upsampler scales x and y separately, so that costs nothing.
pub const SHAPE_ROUNDING: f64 = 8.0;
/// The most the shapes may differ however small the map: a map of another shape (a portrait's depth on a landscape
/// photo, 4:5 against 5:4 at the closest) stays refused.
pub const SHAPE_MAX: f64 = 0.15;

/// What a depth map's values mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Larger is farther (a depth model's output).
    Depth,
    /// Larger is nearer (an iPhone Portrait photo's disparity map).
    Disparity,
}

/// A depth map: `width × height` values, row-major, over the whole photo.
#[derive(Clone, Copy, Debug)]
pub struct DepthMap<'a> {
    pub width: usize,
    pub height: usize,
    pub values: &'a [f32],
    pub kind: Kind,
}

/// See the module docs.
#[derive(Clone, Debug)]
pub struct Field {
    pub width: usize,
    pub height: usize,
    /// Log depth relative to the photo's median depth.
    pub(crate) ln_z: Vec<f32>,
    /// Unit normals in camera space, on the camera's side of the surface (`n · view ray < 0`): z is negative for a
    /// surface facing the camera, and can be slightly positive for a floor under the lens.
    normal: Vec<[f32; 3]>,
    /// The guide's log2 luminance relative to 18 % grey.
    pub(crate) lum: Vec<f32>,
    /// Log depth (relative) of the nearest and the farthest of the photo: the 2nd and 98th percentiles.
    pub near: f32,
    pub far: f32,
    /// The guide's luminance, cube root (the edge band's guide).
    pub(crate) gi: Vec<f32>,
    /// The pixels near a step in depth, each with the mean and variance of `gi` over its window.
    pub(crate) band: Vec<(u32, f32, f32)>,
    /// The air's colour (issue #331, [`Field::air_colour`]): the guide's mean colour (linear Rec.2020) over its farthest
    /// tenth, and the luminance of the photo's light quarter (its 75th percentile).
    pub(crate) air: [f32; 3],
    pub(crate) bright: f32,
    /// The ground plane under the photo (issue #331, [`Field::ground`]), fitted the first time the mist asks.
    pub(crate) ground: std::sync::OnceLock<crate::mist::Ground>,
    /// The field reduced for the shadow march (issue #315), made the first time a light asks.
    pub(crate) reduced: std::sync::OnceLock<crate::shadow::Reduced>,
    /// The same, coarser, for a drag's shadows.
    pub(crate) reduced_draft: std::sync::OnceLock<crate::shadow::Reduced>,
    /// Issue #330: the guide's linear colour over the reduced field's cells, for the sky's clouds.
    pub(crate) tint: Vec<[f32; 3]>,
    /// Each of those cells' brightest channel of its brightest pixel (issue #330: what a sun's rays may add there).
    pub(crate) tint_top: Vec<f32>,
    /// The photo's sky (issue #330), made the first time the rays ask.
    pub(crate) sky: std::sync::OnceLock<crate::sky::Sky>,
    /// The depth model's sky scores (issue #330) on the reduced field's cells, when it gave them (empty: none).
    pub(crate) sky_scores: Vec<f32>,
}

/// The radius of the window the light is refined over along an edge (field pixels).
pub const BAND_RADIUS: usize = 3;
/// The shortest side [`Field::halved`] makes: a smaller field is cheap enough as it is.
const MIN_HALVED: usize = 64;
/// A change of log depth across three pixels that makes a step (about 6 %).
const STEP: f32 = 0.06;

/// The edge band of log depth `lz` (`w × h`): pixels within [`BAND_RADIUS`] + 1 of a step, with `gi`'s mean and
/// variance over the `(2 BAND_RADIUS + 1)²` window around each.
fn edge_band(lz: &[f32], gi: &[f32], w: usize, h: usize) -> Vec<(u32, f32, f32)> {
    let n = w * h;
    let at = |x: usize, y: usize| lz.get(y * w + x).copied().unwrap_or(0.0);
    let step: Vec<f32> = (0..n)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for yy in y.saturating_sub(1)..(y + 2).min(h) {
                for xx in x.saturating_sub(1)..(x + 2).min(w) {
                    let v = at(xx, yy);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            if hi - lo > STEP { 1.0 } else { 0.0 }
        })
        .collect();
    let near = box_mean(&step, w, h, BAND_RADIUS + 1);
    let r = BAND_RADIUS;
    let gg: Vec<f32> = gi.iter().map(|v| v * v).collect();
    let (mean, sq) = (box_mean(gi, w, h, r), box_mean(&gg, w, h, r));
    (0..n)
        .filter(|i| near.get(*i).is_some_and(|v| *v > 0.0))
        .filter_map(|i| {
            let (m, q) = (mean.get(i).copied()?, sq.get(i).copied()?);
            Some((u32::try_from(i).ok()?, m, (q - m * m).max(0.0)))
        })
        .collect()
}

/// The depth model's terraces: a floor or wall comes out as flat plateaus with small steps between them, not as the
/// ramp it is, and a step kept as a step shows in the light as a band (and as an edge the light is refitted to
/// the photo's brightness along, so the photo's own lines print into it). So log depth `lz` (`w × h`) is
/// smoothed by a guided filter against the photo's luminance `gi` over a window of 6 % of the long side (a surface
/// of one colour takes the mean of its terraces; at the photo's own edges the filter follows them), and a pixel
/// takes that where it is within [`TERRACE`] of its own depth, and within the depths of its own side near it
/// ([`same_side`]). Steps above about 11 % of depth (a cat against the floor) stay sharp; smaller ones on one colour
/// soften, and the light there is smooth anyway.
fn deterrace(lz: &[f32], gi: &[f32], w: usize, h: usize) -> Vec<f32> {
    let r = ((w.max(h) as f32 * 0.03).round() as usize).max(2);
    let smooth = guided(gi, lz, w, h, r, 1.5e-2);
    lz.par_iter()
        .zip(&smooth)
        .enumerate()
        .map(|(i, (&a, &b))| {
            let b = if b.is_finite() { same_side(lz, w, h, i, b) } else { b };
            // (the weight of the smoothed value: 1 within a terrace of the pixel's own depth, 0 past it)
            let t = ((a - b).abs() - TERRACE.0) / (TERRACE.1 - TERRACE.0);
            let t = 1.0 - t.clamp(0.0, 1.0);
            let keep = t * t * (3.0 - 2.0 * t);
            if b.is_finite() { a + (b - a) * keep } else { a }
        })
        .collect()
}

/// Pixels each side that [`same_side`] looks over.
const SIDE_RADIUS: usize = 5;

/// `b`, the smoothed depth of pixel `i` of `lz` (`w × h`), kept within the range of the depths around it on its own
/// side of any step: the pixels within [`SIDE_RADIUS`] whose depth is within [`TERRACE`]'s upper end of its own.
/// The guided filter fits the depth to the photo's luminance over a window that holds a step, so on a flat floor
/// beside a figure it pulls the depth towards the figure's by a few percent, as far as the window reaches; that tilts
/// the normals into a halo of wrong light along the outline. The floor never has a depth outside its own.
fn same_side(lz: &[f32], w: usize, h: usize, i: usize, b: f32) -> f32 {
    let (x, y) = (i % w.max(1), i / w.max(1));
    let a = lz.get(i).copied().unwrap_or(b);
    if (b - a).abs() < 1e-3 {
        return b;
    }
    let (mut lo, mut hi) = (a, a);
    for yy in y.saturating_sub(SIDE_RADIUS)..(y + SIDE_RADIUS + 1).min(h) {
        for xx in x.saturating_sub(SIDE_RADIUS)..(x + SIDE_RADIUS + 1).min(w) {
            let v = lz.get(yy * w + xx).copied().unwrap_or(a);
            if (v - a).abs() < TERRACE.1 {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if lo <= hi { b.clamp(lo, hi) } else { b }
}

/// How far (log depth) a pixel may be from the smoothed depth and still be a terrace of it: fully smoothed under
/// `.0` (about 4 %), left alone over `.1` (about 11 %).
const TERRACE: (f32, f32) = (0.04, 0.11);

/// The air's colour of a field of log depth `lz` and luminance `y` (`colour` gives pixel `i`'s): the mean colour over the
/// farthest tenth, and the 75th percentile of the luminance (both finite, not negative).
fn air_of(lz: &[f32], y: &[f32], colour: impl Fn(usize) -> [f32; 3] + Sync) -> ([f32; 3], f32) {
    let cut = percentile(lz, 0.9);
    let (sum, n) = lz
        .par_iter()
        .enumerate()
        .filter(|(_, z)| **z >= cut)
        .map(|(i, _)| (colour(i).map(|c| if c.is_finite() { c.max(0.0) } else { 0.0 }), 1usize))
        .reduce(|| ([0.0; 3], 0), |(a, n), (b, m)| ([a[0] + b[0], a[1] + b[1], a[2] + b[2]], n + m));
    let air = if n > 0 { sum.map(|c| c / n as f32) } else { [0.18; 3] };
    let bright = percentile(y, 0.75);
    (air, if bright.is_finite() { bright.max(0.0) } else { 0.0 })
}

/// Percentile `q` (0..1) of `v` (copied; the value a full sort would put there, found in linear time).
fn percentile(v: &[f32], q: f64) -> f32 {
    let mut s: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return 0.0;
    }
    let i = (((s.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize).min(s.len() - 1);
    *s.select_nth_unstable_by(i, f32::total_cmp).1
}

/// Log depth of each value of `d`, relative to the median; values that can't be read (not finite, not positive
/// depth) take the median. `Err` when fewer than half can be read.
fn log_depth(d: &DepthMap<'_>) -> Result<Vec<f32>> {
    let max = d.values.iter().copied().filter(|v| v.is_finite()).fold(0f32, f32::max);
    let ln = |v: f32| -> Option<f32> {
        if !v.is_finite() {
            return None;
        }
        match d.kind {
            Kind::Depth => (v > 0.0).then(|| v.ln()),
            // (zero disparity is infinitely far: kept finite, a fiftieth of the nearest's disparity)
            Kind::Disparity => (v >= 0.0 && max > 0.0).then(|| -(v + 0.02 * max).ln()),
        }
    };
    let raw: Vec<Option<f32>> = d.values.iter().map(|v| ln(*v)).collect();
    let good: Vec<f32> = raw.iter().flatten().copied().collect();
    if good.len() * 2 < raw.len() {
        return Err(Error::BadDepth(format!("{} of {} depth values can't be read", raw.len() - good.len(), raw.len())));
    }
    let median = percentile(&good, 0.5);
    Ok(raw.into_iter().map(|v| v.map_or(0.0, |v| v - median)).collect())
}

/// A guide colour: each channel of linear light compressed by a cube root (about perceptual).
fn guide_colour(c: [f32; 3]) -> Guide {
    c.map(|v| v.max(0.0).cbrt())
}

impl Field {
    /// Make the field from depth map `depth` and the photo `guide` (linear Rec.2020, the size the field will
    /// have). `Err` for a depth map of another shape, too small, or mostly unreadable, and for an empty or huge
    /// guide.
    pub fn prepare(depth: DepthMap<'_>, guide: &Rgb32f) -> Result<Field> {
        Field::prepare_with(depth, guide, 0.0)
    }

    /// [`Field::prepare`] for a depth model's map, whose sides the model rounded to its patches (up to
    /// [`SHAPE_ROUNDING`] pixels each).
    pub fn prepare_model(depth: DepthMap<'_>, guide: &Rgb32f) -> Result<Field> {
        Field::prepare_with(depth, guide, SHAPE_ROUNDING)
    }

    /// [`Field::prepare`], each side of the map allowed `rounding` pixels off the photo's shape.
    fn prepare_with(depth: DepthMap<'_>, guide: &Rgb32f, rounding: f64) -> Result<Field> {
        let (w, h) = (guide.width, guide.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || guide.data.len() != w * h {
            return Err(Error::BadGuide(format!("a guide of {w} × {h}")));
        }
        let (lw, lh) = (depth.width, depth.height);
        if lw < MIN_DEPTH_SIDE || lh < MIN_DEPTH_SIDE || lw > MAX_SIDE || lh > MAX_SIDE || depth.values.len() != lw.saturating_mul(lh) {
            return Err(Error::BadDepth(format!("{} values for a depth map of {lw} × {lh}", depth.values.len())));
        }
        let (a, b) = (lw as f64 / lh as f64, w as f64 / h as f64);
        let tolerance = (SHAPE_TOLERANCE + rounding / lw.min(lh) as f64).min(SHAPE_MAX);
        if (a / b - 1.0).abs() > tolerance {
            return Err(Error::BadDepth(format!("a depth map of {lw} × {lh} for a photo of {w} × {h}")));
        }
        let lz_lo = log_depth(&depth)?;
        let near = percentile(&lz_lo, 0.02);
        let far = percentile(&lz_lo, 0.98);
        let hi_guide: Vec<Guide> = guide.data.par_iter().map(|c| guide_colour(*c)).collect();
        let lo_guide = area_down(&hi_guide, (w, h), (lw, lh));
        let ln_z = joint_bilateral_up(&lz_lo, (lw, lh), &lo_guide, &hi_guide, (w, h), &Upsample { sigma_s: 1.0, sigma_r: 0.08, split: 0.05 });
        let y: Vec<f32> = guide.data.par_iter().map(|c| lightcraft_color::luminance_2020(*c).max(1e-6)).collect();
        let lum = y.par_iter().map(|y| (y / 0.18).log2()).collect();
        let gi: Vec<f32> = y.par_iter().map(|y| y.cbrt()).collect();
        let ln_z = deterrace(&ln_z, &gi, w, h);
        let normal = normals(&ln_z, w, h);
        let band = edge_band(&ln_z, &gi, w, h);
        let (air, bright) = air_of(&ln_z, &y, |i| guide.data.get(i).copied().unwrap_or([0.18; 3]));
        let (tint, tint_top) = crate::sky::tints_of(&guide.data, w, h);
        Ok(Field {
            width: w,
            height: h,
            ln_z,
            normal,
            lum,
            near,
            far,
            gi,
            band,
            air,
            bright,
            ground: std::sync::OnceLock::new(),
            reduced: std::sync::OnceLock::new(),
            reduced_draft: std::sync::OnceLock::new(),
            tint,
            tint_top,
            sky: std::sync::OnceLock::new(),
            sky_scores: Vec::new(),
        })
    }

    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.ln_z.len() * 4
            + self.normal.len() * 12
            + self.lum.len() * 4
            + self.gi.len() * 4
            + self.band.len() * 12
            + self.reduced.get().map_or(0, crate::shadow::Reduced::bytes)
            + self.reduced_draft.get().map_or(0, crate::shadow::Reduced::bytes)
            + self.tint.len() * 12
            + self.tint_top.len() * 4
            + self.sky_scores.len() * 4
            + self.sky.get().map_or(0, crate::sky::Sky::bytes)
    }

    /// Log depth, normal and guide luminance at normalized photo coordinates `(u, v)` (bilinear).
    #[inline]
    pub(crate) fn at(&self, u: f32, v: f32) -> (f32, [f32; 3], f32) {
        let (w, h) = (self.width, self.height);
        let (x, y) = (u * w as f32, v * h as f32);
        let lz = bilinear(&self.ln_z, w, h, x, y);
        let lum = bilinear(&self.lum, w, h, x, y);
        // (normals: the nearest one, renormalized after a bilinear mix would only blur it more)
        let (Some(xm), Some(ym)) = (w.checked_sub(1), h.checked_sub(1)) else { return (lz, [0.0, 0.0, -1.0], lum) };
        let (ix, iy) = ((x.max(0.0) as usize).min(xm), (y.max(0.0) as usize).min(ym));
        let n = self.normal.get(iy * w + ix).copied().unwrap_or([0.0, 0.0, -1.0]);
        (lz, n, lum)
    }

    /// Log depth, normal and guide luminance of field pixel `i`.
    #[inline]
    pub(crate) fn at_px(&self, i: usize) -> (f32, [f32; 3], f32) {
        (self.ln_z.get(i).copied().unwrap_or(0.0), self.normal.get(i).copied().unwrap_or([0.0, 0.0, -1.0]), self.lum.get(i).copied().unwrap_or(0.0))
    }

    /// The surface normal at normalized photo coordinates `(u, v)` (pictures of the field).
    pub fn normal_at(&self, u: f32, v: f32) -> [f32; 3] {
        self.at(u, v).1
    }

    /// The camera-space point at normalized photo coordinates `(u, v)` at log depth `lz`.
    #[inline]
    pub(crate) fn point(&self, u: f32, v: f32, lz: f32) -> [f32; 3] {
        let long = self.width.max(self.height) as f32;
        let z = lz.exp();
        [(u - 0.5) * self.width as f32 / long * LENS * z, (v - 0.5) * self.height as f32 / long * LENS * z, z]
    }

    /// Near light, far dark: the field's depth as an 8-bit picture fitting `max × max` (for the section's depth
    /// box), and its size.
    pub fn thumbnail(&self, max: usize) -> (usize, usize, Vec<u8>) {
        let long = self.width.max(self.height).max(1);
        let s = (max.max(1) as f64 / long as f64).min(1.0);
        let (tw, th) = (((self.width as f64 * s).round() as usize).max(1), ((self.height as f64 * s).round() as usize).max(1));
        let span = (self.far - self.near).max(1e-3);
        let px = (0..tw * th)
            .map(|i| {
                let (x, y) = ((i % tw) as f32 + 0.5, (i / tw) as f32 + 0.5);
                let lz = bilinear(&self.ln_z, self.width, self.height, x / tw as f32 * self.width as f32, y / th as f32 * self.height as f32);
                let t = (1.0 - (lz - self.near) / span).clamp(0.0, 1.0);
                (t.powf(0.8) * 235.0 + 20.0).round() as u8
            })
            .collect();
        (tw, th, px)
    }

    /// The field at half its size each way, for a drag's renders: every per-pixel stage (the lobes, the shading, the beams'
    /// and the shadows' upsample, the glow) costs a quarter. Every number of the field is in normalized photo coordinates or
    /// relative to its long side, so the light looks the same, only softer at the photo's edges. Each 2 × 2 block takes the
    /// depth of its own pixel nearest the block's mean (a real depth, never a blend across a step) and the mean of its
    /// luminance; the normals and the edge band are made again at the new size. `None` for a field too small to halve.
    pub fn halved(&self) -> Option<Field> {
        let (w, h) = (self.width / 2, self.height / 2);
        if w < MIN_HALVED || h < MIN_HALVED {
            return None;
        }
        let block =
            |x: usize, y: usize| [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)].map(|(a, b)| b * self.width + a);
        let mean = |v: &[f32], i: [usize; 4]| i.iter().map(|&i| v.get(i).copied().unwrap_or(0.0)).sum::<f32>() / 4.0;
        let (ln_z, (lum, gi)): (Vec<f32>, (Vec<f32>, Vec<f32>)) = (0..w * h)
            .into_par_iter()
            .map(|j| {
                let i = block(j % w, j / w);
                let m = mean(&self.ln_z, i);
                let z = i.iter().filter_map(|&i| self.ln_z.get(i).copied()).min_by(|a, b| (a - m).abs().total_cmp(&(b - m).abs())).unwrap_or(m);
                (z, (mean(&self.lum, i), mean(&self.gi, i)))
            })
            .unzip();
        let normal = normals(&ln_z, w, h);
        let band = edge_band(&ln_z, &gi, w, h);
        Some(Field {
            width: w,
            height: h,
            ln_z,
            normal,
            lum,
            near: self.near,
            far: self.far,
            gi,
            band,
            air: self.air,
            bright: self.bright,
            ground: std::sync::OnceLock::new(),
            reduced: std::sync::OnceLock::new(),
            reduced_draft: std::sync::OnceLock::new(),
            // (issue #330: the reduced field keeps its cells at half size, each covering the same part of the photo, so the
            // sky's colour and scores carry over; a grid that came out another size has no sky, so no rays)
            tint: self.tint.clone(),
            tint_top: self.tint_top.clone(),
            sky: std::sync::OnceLock::new(),
            sky_scores: self.sky_scores.clone(),
        })
    }

    /// The field's log depth as a plane (tests, pictures).
    pub fn depth_plane(&self) -> Plane {
        Plane { width: self.width, height: self.height, data: self.ln_z.clone() }
    }

    /// A field from finished parts (tests): log depth relative to the median and the guide's luminance; normals
    /// are made as [`Field::prepare`] makes them.
    pub fn from_parts(width: usize, height: usize, ln_z: Vec<f32>, lum: Vec<f32>) -> Result<Field> {
        if width == 0 || height == 0 || ln_z.len() != width * height || lum.len() != ln_z.len() {
            return Err(Error::BadDepth(format!("{} values for {width} × {height}", ln_z.len())));
        }
        let gi: Vec<f32> = lum.iter().map(|l| (0.18 * l.exp2()).cbrt()).collect();
        let ln_z = deterrace(&ln_z, &gi, width, height);
        let normal = normals(&ln_z, width, height);
        let (near, far) = (percentile(&ln_z, 0.02), percentile(&ln_z, 0.98));
        let band = edge_band(&ln_z, &gi, width, height);
        let y: Vec<f32> = lum.iter().map(|l| 0.18 * l.exp2()).collect();
        let (air, bright) = air_of(&ln_z, &y, |i| [y.get(i).copied().unwrap_or(0.18); 3]);
        let grey: Vec<[f32; 3]> = lum.iter().map(|l| [0.18 * l.exp2(); 3]).collect();
        let (tint, tint_top) = crate::sky::tints_of(&grey, width, height);
        Ok(Field {
            width,
            height,
            ln_z,
            normal,
            lum,
            near,
            far,
            gi,
            band,
            air,
            bright,
            ground: std::sync::OnceLock::new(),
            reduced: std::sync::OnceLock::new(),
            reduced_draft: std::sync::OnceLock::new(),
            tint,
            tint_top,
            sky: std::sync::OnceLock::new(),
            sky_scores: Vec::new(),
        })
    }

    /// The field with a depth model's sky scores (issue #330; `w × h` over the whole photo, how sure the model is that a pixel is
    /// sky), which its rays between clouds take for the sky. Scores of a bad size or with a number that is not one are left out.
    pub fn with_sky_scores(mut self, w: usize, h: usize, scores: &[f32]) -> Field {
        if w == 0 || h == 0 || scores.len() != w.saturating_mul(h) || !scores.iter().all(|v| v.is_finite()) {
            return self;
        }
        self.sky_scores = crate::sky::scores_of(scores, (w, h), (self.width, self.height));
        self.sky = std::sync::OnceLock::new();
        self
    }

    /// The field with the guide's colour `rgb` (linear Rec.2020, the field's size) for its sky (tests; [`Field::prepare`]
    /// keeps the guide's own). `Err` for a picture of another size.
    pub fn with_colour(mut self, rgb: &[[f32; 3]]) -> Result<Field> {
        if rgb.len() != self.width * self.height {
            return Err(Error::BadGuide(format!("{} colours for {} × {}", rgb.len(), self.width, self.height)));
        }
        (self.tint, self.tint_top) = crate::sky::tints_of(rgb, self.width, self.height);
        self.sky = std::sync::OnceLock::new();
        Ok(self)
    }
}

/// Surface normals of log depth `lz` (`w × h`). Slopes come from a copy smoothed by a self-guided filter (radius
/// 1.2 % of the long side: depth changes under 3 % go, steps between near and far stay). A slope steeper than a
/// surface seen at 88° can have is a step between two things, not a surface: those pixels give no normal. The
/// normals are then averaged over a disc of 1 % of the long side (only pixels that gave one), so the depth
/// model's small blocks and ripples never show as lighting texture, and the pixels of a step take their
/// neighbours'.
fn normals(lz: &[f32], w: usize, h: usize) -> Vec<[f32; 3]> {
    let long = w.max(h);
    let n = w * h;
    let r = ((long as f32 * 0.012).round() as usize).max(2);
    let s = guided(lz, lz, w, h, r, 0.03 * 0.03);
    let e = LENS / long.max(1) as f32;
    // tan 88°: a floor at a cat's eye view leans well past 80°, a step between two things much further
    let steep = 28.6 * e;
    let at = |x: usize, y: usize| s.get(y * w + x).copied().unwrap_or(0.0);
    // the central slope, or the gentler one-sided slope when the other side is a step
    let slope = |c: f32, before: Option<f32>, after: Option<f32>| -> Option<f32> {
        let d = match (before, after) {
            (Some(b), Some(a)) => {
                let (l, r) = (c - b, a - c);
                match (l.abs() <= steep, r.abs() <= steep) {
                    (true, true) => 0.5 * (l + r),
                    (true, false) => l,
                    (false, true) => r,
                    (false, false) => return None,
                }
            }
            (Some(b), None) => c - b,
            (None, Some(a)) => a - c,
            (None, None) => 0.0,
        };
        (d.abs() <= steep).then_some(d)
    };
    // each pixel's normal, scaled by whether it has one (x, y, z, weight)
    let mut parts = [vec![0f32; n], vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    let [px, py, pz, pw] = &mut parts;
    px.par_chunks_mut(w).zip(py.par_chunks_mut(w)).zip(pz.par_chunks_mut(w)).zip(pw.par_chunks_mut(w)).enumerate().for_each(
        |(y, (((rx, ry), rz), rw))| {
            for x in 0..w {
                let c = at(x, y);
                let a = slope(c, x.checked_sub(1).map(|x| at(x, y)), (x + 1 < w).then(|| at(x + 1, y)));
                let b = slope(c, y.checked_sub(1).map(|y| at(x, y)), (y + 1 < h).then(|| at(x, y + 1)));
                let (Some(a), Some(b)) = (a, b) else { continue };
                let (xp, yp) = (((x as f32 + 0.5) - 0.5 * w as f32) / long as f32 * LENS, ((y as f32 + 0.5) - 0.5 * h as f32) / long as f32 * LENS);
                // N ∝ (a, b, −(a·x′ + b·y′ + e)) for a point z·(x′, y′, 1) whose log depth changes by a, b per pixel
                let v = [a, b, -(a * xp + b * yp + e)];
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if !(l > 0.0 && l.is_finite()) {
                    continue;
                }
                if let (Some(ox), Some(oy), Some(oz), Some(ow)) = (rx.get_mut(x), ry.get_mut(x), rz.get_mut(x), rw.get_mut(x)) {
                    (*ox, *oy, *oz, *ow) = (v[0] / l, v[1] / l, v[2] / l, 1.0);
                }
            }
        },
    );
    let rb = ((long as f32 * 0.005).round() as usize).max(1);
    // two box passes: about a disc of 1 % of the long side
    let blur = |p: &[f32]| box_mean(&box_mean(p, w, h, rb), w, h, rb);
    let (bx, by, bz, bw) = (blur(&parts[0]), blur(&parts[1]), blur(&parts[2]), blur(&parts[3]));
    (0..n)
        .into_par_iter()
        .map(|i| {
            let wgt = bw.get(i).copied().unwrap_or(0.0);
            let v = [bx.get(i).copied().unwrap_or(0.0), by.get(i).copied().unwrap_or(0.0), bz.get(i).copied().unwrap_or(0.0)];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            if wgt > 1e-4 && l > 1e-6 && l.is_finite() { v.map(|c| c / l) } else { [0.0, 0.0, -1.0] }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Light;

    fn flat_guide(w: usize, h: usize) -> Rgb32f {
        Rgb32f::filled(w, h, [0.18; 3])
    }

    #[test]
    fn malformed_or_missing_depth_is_an_error_not_a_panic() {
        let g = flat_guide(16, 12);
        let ok = vec![1.0f32; 8 * 6];
        assert!(Field::prepare(DepthMap { width: 8, height: 6, values: &ok, kind: Kind::Depth }, &g).is_ok());
        // too few values, none at all, a 1 × 1 map
        assert!(Field::prepare(DepthMap { width: 8, height: 6, values: &ok[..10], kind: Kind::Depth }, &g).is_err());
        assert!(Field::prepare(DepthMap { width: 0, height: 0, values: &[], kind: Kind::Depth }, &g).is_err());
        assert!(Field::prepare(DepthMap { width: 1, height: 1, values: &[1.0], kind: Kind::Depth }, &g).is_err());
        // another shape
        assert!(Field::prepare(DepthMap { width: 6, height: 8, values: &ok, kind: Kind::Depth }, &g).is_err());
        // mostly NaN, negative or zero depth
        let nan = vec![f32::NAN; 48];
        assert!(Field::prepare(DepthMap { width: 8, height: 6, values: &nan, kind: Kind::Depth }, &g).is_err());
        let neg = vec![-1.0f32; 48];
        assert!(Field::prepare(DepthMap { width: 8, height: 6, values: &neg, kind: Kind::Depth }, &g).is_err());
        // a few bad values are taken as the median
        let mut some = ok.clone();
        some[3] = f32::INFINITY;
        some[7] = 0.0;
        assert!(Field::prepare(DepthMap { width: 8, height: 6, values: &some, kind: Kind::Depth }, &g).is_ok());
        // an empty or mismatched guide
        let empty = Rgb32f { width: 4, height: 4, data: Vec::new() };
        assert!(Field::prepare(DepthMap { width: 8, height: 6, values: &ok, kind: Kind::Depth }, &empty).is_err());
    }

    #[test]
    fn a_depth_models_map_rounded_to_its_patches_fits_wide_photos_and_a_files_map_must_fit() {
        // the model reads a 16:10 photo at 504 × 322 (sides rounded to 14-pixel patches), a 2.35:1 one at 504 × 210
        for ((gw, gh), (lw, lh)) in [((160, 100), (504, 322)), ((235, 100), (504, 210)), ((185, 100), (504, 266))] {
            let values = vec![1.0f32; lw * lh];
            let map = DepthMap { width: lw, height: lh, values: &values, kind: Kind::Depth };
            let r = Field::prepare_model(map, &flat_guide(gw, gh));
            assert!(r.is_ok(), "{lw} × {lh} for {gw} × {gh}: {:?}", r.err());
            // a file's own map of that shape is refused: it is not rounded
            assert!(Field::prepare(map, &flat_guide(gw, gh)).is_err(), "{lw} × {lh} for {gw} × {gh}");
        }
        // nor is a small file map a little off (5:4 on a 4:3 photo)
        let values = vec![1.0f32; 200 * 160];
        assert!(Field::prepare(DepthMap { width: 200, height: 160, values: &values, kind: Kind::Disparity }, &flat_guide(400, 300)).is_err());
        // a portrait's map on a landscape photo is still refused
        let values = vec![1.0f32; 322 * 504];
        assert!(Field::prepare_model(DepthMap { width: 322, height: 504, values: &values, kind: Kind::Depth }, &flat_guide(160, 100)).is_err());
        let values = vec![1.0f32; 400 * 500];
        assert!(Field::prepare_model(DepthMap { width: 400, height: 500, values: &values, kind: Kind::Depth }, &flat_guide(125, 100)).is_err());
    }

    #[test]
    fn a_flat_wall_faces_the_camera_and_a_receding_floor_tilts_up() {
        let (w, h) = (64, 48);
        let wall = Field::from_parts(w, h, vec![0.0; w * h], vec![0.0; w * h]).unwrap();
        let (_, n, _) = wall.at(0.5, 0.5);
        assert!(n[2] < -0.99, "{n:?}");
        // depth growing upwards (a floor seen from above it): the normal points up (negative y) and to the camera
        let floor: Vec<f32> = (0..w * h).map(|i| -0.01 * (i / w) as f32).collect();
        let f = Field::from_parts(w, h, floor, vec![0.0; w * h]).unwrap();
        let (_, n, _) = f.at(0.5, 0.5);
        assert!(n[1] < -0.3 && n[2] < 0.0, "{n:?}");
    }

    #[test]
    fn a_floor_seen_at_a_grazing_angle_is_still_a_surface() {
        // a floor at the camera's feet (a cat's eye view): log depth changes 3 % a row on 240, as it does 0.6 % on
        // 1200. That is a steep slope, not a step: its normal must tilt up, not fall back to facing the camera
        // (which gave the floor patches of another light, where a window of pixels had no normal)
        let (w, h) = (320, 240);
        let lz: Vec<f32> = (0..w * h).map(|i| -0.03 * (i / w) as f32).collect();
        let f = Field::from_parts(w, h, lz, vec![0.0; w * h]).unwrap();
        for v in [0.2, 0.5, 0.8] {
            let (_, n, _) = f.at(0.5, v);
            assert!(n[1] < -0.9, "row {v}: {n:?}");
        }
    }

    #[test]
    fn a_step_between_near_and_far_is_no_surface() {
        // a near square on a far wall: normals next to its edge still face the camera
        let (w, h) = (80, 60);
        let lz: Vec<f32> = (0..w * h).map(|i| if (20..60).contains(&(i % w)) && (15..45).contains(&(i / w)) { -1.0 } else { 0.5 }).collect();
        let f = Field::from_parts(w, h, lz, vec![0.0; w * h]).unwrap();
        for x in [18usize, 19, 20, 21, 59, 60] {
            let (_, n, _) = f.at((x as f32 + 0.5) / w as f32, 0.5);
            assert!(n[2] < -0.9, "x {x}: {n:?}");
        }
    }

    #[test]
    fn a_floor_in_terraces_takes_the_same_light_as_the_smooth_floor_it_is() {
        // a floor receding up the photo whose depth comes in 8-row terraces (steps of 8 %, the depth model's way)
        // against the same floor as a ramp, under one light: the light must not follow the steps
        let (w, h) = (320, 240);
        let ramp: Vec<f32> = (0..w * h).map(|i| 0.01 * (h - i / w) as f32).collect();
        let terraces: Vec<f32> = (0..w * h).map(|i| 0.08 * ((h - i / w) / 8) as f32 + 0.04).collect();
        let lum = vec![-1.0; w * h];
        let light = Light { x: 0.3, y: 0.4, depth: 0.2, amount: 1.0, softness: 0.6, warmth: 0.5, wrap: 0.3, kind: crate::LightKind::Point };
        let lit = |lz: Vec<f32>| {
            let f = Field::from_parts(w, h, lz, lum.clone()).unwrap();
            let sh = f.shading(&f.rig(&light, 0.0, 0.0));
            sh.planes(w, h, &|x, y| (x / w as f64, y / h as f64))
        };
        let (a, b) = (lit(ramp), lit(terraces));
        // (the middle of the photo: the borders have fewer rows to average over)
        let worst = |p: &[f32], q: &[f32]| -> f32 {
            (30..h - 30).flat_map(|y| (0..w).step_by(16).map(move |x| y * w + x)).map(|i| (p[i] - q[i]).abs()).fold(0.0, f32::max)
        };
        let (stops, share) = (worst(&a.stops, &b.stops), worst(&a.share, &b.share));
        assert!(stops < 0.07, "the gain differs by {stops} stops");
        assert!(share < 0.03, "the light's colour differs by {share}");
    }

    #[test]
    fn disparity_reads_as_depth_the_other_way_round() {
        let g = flat_guide(8, 4);
        let near_left: Vec<f32> = (0..8 * 4).map(|i| if i % 8 < 4 { 1.0 } else { 0.1 }).collect();
        let f = Field::prepare(DepthMap { width: 8, height: 4, values: &near_left, kind: Kind::Disparity }, &g).unwrap();
        assert!(f.at(0.1, 0.5).0 < f.at(0.9, 0.5).0);
        let d = Field::prepare(DepthMap { width: 8, height: 4, values: &near_left, kind: Kind::Depth }, &g).unwrap();
        assert!(d.at(0.1, 0.5).0 > d.at(0.9, 0.5).0);
    }
}
