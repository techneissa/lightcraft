//! What Background computes once per picture and keeps for the slider drags that follow: the matte, its edge's
//! colours, the blur's shares, the background blurred at four radii, the cleaned background and the fitted Swap
//! picture. Kept for the newest [`KEEP`] pictures (by a hash of the image's pixels, the faces' Entire Person masks,
//! the view, the render's place and the depth map); pictures over [`KEEP_MAX_PX`] pixels (exports) are not kept.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};

use super::falloff::{self, PhotoSize};
use super::grid::{Grid, Weighted};
use super::matte::{self, EDGE, Edge};
use super::{BLUR_MAX, Backdrop, CLEAN, Place};
use crate::input::Fnv;
use crate::{Part, Scene};

/// Pictures kept (a draft and the loupe).
const KEEP: usize = 2;
/// Pictures larger than this are worked on and dropped.
const KEEP_MAX_PX: usize = 12_000_000;
/// The blur's radii, as fractions of Blur's whole radius: each a disc on blocks of a third of it.
pub(crate) const LEVELS: [f64; 4] = [0.125, 0.25, 0.5, 1.0];

pub(crate) struct Prepared {
    pub alpha: Plane,
    pub edge: Edge,
    pub size: PhotoSize,
    pub place: Place,
    /// Blur's whole radius, in pixels.
    pub radius: f64,
    shares: OnceLock<Plane>,
    levels: OnceLock<Vec<(f64, Grid)>>,
    clean: OnceLock<Grid>,
    grain: OnceLock<Plane>,
    backdrop: Mutex<Option<(u64, Arc<Fitted>)>>,
}

/// The Swap picture fitted to cover the render: reduced to about the render's resolution, and blurred at the
/// blur's radii on the render's blocks.
pub(crate) struct Fitted {
    pub image: Rgb32f,
    /// Render pixel → picture pixel: `(g − place centre) · scale + picture centre`.
    pub scale: f64,
    pub levels: OnceLock<Vec<(f64, Grid)>>,
}

impl Fitted {
    /// The picture at render position (`gx`, `gy`) (continuous: a pixel's centre at +0.5), bilinear.
    pub fn at(&self, place: &Place, gx: f64, gy: f64) -> [f32; 3] {
        let (w, h) = (self.image.width, self.image.height);
        let u = (gx - place.width as f64 / 2.0) * self.scale + w as f64 / 2.0;
        let v = (gy - place.height as f64 / 2.0) * self.scale + h as f64 / 2.0;
        self.image.sample_bilinear(u as f32, v as f32)
    }
}

static KEPT: Mutex<Vec<(u64, Arc<Prepared>)>> = Mutex::new(Vec::new());

/// The kept work for `img` in `scene`, made now when it is not kept; `None` when there is no person to work around.
pub(crate) fn prepared(img: &Rgb32f, scene: &Scene<'_>) -> Option<Arc<Prepared>> {
    let size = PhotoSize::of(scene)?;
    let place = scene.view().place().unwrap_or(Place { x: 0, y: 0, width: img.width, height: img.height });
    let key = key_of(img, scene, &place);
    let keep = img.width.saturating_mul(img.height) <= KEEP_MAX_PX;
    if keep && let Some(p) = KEPT.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key).map(|(_, p)| p.clone()) {
        return Some(p);
    }
    let alpha = matte::person(scene)?;
    let edge = matte::edge(img, &alpha, size.long());
    let p = Arc::new(Prepared {
        alpha,
        edge,
        size,
        place,
        radius: BLUR_MAX * size.long(),
        shares: OnceLock::new(),
        levels: OnceLock::new(),
        clean: OnceLock::new(),
        grain: OnceLock::new(),
        backdrop: Mutex::new(None),
    });
    if keep {
        let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
        kept.retain(|(k, _)| *k != key);
        kept.insert(0, (key, p.clone()));
        kept.truncate(KEEP);
    }
    Some(p)
}

