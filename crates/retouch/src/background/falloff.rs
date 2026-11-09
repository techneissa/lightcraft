//! How much of Blur's radius each background pixel gets (0…1): how far behind the person it is.
//!
//! With the photo's depth map ([`super::Depth`], e.g. an iPhone Portrait photo's), the person's depth and the
//! farthest depth are measured on the whole map (the median under the person, the 2nd percentile of the whole map),
//! and a pixel gets the share of the way from the one to the other it lies at. Without one, the distance from the
//! person stands in for depth (the floor next to a person's feet is near, the far wall is far): [`NEAR`] next to the
//! person, growing to the whole radius over [`FALL`] of the person's size. The distance is measured on a grid over
//! the whole photo, in normalised coordinates, so a window of the render (the zoomed loupe) reads the same values as
//! the whole render.

use lightcraft_raster::{Plane, par_rows};

use crate::{Part, Scene};

use super::Depth;

/// The share of the radius right next to the person (without a depth map).
pub const NEAR: f32 = 0.35;
/// The distance (a fraction of the person's size, the longer side of their box) over which the blur grows from
/// [`NEAR`] to its whole radius, kept within [`FALL_MIN`]…[`FALL_MAX`] of the photo's long side.
pub const FALL: f64 = 0.4;
pub const FALL_MIN: f64 = 0.05;
pub const FALL_MAX: f64 = 0.3;
/// A cell's squared distance before the transform when it is not person (more than any distance on the grid).
const FAR: f32 = 1e10;
/// Cells on the photo's long side of the distance grid.
const GRID: usize = 256;
/// The least share of the radius with a depth map (the depth map is coarse at the hair: the background seen between
/// strands is blurred a little even where the map gives it the person's depth).
const DEPTH_FLOOR: f32 = 0.15;

/// The photo in the faces' normalised frame: its width and height in the image's pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PhotoSize {
    pub w: f64,
    pub h: f64,
}

