//! The field: what no control changes, made once per photo from its depth and a guide picture of it, and kept.
//! Log depth brought up to the guide's size edge-aware (joint-bilateral upsampling against the photo's colours,
//! so depth stops at outlines and hair), surface normals from a copy of it smoothed at a scale tied to the photo
//! (self-guided, so the steps between near and far stay steps and the depth model's small blocks and ripples go),
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
/// How far apart the depth map's and the guide's shapes may be (relative difference of their aspect ratios).
pub const SHAPE_TOLERANCE: f64 = 0.02;

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
    ln_z: Vec<f32>,
    /// Unit normals in camera space, towards the camera (negative z).
    normal: Vec<[f32; 3]>,
    /// The guide's log2 luminance relative to 18 % grey.
    lum: Vec<f32>,
    /// Log depth (relative) of the nearest and the farthest of the photo: the 2nd and 98th percentiles.
    pub near: f32,
    pub far: f32,
    /// The guide's luminance, cube root (the edge band's guide).
    pub(crate) gi: Vec<f32>,
    /// The pixels near a step in depth, each with the mean and variance of `gi` over its window.
    pub(crate) band: Vec<(u32, f32, f32)>,
}

/// The radius of the window the light is refined over along an edge (field pixels).
pub const BAND_RADIUS: usize = 3;
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

/// Percentile `q` (0..1) of `v` (copied and sorted; `v` is not empty).
fn percentile(v: &[f32], q: f64) -> f32 {
    let mut s: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(f32::total_cmp);
    let i = ((s.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    s.get(i).copied().unwrap_or(0.0)
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
        let (w, h) = (guide.width, guide.height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || guide.data.len() != w * h {
            return Err(Error::BadGuide(format!("a guide of {w} × {h}")));
        }
        let (lw, lh) = (depth.width, depth.height);
        if lw < MIN_DEPTH_SIDE || lh < MIN_DEPTH_SIDE || lw > MAX_SIDE || lh > MAX_SIDE || depth.values.len() != lw.saturating_mul(lh) {
            return Err(Error::BadDepth(format!("{} values for a depth map of {lw} × {lh}", depth.values.len())));
        }
        let (a, b) = (lw as f64 / lh as f64, w as f64 / h as f64);
        if (a / b - 1.0).abs() > SHAPE_TOLERANCE {
            return Err(Error::BadDepth(format!("a depth map of {lw} × {lh} for a photo of {w} × {h}")));
        }
        let lz_lo = log_depth(&depth)?;
        let near = percentile(&lz_lo, 0.02);
        let far = percentile(&lz_lo, 0.98);
        let hi_guide: Vec<Guide> = guide.data.par_iter().map(|c| guide_colour(*c)).collect();
        let lo_guide = area_down(&hi_guide, (w, h), (lw, lh));
        let ln_z = joint_bilateral_up(&lz_lo, (lw, lh), &lo_guide, &hi_guide, (w, h), &Upsample { sigma_s: 1.0, sigma_r: 0.08, split: 0.05 });
        let normal = normals(&ln_z, w, h);
        let y: Vec<f32> = guide.data.par_iter().map(|c| lightcraft_color::luminance_2020(*c).max(1e-6)).collect();
        let lum = y.par_iter().map(|y| (y / 0.18).log2()).collect();
        let gi: Vec<f32> = y.par_iter().map(|y| y.cbrt()).collect();
        let band = edge_band(&ln_z, &gi, w, h);
        Ok(Field { width: w, height: h, ln_z, normal, lum, near, far, gi, band })
    }

    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.ln_z.len() * 4 + self.normal.len() * 12 + self.lum.len() * 4 + self.gi.len() * 4 + self.band.len() * 12
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
        let normal = normals(&ln_z, width, height);
        let (near, far) = (percentile(&ln_z, 0.02), percentile(&ln_z, 0.98));
        let gi: Vec<f32> = lum.iter().map(|l| (0.18 * l.exp2()).cbrt()).collect();
        let band = edge_band(&ln_z, &gi, width, height);
        Ok(Field { width, height, ln_z, normal, lum, near, far, gi, band })
    }
}

/// Surface normals of log depth `lz` (`w × h`). Slopes come from a copy smoothed by a self-guided filter (radius
/// 1.2 % of the long side: depth changes under 3 % go, steps between near and far stay). A slope steeper than a
/// surface seen at 80° can have is a step between two things, not a surface: those pixels give no normal. The
/// normals are then averaged over a disc of 1 % of the long side (only pixels that gave one), so the depth
/// model's small blocks and ripples never show as lighting texture, and the pixels of a step take their
/// neighbours'.
fn normals(lz: &[f32], w: usize, h: usize) -> Vec<[f32; 3]> {
    let long = w.max(h);
    let n = w * h;
    let r = ((long as f32 * 0.012).round() as usize).max(2);
    let s = guided(lz, lz, w, h, r, 0.03 * 0.03);
    let e = LENS / long.max(1) as f32;
    // tan 80°
    let steep = 5.7 * e;
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
    fn disparity_reads_as_depth_the_other_way_round() {
        let g = flat_guide(8, 4);
        let near_left: Vec<f32> = (0..8 * 4).map(|i| if i % 8 < 4 { 1.0 } else { 0.1 }).collect();
        let f = Field::prepare(DepthMap { width: 8, height: 4, values: &near_left, kind: Kind::Disparity }, &g).unwrap();
        assert!(f.at(0.1, 0.5).0 < f.at(0.9, 0.5).0);
        let d = Field::prepare(DepthMap { width: 8, height: 4, values: &near_left, kind: Kind::Depth }, &g).unwrap();
        assert!(d.at(0.1, 0.5).0 > d.at(0.9, 0.5).0);
    }
}