/// Drop every kept picture (tests).
pub fn forget() {
    KEPT.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

/// How many pictures are kept (tests).
pub fn kept() -> usize {
    KEPT.lock().unwrap_or_else(PoisonError::into_inner).len()
}

/// A hash of what the kept work is made from.
fn key_of(img: &Rgb32f, scene: &Scene<'_>, place: &Place) -> u64 {
    let mut h = Fnv::new();
    h.u64(img.width as u64);
    h.u64(img.height as u64);
    h.u64(pixels_hash(img));
    for v in [place.x as u64, place.y as u64, place.width as u64, place.height as u64] {
        h.u64(v);
    }
    let view = scene.view();
    for (x, y) in [(0.5, 0.5), (img.width as f64, 0.5), (0.5, img.height as f64)] {
        let (nx, ny) = view.to_norm(x, y);
        h.u64(nx.to_bits());
        h.u64(ny.to_bits());
    }
    for f in &scene.faces {
        if let Some(m) = f.face.mask(Part::EntirePerson) {
            h.u64(f.index as u64);
            for v in [m.area.x0, m.area.y0, m.area.x1, m.area.y1] {
                h.u64(v.to_bits());
            }
            h.u64(m.plane.width as u64);
            h.u64(plane_hash(&m.plane.data));
        }
    }
    h.u64(view.depth().map_or(0, |d| d.key));
    h.0
}

/// A fast hash of every pixel's bits (rows hashed in parallel, then folded).
fn pixels_hash(img: &Rgb32f) -> u64 {
    let w = img.width.max(1);
    let mut rows = vec![0u64; img.height];
    par_rows(&mut rows, 1, |y, out| {
        let mut h = 0x9e37_79b9_7f4a_7c15u64 ^ y as u64;
        for p in img.data.get(y * w..(y + 1) * w).unwrap_or(&[]) {
            let a = u64::from(p[0].to_bits()) | (u64::from(p[1].to_bits()) << 32);
            h = (h ^ a).wrapping_mul(0xff51_afd7_ed55_8ccd).rotate_left(29);
            h = (h ^ u64::from(p[2].to_bits())).wrapping_mul(0xc4ce_b9fe_1a85_ec53).rotate_left(31);
        }
        if let Some(o) = out.first_mut() {
            *o = h;
        }
    });
    let mut f = Fnv::new();
    for r in rows {
        f.u64(r);
    }
    f.0
}

fn plane_hash(data: &[f32]) -> u64 {
    data.iter().fold(0x2545_f491_4f6c_dd1du64, |h, v| (h ^ u64::from(v.to_bits())).wrapping_mul(0xff51_afd7_ed55_8ccd).rotate_left(27))
}

impl Prepared {
    /// The background colour at pixel `i` of `img` and its weight: the pixel where it is wholly background, the
    /// colour behind the person at the edge (weighted by how much of it is background), nothing under the person.
    fn plate(&self, img: &Rgb32f, x: usize, y: usize) -> Weighted {
        let i = y * img.width + x;
        let a = self.alpha.data.get(i).copied().unwrap_or(0.0);
        if a <= EDGE.0 {
            return (img.data.get(i).copied().unwrap_or([0.0; 3]), 1.0);
        }
        if a >= EDGE.1 {
            return ([0.0; 3], 0.0);
        }
        let row = self.edge.row(y);
        match row.binary_search_by_key(&i, |p| p.i) {
            Ok(k) => row.get(k).map_or(([0.0; 3], 0.0), |p| (p.behind, 1.0 - a)),
            Err(_) => ([0.0; 3], 0.0),
        }
    }

    /// Each pixel's share of Blur's radius.
    pub fn shares(&self, scene: &Scene<'_>) -> &Plane {
        self.shares.get_or_init(|| falloff::shares(scene, self.size, scene.view().depth()))
    }

    /// The background blurred at each of [`LEVELS`]: (radius in pixels, blocks).
    pub fn levels(&self, img: &Rgb32f) -> &[(f64, Grid)] {
        self.levels.get_or_init(|| {
            LEVELS
                .iter()
                .map(|f| {
                    let r = f * self.radius;
                    let s = block(r);
                    let g = Grid::pool(img.width, img.height, (self.place.x, self.place.y), s, &|x, y| self.plate(img, x, y));
                    (r, g.disc((r / s as f64) as f32))
                })
                .collect()
        })
    }

    /// The background smoothed by Clean Backdrop's Gaussian.
    pub fn clean(&self, img: &Rgb32f) -> &Grid {
        self.clean.get_or_init(|| {
            let sigma = CLEAN * self.size.long();
            let s = ((sigma / 2.0).round() as usize).max(1);
            Grid::pool(img.width, img.height, (self.place.x, self.place.y), s, &|x, y| self.plate(img, x, y)).gauss((sigma / s as f64) as f32)
        })
    }

    /// The backdrop's grain Clean Backdrop keeps: each background pixel's brightness over its [`GRAIN`] blur's,
    /// faded to 1 (none) where it differs by more than grain does ([`GRAIN_MAX`]: a scuff or a dot is not grain).
    pub fn grain(&self, img: &Rgb32f) -> &Plane {
        self.grain.get_or_init(|| {
            let sigma = GRAIN * self.size.long();
            let s = ((sigma / 1.5).round() as usize).max(1);
            let g =
                Grid::pool(img.width, img.height, (self.place.x, self.place.y), s, &|x, y| self.plate(img, x, y)).gauss((sigma / s as f64) as f32);
            let (w, place) = (img.width, self.place);
            let mut out = Plane::new(img.width, img.height);
            par_rows(&mut out.data, w.max(1), |y, row| {
                for (x, o) in row.iter_mut().enumerate() {
                    let (c, wt) = self.plate(img, x, y);
                    let fine = g.read(place.x + x as i64, place.y + y as i64, 1e-3);
                    *o = match fine {
                        Some(f) if wt > 0.0 => {
                            let r = luma(c) / luma(f).max(1e-5);
                            let fade = ((r - 1.0).abs() / GRAIN_MAX - 1.0).clamp(0.0, 1.0);
                            r + (1.0 - r) * fade
                        }
                        _ => 1.0,
                    };
                }
            });
            out
        })
    }

    /// The Swap picture fitted to the render (kept while the picture stays the same).
    pub fn fitted(&self, b: &Backdrop) -> Option<Arc<Fitted>> {
        let mut slot = self.backdrop.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((k, f)) = slot.as_ref()
            && *k == b.key
        {
            return Some(f.clone());
        }
        let f = Arc::new(fit(&b.image, &self.place)?);
        *slot = Some((b.key, f.clone()));
        Some(f)
    }

    /// The fitted picture blurred at each of [`LEVELS`] on the render's blocks (the whole render's).
    pub fn picture_levels<'a>(&self, f: &'a Fitted) -> &'a [(f64, Grid)] {
        f.levels.get_or_init(|| {
            let place = self.place;
            LEVELS
                .iter()
                .map(|lv| {
                    let r = lv * self.radius;
                    let s = block(r);
                    let si = s as i64;
                    let (gw, gh) = ((place.width as i64 + si - 1) / si, (place.height as i64 + si - 1) / si);
                    let mut cells = Image::<[f32; 4]>::new(gw.max(0) as usize, gh.max(0) as usize);
                    let cw = cells.width.max(1);
                    par_rows(&mut cells.data, cw, |by, row| {
                        for (bx, c) in row.iter_mut().enumerate() {
                            let (gx, gy) = (((bx as f64) + 0.5) * s as f64, ((by as f64) + 0.5) * s as f64);
                            let p = f.at(&place, gx, gy);
                            *c = [p[0], p[1], p[2], 1.0];
                        }
                    });
                    (r, Grid { s, b0: (0, 0), cells }.disc((r / s as f64) as f32))
                })
                .collect()
        })
    }

    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        let grids = |l: Option<&Vec<(f64, Grid)>>| l.map_or(0, |l| l.iter().map(|(_, g)| g.bytes()).sum());
        self.alpha.data.len() * 4
            + self.edge.bytes()
            + self.shares.get().map_or(0, |p| p.data.len() * 4)
            + grids(self.levels.get())
            + self.clean.get().map_or(0, Grid::bytes)
            + self.grain.get().map_or(0, |p| p.data.len() * 4)
    }
}