impl PhotoSize {
    /// From the view's scale (measured across the photo's middle, so a lens warp near the corners does not count).
    pub fn of(scene: &Scene<'_>) -> Option<PhotoSize> {
        let v = scene.view();
        let d = |a: (f64, f64), b: (f64, f64)| -> Option<f64> {
            let (p, q) = (v.to_px(a.0, a.1)?, v.to_px(b.0, b.1)?);
            Some(((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt())
        };
        let w = 2.0 * d((0.25, 0.5), (0.75, 0.5))?;
        let h = 2.0 * d((0.5, 0.25), (0.5, 0.75))?;
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some(PhotoSize { w, h })
    }

    pub fn long(&self) -> f64 {
        self.w.max(self.h)
    }
}

/// The union of every face's Entire Person mask at normalised point (`nx`, `ny`).
fn person_at(scene: &Scene<'_>, nx: f64, ny: f64) -> f32 {
    scene.faces.iter().filter_map(|f| f.face.mask(Part::EntirePerson)).map(|m| m.sample(nx, ny)).fold(0.0, f32::max)
}

/// The person on a grid over the whole photo: `GRID` cells on its long side; per cell whether it is person.
struct PersonGrid {
    w: usize,
    h: usize,
    on: Vec<bool>,
}

impl PersonGrid {
    fn new(scene: &Scene<'_>, size: PhotoSize) -> PersonGrid {
        let (w, h) = (((GRID as f64 * size.w / size.long()).round() as usize).max(1), ((GRID as f64 * size.h / size.long()).round() as usize).max(1));
        let on = (0..w * h).map(|i| person_at(scene, ((i % w) as f64 + 0.5) / w as f64, ((i / w) as f64 + 0.5) / h as f64) > 0.5).collect();
        PersonGrid { w, h, on }
    }

    /// The person's size (the longer side of the box around every person cell), in the image's pixels.
    fn person_size(&self, size: PhotoSize) -> Option<f64> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for (i, on) in self.on.iter().enumerate() {
            if *on {
                let (x, y) = (i % self.w, i / self.w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
        (x1 > x0).then(|| ((x1 - x0) as f64 * size.w / self.w as f64).max((y1 - y0) as f64 * size.h / self.h as f64))
    }

    /// Each cell's distance to the nearest person cell, in cells (exact Euclidean distance transform).
    fn distances(&self) -> Vec<f32> {
        let (w, h) = (self.w, self.h);
        let mut d: Vec<f32> = self.on.iter().map(|on| if *on { 0.0 } else { FAR }).collect();
        // columns, then rows (Felzenszwalb and Huttenlocher's squared distances)
        let mut col = vec![0.0f32; h];
        for x in 0..w {
            for (y, c) in col.iter_mut().enumerate() {
                *c = d.get(y * w + x).copied().unwrap_or(FAR);
            }
            let t = edt_1d(&col);
            for (y, v) in t.into_iter().enumerate() {
                if let Some(o) = d.get_mut(y * w + x) {
                    *o = v;
                }
            }
        }
        for row in d.chunks_mut(w.max(1)) {
            let t = edt_1d(row);
            row.copy_from_slice(&t);
        }
        d.iter().map(|v| v.sqrt()).collect()
    }
}

/// The 1-D squared distance transform of `f` (squared distances, 0 at a source; Felzenszwalb and Huttenlocher's lower
/// envelope of parabolas).
fn edt_1d(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    if n == 0 {
        return Vec::new();
    }
    let at = |i: usize| f64::from(f.get(i).copied().unwrap_or(FAR));
    let (mut v, mut z) = (vec![0usize; n], vec![0f64; n + 1]);
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let cross = |q: usize, p: usize| ((at(q) + (q * q) as f64) - (at(p) + (p * p) as f64)) / (2.0 * (q as f64 - p as f64));
    for q in 1..n {
        let mut s = cross(q, v[k]);
        while k > 0 && s <= z[k] {
            k -= 1;
            s = cross(q, v[k]);
        }
        if s <= z[k] {
            // (below the first parabola everywhere)
            v[0] = q;
            z[1] = f64::INFINITY;
            continue;
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    let mut k = 0usize;
    (0..n)
        .map(|q| {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let d = q as f64 - v[k] as f64;
            (d * d + at(v[k])) as f32
        })
        .collect()
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A plane sampled bilinearly at normalised (`nx`, `ny`) (held at the edges).
pub(crate) fn sample(p: &Plane, nx: f64, ny: f64) -> f32 {
    let (w, h) = (p.width, p.height);
    if w == 0 || h == 0 || p.data.len() != w * h || !nx.is_finite() || !ny.is_finite() {
        return 0.0;
    }
    let fx = (nx * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
    let fy = (ny * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let at = |x: usize, y: usize| p.data.get(y * w + x).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
    top + (bottom - top) * ty
}

/// The depth map's person and far depths: the median of the map under the person and its 2nd percentile; `None`
/// when they are too close to tell the person from the background.
fn depth_range(scene: &Scene<'_>, depth: &Depth) -> Option<(f32, f32)> {
    let p = &depth.plane;
    let (w, h) = (p.width, p.height);
    if w == 0 || h == 0 || p.data.len() != w * h {
        return None;
    }
    let mut all: Vec<f32> = p.data.iter().copied().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)).collect();
    let mut under: Vec<f32> = Vec::new();
    // (on a grid of at most 128 cells a side: the median needs no more)
    let step = (w.max(h) / 128).max(1);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let (nx, ny) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
            if person_at(scene, nx, ny) > 0.5
                && let Some(v) = p.data.get(y * w + x).filter(|v| v.is_finite())
            {
                under.push(v.clamp(0.0, 1.0));
            }
        }
    }
    if under.is_empty() || all.is_empty() {
        return None;
    }
    let pick = |v: &mut Vec<f32>, q: f32| {
        let last = v.len().saturating_sub(1);
        let k = ((last as f32 * q).round() as usize).min(last);
        v.select_nth_unstable_by(k, f32::total_cmp);
        v.get(k).copied().unwrap_or(0.0)
    };
    let near = pick(&mut under, 0.5);
    let far = pick(&mut all, 0.02);
    (near - far > 0.05).then_some((near, far))
}

/// How far a depth map's shape (width over height) may be from the photo's before it is taken for another picture's
/// and left out (the distance from the person is used instead).
pub const DEPTH_SHAPE: f64 = 0.01;

/// Whether `depth` has the photo's shape (within [`DEPTH_SHAPE`]).
pub(crate) fn depth_fits(depth: &Depth, size: PhotoSize) -> bool {
    let (w, h) = (depth.plane.width as f64, depth.plane.height as f64);
    if w <= 0.0 || h <= 0.0 {
        return false;
    }
    let (a, b) = (w / h, size.w / size.h);
    a.is_finite() && b.is_finite() && (a / b - 1.0).abs() <= DEPTH_SHAPE
}

/// Each pixel's share of Blur's radius (0…1) over a `w × h` image whose pixel centre (`x + 0.5`, `y + 0.5`) is at
/// normalised `to_norm(x + 0.5, y + 0.5)`.
pub(crate) fn shares(
    scene: &Scene<'_>,
    size: PhotoSize,
    depth: Option<&Depth>,
    w: usize,
    h: usize,
    to_norm: &(impl Fn(f64, f64) -> (f64, f64) + Sync),
) -> Plane {
    let mut out = Plane::new(w, h);
    if let Some(d) = depth.filter(|d| depth_fits(d, size))
        && let Some((near, far)) = depth_range(scene, d)
    {
        par_rows(&mut out.data, w.max(1), |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let (nx, ny) = to_norm(x as f64 + 0.5, y as f64 + 0.5);
                let t = (near - sample(&d.plane, nx, ny)) / (near - far);
                *o = DEPTH_FLOOR + (1.0 - DEPTH_FLOOR) * smoothstep(t);
            }
        });
        return out;
    }
    let grid = PersonGrid::new(scene, size);
    let Some(person) = grid.person_size(size) else {
        out.data.fill(1.0);
        return out;
    };
    let cell = size.long() / GRID as f64;
    let fall = (FALL * person).clamp(FALL_MIN * size.long(), FALL_MAX * size.long()) / cell;
    let dist = Plane { width: grid.w, height: grid.h, data: grid.distances().into_iter().map(|d| smoothstep(d / fall as f32)).collect() };
    par_rows(&mut out.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let (nx, ny) = to_norm(x as f64 + 0.5, y as f64 + 0.5);
            *o = NEAR + (1.0 - NEAR) * sample(&dist, nx, ny);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_distance_transform_is_exact_on_a_line() {
        let f = [FAR, FAR, 0.0, FAR, FAR, FAR, 0.0];
        let d: Vec<f32> = edt_1d(&f).into_iter().map(f32::sqrt).collect();
        assert_eq!(d, vec![2.0, 1.0, 0.0, 1.0, 2.0, 1.0, 0.0]);
        assert!(edt_1d(&[]).is_empty());
        let none = edt_1d(&[FAR; 3]);
        assert!(none.iter().all(|v| *v >= FAR));
    }

    #[test]
    fn a_depth_map_of_another_shape_is_left_out() {
        let d = |w: usize, h: usize| Depth { plane: std::sync::Arc::new(Plane::new(w, h)), key: 0 };
        let size = PhotoSize { w: 3000.0, h: 2000.0 };
        assert!(depth_fits(&d(300, 200), size));
        assert!(depth_fits(&d(301, 200), size), "within a percent");
        assert!(!depth_fits(&d(200, 300), size), "turned");
        assert!(!depth_fits(&d(320, 200), size));
        assert!(!depth_fits(&d(0, 0), size));
    }
}
