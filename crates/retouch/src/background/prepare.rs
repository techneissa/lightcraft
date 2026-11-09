//! What Background makes once from the [`Analysis`] image (the whole frame before the step, at a fixed size) and keeps
//! with it for the slider drags, loupe pans and render sizes that follow: the matte, its edge's two colours, the
//! blur's shares, the background blurred at four radii, the cleaned background and the fitted Swap picture.
//!
//! The work is kept by what it is made from, never by the pixels of a render: the analysis image's key (the source
//! and the settings before the step), the faces' input ([`Scene::input_key`]: their masks, the depth map, the Swap
//! picture) and where the analysis image lies in the photo's normalised frame. A window of the render (the zoomed
//! loupe) maps to the same analysis image, so a pan reuses it. [`KEEP`] sets are kept per analysis image.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};

use super::falloff::{self, PhotoSize};
use super::grid::{Grid, Weighted};
use super::matte::{self, EDGE};
use super::{Analysis, BLUR_MAX, Backdrop, CLEAN, Place};
use crate::input::Fnv;
use crate::{PartMask, Scene};

/// Sets of work kept per analysis image (a draft's and the loupe's faces, a depth map read meanwhile).
const KEEP: usize = 2;
/// Windows' mattes kept per set (the loupe and the draft under it).
const KEEP_WINDOWS: usize = 2;
/// Fitted Swap pictures kept per set (one per render size).
const KEEP_FITTED: usize = 2;
/// The blur's radii, as fractions of Blur's whole radius: each a disc on blocks of a third of it.
pub(crate) const LEVELS: [f64; 4] = [0.125, 0.25, 0.5, 1.0];

/// The sets of work kept with one analysis image, newest first.
#[derive(Default)]
pub(crate) struct Kept(Mutex<Vec<(u64, Arc<Prepared>)>>);

impl Kept {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(u64, Arc<Prepared>)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn bytes(&self) -> usize {
        self.lock().iter().map(|(_, p)| p.bytes()).sum()
    }
}

/// How the analysis image maps onto a render: the render's whole frame (`frame`, pixels) and the analysis size.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub aw: usize,
    pub ah: usize,
    pub w: usize,
    pub h: usize,
    pub place: Place,
}

impl Frame {
    /// Render pixel (`x`, `y`) of the image being retouched (pixel centres at whole numbers) → analysis point (pixel
    /// centres at whole numbers).
    pub fn to_analysis(self, x: usize, y: usize) -> (f32, f32) {
        let gx = (self.place.x + x as i64) as f64 + 0.5;
        let gy = (self.place.y + y as i64) as f64 + 0.5;
        ((gx * self.aw as f64 / self.w.max(1) as f64 - 0.5) as f32, (gy * self.ah as f64 / self.h.max(1) as f64 - 0.5) as f32)
    }

    /// Analysis position (`ax`, `ay`; pixel centres at +0.5) → the image's pixel position (what the view maps).
    fn to_image(self, ax: f64, ay: f64) -> (f64, f64) {
        (ax * self.w as f64 / self.aw.max(1) as f64 - self.place.x as f64, ay * self.h as f64 / self.ah.max(1) as f64 - self.place.y as f64)
    }
}

pub(crate) struct Prepared {
    pub masks: Vec<Arc<PartMask>>,
    /// The analysis image's size.
    pub aw: usize,
    pub ah: usize,
    /// The photo's size in analysis pixels.
    pub size: PhotoSize,
    /// The matte over the analysis image.
    pub alpha: Plane,
    /// The person's and the background's colours of every analysis pixel ([`matte::colours`]).
    pub person: Rgb32f,
    pub behind: Rgb32f,
    /// Blur's whole radius, in analysis pixels.
    pub radius: f64,
    shares: OnceLock<Plane>,
    levels: OnceLock<Vec<(f64, Grid)>>,
    clean: OnceLock<Grid>,
    fitted: Mutex<Vec<(u64, Arc<Fitted>)>>,
    windows: Mutex<Vec<(u64, Arc<Plane>)>>,
}

/// The Swap picture fitted to cover a render's frame: reduced to about the frame's resolution, and blurred at the
/// blur's radii on the analysis image's blocks.
pub(crate) struct Fitted {
    pub image: Arc<Rgb32f>,
    /// Frame pixel → picture pixel: `(g − frame centre) · scale + picture centre`.
    pub scale: f64,
    /// The frame's size.
    pub w: usize,
    pub h: usize,
    pub levels: OnceLock<Vec<(f64, Grid)>>,
}