/// The scale of the grain Clean Backdrop keeps (a Gaussian's sigma), a fraction of the photo's long side.
pub const GRAIN: f64 = 0.0015;
/// Grain changes brightness by up to this share; a pixel that differs more fades to none of it by twice this.
pub const GRAIN_MAX: f32 = 0.08;

/// Luminance weights (Rec.2020).
pub(crate) fn luma(p: [f32; 3]) -> f32 {
    p[0] * 0.2627 + p[1] * 0.678 + p[2] * 0.0593
}

/// The block size for a blur of `r` pixels: a third of it, at least a pixel.
fn block(r: f64) -> usize {
    if r.is_finite() { ((r / 3.0).round() as usize).max(1) } else { 1 }
}

/// `image` scaled to cover a render of `place`'s size (centred, cut where its shape differs), reduced to about the
/// render's resolution so it is not read finer than its pixels. `None` for an empty picture or render.
fn fit(image: &Rgb32f, place: &Place) -> Option<Fitted> {
    let (iw, ih) = (image.width, image.height);
    if iw == 0 || ih == 0 || image.data.len() != iw * ih || place.width == 0 || place.height == 0 {
        return None;
    }
    // picture pixels per render pixel
    let scale = (iw as f64 / place.width as f64).min(ih as f64 / place.height as f64);
    let factor = (scale.floor() as usize).clamp(1, iw.min(ih));
    if factor == 1 {
        return Some(Fitted { image: image.clone(), scale, levels: OnceLock::new() });
    }
    let (w, h) = (iw / factor, ih / factor);
    let mut small = Rgb32f::new(w.max(1), h.max(1));
    let sw = small.width;
    par_rows(&mut small.data, sw, |y, row| {
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
    Some(Fitted { image: small, scale: scale / factor as f64, levels: OnceLock::new() })
}