impl Fitted {
    /// The picture at frame position (`gx`, `gy`) (continuous: a pixel's centre at +0.5), bilinear.
    pub fn at(&self, gx: f64, gy: f64) -> [f32; 3] {
        let (w, h) = (self.image.width, self.image.height);
        let u = (gx - self.w as f64 / 2.0) * self.scale + w as f64 / 2.0;
        let v = (gy - self.h as f64 / 2.0) * self.scale + h as f64 / 2.0;
        self.image.sample_bilinear(u as f32, v as f32)
    }
}

/// A plane read bilinearly at point (`u`, `v`) (pixel centres at whole numbers), held at the edges.
pub(crate) fn plane_at(p: &Plane, u: f32, v: f32) -> f32 {
    let (w, h) = (p.width, p.height);
    if w == 0 || h == 0 || p.data.len() != w * h || !u.is_finite() || !v.is_finite() {
        return 0.0;
    }
    let (cu, cv) = (u.clamp(0.0, (w - 1) as f32), v.clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (cu as usize, cv as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (cu - x0 as f32, cv - y0 as f32);
    let g = |x: usize, y: usize| p.data.get(y * w + x).copied().unwrap_or(0.0);
    (g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx) * (1.0 - ty) + (g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx) * ty
}

/// A point the view maps, rounded so that the same frame seen through two windows of it hashes the same.
fn hash_norm(h: &mut Fnv, (nx, ny): (f64, f64)) {
    let q = |v: f64| if v.is_finite() { (v * 1e7).round() as i64 as u64 } else { u64::MAX };
    h.u64(q(nx));
    h.u64(q(ny));
}

/// The frame of the image being retouched (`w × h`): its place in the render, or the image itself.
pub(crate) fn frame_of(analysis: &Analysis, scene: &Scene<'_>, w: usize, h: usize) -> Frame {
    let place = scene.view().place().unwrap_or(Place { x: 0, y: 0, width: w, height: h });
    Frame { aw: analysis.image.width, ah: analysis.image.height, w: place.width, h: place.height, place }
}

/// The kept work for the analysis image in `scene`, rendered as a `w × h` image, and whether it was kept; made now
/// when it is not. `None` when there is no person to work around.
pub(crate) fn prepared(analysis: &Analysis, scene: &Scene<'_>, w: usize, h: usize) -> Option<(Arc<Prepared>, bool)> {
    let frame = frame_of(analysis, scene, w, h);
    let (aw, ah) = (frame.aw, frame.ah);
    if aw == 0 || ah == 0 || analysis.image.data.len() != aw * ah || frame.w == 0 || frame.h == 0 {
        return None;
    }
    let view = scene.view();
    let to_norm = |x: f64, y: f64| {
        let (ix, iy) = frame.to_image(x, y);
        view.to_norm(ix, iy)
    };
    let mut key = Fnv::new();
    key.u64(analysis.key);
    key.u64(scene.input_key);
    key.u64(aw as u64);
    key.u64(ah as u64);
    for (x, y) in [(0.5, 0.5), (aw as f64 - 0.5, 0.5), (0.5, ah as f64 - 0.5), (aw as f64 / 2.0, ah as f64 / 2.0)] {
        hash_norm(&mut key, to_norm(x, y));
    }
    let key = key.0;
    if let Some(p) = analysis.kept.lock().iter().find(|(k, _)| *k == key).map(|(_, p)| p.clone()) {
        return Some((p, true));
    }
    let px = PhotoSize::of(scene)?;
    let size = PhotoSize { w: px.w * aw as f64 / frame.w as f64, h: px.h * ah as f64 / frame.h as f64 };
    let masks = matte::masks(scene)?;
    let alpha = matte::plane(&masks, aw, ah, &to_norm);
    let (person, behind) = matte::colours(&analysis.image, &alpha, size.long());
    let p = Arc::new(Prepared {
        masks,
        aw,
        ah,
        size,
        alpha,
        person,
        behind,
        radius: BLUR_MAX * size.long(),
        shares: OnceLock::new(),
        levels: OnceLock::new(),
        clean: OnceLock::new(),
        fitted: Mutex::new(Vec::new()),
        windows: Mutex::new(Vec::new()),
    });
    let mut kept = analysis.kept.lock();
    kept.retain(|(k, _)| *k != key);
    kept.insert(0, (key, p.clone()));
    kept.truncate(KEEP);
    Some((p, false))
}

impl Prepared {
    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        let planes = self.alpha.data.len() + self.shares.get().map_or(0, |s| s.data.len());
        let colours = self.person.data.len() + self.behind.data.len();
        let grids = |l: &[(f64, Grid)]| l.iter().map(|(_, g)| g.cells.data.len() * 16).sum::<usize>();
        let windows: usize = self.windows.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(_, p)| p.data.len() * 4).sum();
        let fitted: usize = self
            .fitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, f)| f.image.data.len() * 12 + f.levels.get().map_or(0, |l| grids(l)))
            .sum();
        planes * 4
            + colours * 12
            + self.levels.get().map_or(0, |l| grids(l))
            + self.clean.get().map_or(0, |g| g.cells.data.len() * 16)
            + windows
            + fitted
    }

    /// The matte over the image being retouched (`w × h`, mapped by the scene's view), kept for the newest windows.
    pub fn window(&self, scene: &Scene<'_>, w: usize, h: usize) -> Arc<Plane> {
        let view = scene.view();
        let mut key = Fnv::new();
        key.u64(w as u64);
        key.u64(h as u64);
        for (x, y) in [(0.5, 0.5), (w as f64 - 0.5, 0.5), (0.5, h as f64 - 0.5), (w as f64 / 2.0, h as f64 / 2.0)] {
            hash_norm(&mut key, view.to_norm(x, y));
        }
        let key = key.0;
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, p)) = windows.iter().find(|(k, _)| *k == key) {
            return p.clone();
        }
        let p = Arc::new(matte::plane(&self.masks, w, h, &|x, y| view.to_norm(x, y)));
        windows.insert(0, (key, p.clone()));
        windows.truncate(KEEP_WINDOWS);
        p
    }

    /// The background colour at analysis pixel (`x`, `y`) and its weight: the pixel where it is wholly background,
    /// the colour behind the person at the edge (weighted by how much of it is background), nothing under the person.
    fn plate(&self, x: usize, y: usize) -> Weighted {
        let i = y * self.aw + x;
        let a = self.alpha.data.get(i).copied().unwrap_or(0.0);
        if a >= EDGE.1 {
            return ([0.0; 3], 0.0);
        }
        let b = self.behind.data.get(i).copied().unwrap_or([0.0; 3]);
        if a <= EDGE.0 { (b, 1.0) } else { (b, 1.0 - a) }
    }

    /// Each analysis pixel's share of Blur's radius (the frame mapped as when the work was made).
    pub fn shares(&self, scene: &Scene<'_>, frame: &Frame) -> &Plane {
        self.shares.get_or_init(|| {
            let view = scene.view();
            falloff::shares(scene, self.size, scene.depth, self.aw, self.ah, &|x, y| {
                let (ix, iy) = frame.to_image(x, y);
                view.to_norm(ix, iy)
            })
        })
    }

    /// The background blurred at each of [`LEVELS`]: (radius in analysis pixels, blocks).
    pub fn levels(&self) -> &[(f64, Grid)] {
        self.levels.get_or_init(|| {
            LEVELS
                .iter()
                .map(|f| {
                    let r = f * self.radius;
                    let s = block(r);
                    let g = Grid::pool(self.aw, self.ah, (0, 0), s, &|x, y| self.plate(x, y));
                    (r, g.disc((r / s as f64) as f32))
                })
                .collect()
        })
    }

    /// The background smoothed by Clean Backdrop's Gaussian.
    pub fn clean(&self) -> &Grid {
        self.clean.get_or_init(|| {
            let sigma = CLEAN * self.size.long();
            let s = ((sigma / 2.0).round() as usize).max(1);
            Grid::pool(self.aw, self.ah, (0, 0), s, &|x, y| self.plate(x, y)).gauss((sigma / s as f64) as f32)
        })
    }

    /// The Swap picture fitted to a frame of `w × h` (kept while the picture and the size stay the same).
    pub fn fitted(&self, b: &Backdrop, w: usize, h: usize) -> Option<Arc<Fitted>> {
        let mut key = Fnv::new();
        key.u64(b.key);
        key.u64(w as u64);
        key.u64(h as u64);
        let key = key.0;
        let mut slot = self.fitted.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, f)) = slot.iter().find(|(k, _)| *k == key) {
            return Some(f.clone());
        }
        let f = Arc::new(fit(&b.image, w, h)?);
        slot.insert(0, (key, f.clone()));
        slot.truncate(KEEP_FITTED);
        Some(f)
    }

    /// The fitted picture blurred at each of [`LEVELS`] on the analysis image's blocks.
    pub fn picture_levels<'a>(&self, f: &'a Fitted) -> &'a [(f64, Grid)] {
        f.levels.get_or_init(|| {
            let (kx, ky) = (f.w as f64 / self.aw.max(1) as f64, f.h as f64 / self.ah.max(1) as f64);
            LEVELS
                .iter()
                .map(|lv| {
                    let r = lv * self.radius;
                    let s = block(r);
                    let mut cells = Image::<[f32; 4]>::new(self.aw.div_ceil(s), self.ah.div_ceil(s));
                    let cw = cells.width.max(1);
                    par_rows(&mut cells.data, cw, |by, row| {
                        for (bx, c) in row.iter_mut().enumerate() {
                            let (ax, ay) = ((bx as f64 + 0.5) * s as f64, (by as f64 + 0.5) * s as f64);
                            let p = f.at(ax * kx, ay * ky);
                            *c = [p[0], p[1], p[2], 1.0];
                        }
                    });
                    (r, Grid { s, b0: (0, 0), cells }.disc((r / s as f64) as f32))
                })
                .collect()
        })
    }
}

/// Luminance weights (Rec.2020).
pub(crate) fn luma(p: [f32; 3]) -> f32 {
    p[0] * 0.2627 + p[1] * 0.678 + p[2] * 0.0593
}

/// The block size for a blur of `r` pixels: a third of it, at least a pixel.
fn block(r: f64) -> usize {
    if r.is_finite() { ((r / 3.0).round() as usize).max(1) } else { 1 }
}

/// `image` scaled to cover a frame of `w × h` (centred, cut where its shape differs), reduced to about the frame's
/// resolution so it is not read finer than its pixels. `None` for an empty picture or frame.
fn fit(image: &Arc<Rgb32f>, w: usize, h: usize) -> Option<Fitted> {
    let (iw, ih) = (image.width, image.height);
    if iw == 0 || ih == 0 || image.data.len() != iw * ih || w == 0 || h == 0 {
        return None;
    }
    // picture pixels per frame pixel
    let scale = (iw as f64 / w as f64).min(ih as f64 / h as f64);
    let factor = (scale.floor() as usize).clamp(1, iw.min(ih));
    if factor == 1 {
        return Some(Fitted { image: image.clone(), scale, w, h, levels: OnceLock::new() });
    }
    let (sw, sh) = (iw / factor, ih / factor);
    let mut small = Rgb32f::new(sw.max(1), sh.max(1));
    let row_w = small.width;
    par_rows(&mut small.data, row_w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            for yy in y * factor..(y + 1) * factor {
                for p in image.data.get(yy * iw + x * factor..yy * iw + (x + 1) * factor).unwrap_or(&[]) {
                    acc = std::array::from_fn(|c| acc[c] + p[c]);
                }
            }
            *o = acc.map(|v| v / (factor * factor) as f32);
        }
    });
    Some(Fitted { image: Arc::new(small), scale: scale / factor as f64, w, h, levels: OnceLock::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plane_reads_between_its_pixels_and_holds_at_the_edges() {
        let p = Plane { width: 2, height: 1, data: vec![0.0, 1.0] };
        assert!((plane_at(&p, 0.5, 0.0) - 0.5).abs() < 1e-6);
        assert_eq!(plane_at(&p, -3.0, 0.0), 0.0);
        assert_eq!(plane_at(&p, 9.0, 4.0), 1.0);
        assert_eq!(plane_at(&p, f32::NAN, 0.0), 0.0);
        assert_eq!(plane_at(&Plane::new(0, 0), 0.0, 0.0), 0.0);
    }

    #[test]
    fn a_render_pixel_maps_onto_the_analysis_image_the_same_from_a_window() {
        let whole = Frame { aw: 100, ah: 50, w: 400, h: 200, place: Place { x: 0, y: 0, width: 400, height: 200 } };
        let win = Frame { place: Place { x: 120, y: 40, width: 400, height: 200 }, ..whole };
        assert_eq!(whole.to_analysis(130, 47), win.to_analysis(10, 7));
        let (u, v) = whole.to_analysis(1, 1);
        // (pixel 1's centre is at 1.5 of 400, 0.375 of 100: analysis point −0.125)
        assert!((u + 0.125).abs() < 1e-6 && (v - u).abs() < 1e-6, "{u} {v}");
    }
}
