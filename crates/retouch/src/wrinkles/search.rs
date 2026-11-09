//! Wrinkles v2, the search that runs once per photo ([`with_fixes`], next to Blemishes' [`crate::blemish::with_spots`]):
//! for each face with the face mesh and a Facial Skin mask, on a crop of at most [`WORK`] pixels:
//!
//! 1. the part of the crop a wrinkle may lie in: skin (eroded, so the hairline and the brows' edges stay out), inside a
//!    wrinkle area, and not an ear, a nostril or the face's edge ([`super::exclude`]);
//! 2. the wrinkle mask ([`super::ridge`]);
//! 3. the replacement skin: a [`Filler`] (LaMa, in the engine) fills the mask, and only its low band is used; with no
//!    filler (not installed, or it failed) the banded heal rebuilds the low band from the skin around ([`super::heal`]);
//!    the fine band (pores) comes back from clean skin nearby either way;
//! 4. the change from old to new, kept as the face's [`Fix`].
//!
//! A cancelled search keeps nothing (every face stays with `None`, and Wrinkles shows the old result).
//!
//! The pixels come from a [`Source`]: in the app, float crops of the photo before any retouching (the 8-bit parse picture
//! would quantise the shadows, where a wrinkle is a step or two deep).

use std::sync::Arc;

use lightcraft_raster::morph::min_filter;
use lightcraft_raster::{Plane, Rgb32f};

use super::fix::Fix;
use super::hairlike;
use super::heal::{self, smoothstep};
use super::{Area, area_weight, areas, ridge, union};
use crate::feature::{Scene, View, WholePhoto};
use crate::input::{Face, Input, NormRect, Part};
use crate::reduced::mean_box;

/// The longest side of the crop the search works on, pixels.
pub const WORK: usize = 768;
/// How far past the areas the crop reaches (a fraction of the face): the context the ridge filter and the fill read.
const MARGIN: f64 = 0.08;
/// The smallest crop worth searching, pixels on a side (a smaller face has no wrinkles to see).
const MIN_SIDE: usize = 48;
/// The skin mask's erosion, a fraction of the face: the hairline, the brows and the lips' edges stay out.
const ERODE: f64 = 0.006;

/// What a [`Filler`] gives back.
pub enum Fill {
    /// The picture with the hole filled (linear light, the crop's size).
    Done(Rgb32f),
    /// No fill to be had (no model, or it failed): the banded heal stands in.
    Unavailable,
    /// The user stopped the search: every face's change is dropped.
    Cancelled,
}

/// Fills a hole in a crop with a learned model; the engine's LaMa runs behind it.
pub trait Filler {
    /// Fill `hole` (1 where, one value per pixel) of the linear picture `rgb`.
    fn fill(&mut self, rgb: &Rgb32f, hole: &Plane) -> Fill;
    /// Whether the search was stopped.
    fn cancelled(&self) -> bool {
        false
    }
}

/// A search without a model: every face gets the banded heal.
pub struct NoFill;

impl Filler for NoFill {
    fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
        Fill::Unavailable
    }
}

/// Where a search takes its pixels from: crops of the photo before any retouching, in linear light and in floating
/// point (the engine renders them from the original; an 8-bit picture quantises the shadows, where a wrinkle's depth
/// is a step or two, so it is the fallback, [`Picture`]).
pub trait Source {
    /// The photo's size in pixels, as far as the faces' masks and boxes (normalised to it) are concerned.
    fn size(&self) -> (usize, usize);
    /// The part of the photo inside `rect` (normalised), at most `max_side` pixels on its longer side (never enlarged),
    /// with the rectangle it covers (a pixel or two off `rect`, by rounding). `None`: no pixels to be had.
    fn crop(&self, rect: NormRect, max_side: usize) -> Option<Crop>;
}

/// A crop a [`Source`] gives: its pixels (linear light) and the normalised rectangle they cover.
pub struct Crop {
    pub img: Rgb32f,
    pub rect: NormRect,
}

/// A whole picture as a [`Source`], reduced by a whole factor as the crop needs.
pub struct Picture<'a>(pub &'a Rgb32f);

impl Source for Picture<'_> {
    fn size(&self) -> (usize, usize) {
        (self.0.width, self.0.height)
    }

    fn crop(&self, rect: NormRect, max_side: usize) -> Option<Crop> {
        let (w, h) = self.size();
        if !rect.is_valid() || w == 0 || h == 0 {
            return None;
        }
        let px = |v: f64, max: usize| if v.is_finite() { (v.max(0.0) as usize).min(max) } else { 0 };
        let b = [
            px((rect.x0 * w as f64).floor(), w),
            px((rect.y0 * h as f64).floor(), h),
            px((rect.x1 * w as f64).ceil(), w),
            px((rect.y1 * h as f64).ceil(), h),
        ];
        let (cw, ch) = (b[2].saturating_sub(b[0]), b[3].saturating_sub(b[1]));
        if cw == 0 || ch == 0 {
            return None;
        }
        let f = cw.max(ch).div_ceil(max_side.max(1)).max(1);
        let img = mean_box(self.0, b, f);
        let (x1, y1) = (b[0] + img.width * f, b[1] + img.height * f);
        let rect = NormRect::new(b[0] as f64 / w as f64, b[1] as f64 / h as f64, x1 as f64 / w as f64, y1 as f64 / h as f64);
        Some(Crop { img, rect })
    }
}

/// A crop's pixels (`0..w × 0..h`) as the normalised rectangle `rect`, for mapping faces into them.
struct NormView {
    rect: NormRect,
    w: f64,
    h: f64,
}

impl View for NormView {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        (self.rect.x0 + x / self.w * self.rect.width(), self.rect.y0 + y / self.h * self.rect.height())
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let p = ((nx - self.rect.x0) / self.rect.width() * self.w, (ny - self.rect.y0) / self.rect.height() * self.h);
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }
}

/// The faces that get LaMa (the largest of them): the rest get the banded heal, so a group photo's search stays
/// bounded (LaMa takes about a second a face).
pub const LAMA_FACES: usize = 8;

/// [`with_fixes_from`] for a whole picture (linear light) the faces are normalised to.
pub fn with_fixes(img: &Rgb32f, faces: Vec<Face>, filler: &mut dyn Filler) -> Vec<Face> {
    with_fixes_from(&Picture(img), faces, filler)
}

/// `faces` (normalised to the photo `src` cuts) with their wrinkle change found ([`Face::wrinkles`]): `Some` for a face
/// searched (an empty one when it has no wrinkles), `None` for a face that can't be (no mesh, no Facial Skin mask, too
/// small). The [`LAMA_FACES`] largest faces are filled by `filler`, the others by the banded heal. A cancelled search
/// ([`Filler::cancelled`]) drops every change, the ones of faces it had finished too: the photo is then "not searched".
pub fn with_fixes_from(src: &dyn Source, faces: Vec<Face>, filler: &mut dyn Filler) -> Vec<Face> {
    let (w, h) = src.size();
    let view = WholePhoto { width: w, height: h };
    let input = Input::new(faces);
    let mut found: Vec<(usize, Option<Fix>)> = Vec::new();
    {
        let scene = Scene::new(&input, &view, w, h);
        // the faces by size, largest first: the first LAMA_FACES get the model
        let mut by_size: Vec<usize> = (0..scene.faces.len()).collect();
        by_size.sort_by(|a, b| {
            let size = |i: &usize| scene.faces.get(*i).map_or(0.0, |f| f.size);
            size(b).total_cmp(&size(a))
        });
        let lama: Vec<usize> = by_size.into_iter().take(LAMA_FACES).collect();
        for (i, f) in scene.faces.iter().enumerate() {
            if filler.cancelled() {
                break;
            }
            let fix = if lama.contains(&i) { find(src, &scene, i, filler) } else { find(src, &scene, i, &mut NoFill) };
            found.push((f.index, fix));
        }
    }
    let cancelled = filler.cancelled();
    let mut faces = input.faces;
    if !cancelled {
        for (i, fix) in found {
            if let Some(f) = faces.get_mut(i) {
                f.wrinkles = fix.map(Arc::new);
            }
        }
    }
    faces
}

/// One face's change; `None` when it can't be searched or the search was cancelled.
fn find(src: &dyn Source, scene: &Scene<'_>, face: usize, filler: &mut dyn Filler) -> Option<Fix> {
    let ctx = scene.faces.get(face)?;
    let (pw, ph) = src.size();
    let facial = scene.mask_box(face, Part::FacialSkin)?;
    let limit = scene.mask_box(face, Part::BodySkin).map_or(facial, |b| union(facial, b));
    // the crop: the four areas, grown, within the masks and the picture
    let mut reach: Option<[f64; 4]> = None;
    for area in Area::ALL {
        for r in areas::regions(ctx, area) {
            let b = r.bounds();
            reach = Some(reach.map_or(b, |u| [u[0].min(b[0]), u[1].min(b[1]), u[2].max(b[2]), u[3].max(b[3])]));
        }
    }
    let r = reach?;
    let m = ctx.px(MARGIN);
    let px = |v: f64, lo: usize, hi: usize| if v.is_finite() { (v.round().max(0.0) as usize).clamp(lo, hi) } else { lo };
    let crop =
        [px(r[0] - m, limit[0], limit[2]), px(r[1] - m, limit[1], limit[3]), px(r[2] + m, limit[0], limit[2]), px(r[3] + m, limit[1], limit[3])];
    let (cw, ch) = (crop[2].saturating_sub(crop[0]), crop[3].saturating_sub(crop[1]));
    if cw < MIN_SIDE || ch < MIN_SIDE {
        return None;
    }
    let wanted = NormRect::new(crop[0] as f64 / pw as f64, crop[1] as f64 / ph as f64, crop[2] as f64 / pw as f64, crop[3] as f64 / ph as f64);
    let Crop { img: work, rect } = src.crop(wanted, WORK)?;
    let (sw, sh) = (work.width, work.height);
    if sw < MIN_SIDE / 2 || sh < MIN_SIDE / 2 || !rect.is_valid() {
        return None;
    }
    let sview = NormView { rect, w: sw as f64, h: sh as f64 };
    let sub = scene.through(&sview, sw, sh);
    let at = sub.faces.iter().position(|c| c.index == ctx.index)?;
    let sctx = sub.faces.get(at)?;
    let size = sctx.size;
    let skin = sub.mask_plane(at, Part::FacialSkin)?;
    let body = sub.mask_plane(at, Part::BodySkin);
    let excluded = super::exclude::excluded(sctx, sw, sh)?;

    // where a wrinkle may be: eroded skin inside an area, not an ear, a nostril or the face's edge
    let corner = [sctx.bounds[0].floor(), sctx.bounds[1].floor()];
    let step = sctx.px(super::LATTICE);
    let mut in_area = Plane::new(sw, sh);
    let mut neck = Plane::new(sw, sh);
    for area in Area::ALL {
        let regions = areas::regions(sctx, area);
        if regions.is_empty() {
            continue;
        }
        let w = area_weight(&regions, [0, 0, sw, sh], corner, step);
        let target = if area == Area::Neck { &mut neck } else { &mut in_area };
        target.data.iter_mut().zip(&w.data).for_each(|(t, v)| *t = t.max(*v));
    }
    let erode = (ERODE * size).ceil().max(1.0) as usize;
    let core = |p: &Plane| min_filter(&p.map(|s| smoothstep(0.5, 0.9, s)), erode);
    let face_part = core(&skin).zip_map(&in_area, |s, a| s * (a * 4.0).min(1.0));
    let allowed = match &body {
        Some(b) => {
            let neck_part = core(b).zip_map(&neck, |s, a| s * (a * 4.0).min(1.0));
            face_part.zip_map(&neck_part, f32::max)
        }
        None => face_part,
    };
    let allowed = allowed.zip_map(&excluded, |a, e| a * (1.0 - e));

    // a hair across the skin is left as it is: out of the mask, and not skin for the fill or the donors
    let lg = heal::log_image(&work);
    let any_skin = match &body {
        Some(b) => skin.zip_map(b, f32::max),
        None => skin.clone(),
    };
    let hair = hairlike::hair_like(&lg, &any_skin, size);
    let allowed = allowed.zip_map(&hair, |a, hl| a * (1.0 - hl));
    let any_skin = any_skin.zip_map(&hair, |s, hl| s * (1.0 - hl));

    let lum = work.map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).max(1e-4).ln());
    let mask = ridge::wrinkle_mask(&lum, size, &allowed);
    let area = rect;
    if !mask.data.iter().any(|m| *m > 0.5) {
        return Some(Fix::default());
    }

    // the replacement: LaMa's low band, or the banded heal's
    let hole = mask.map(|m| if m > 0.5 { 1.0 } else { 0.0 });
    let low = match filler.fill(&work, &hole) {
        Fill::Done(fill) if fill.width == sw && fill.height == sh => heal::low_of_fill(&fill, size),
        Fill::Cancelled => return None,
        _ => heal::rebuild_low(&lg, &mask, &any_skin, size),
    };
    if filler.cancelled() {
        return None;
    }
    let clean = heal::cleanliness(&mask, &any_skin, (0.006 * size) as f32);
    let fine = heal::fine_from_neighbours(&lg, &mask, &clean, size);
    let trust = heal::trust(&mask, size);
    let delta = heal::delta(&lg, &mask, &trust, &low, &fine);
    Some(Fix::new(area, &delta).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_gives_the_crop_asked_for_reduced_by_a_whole_factor_and_never_enlarged() {
        let img = Rgb32f::from_fn(1000, 500, |x, y| [x as f32, y as f32, 0.0]);
        let want = NormRect::new(0.2, 0.2, 0.8, 0.9);
        let crop = Picture(&img).crop(want, 150).expect("a crop");
        assert!(crop.img.width.max(crop.img.height) <= 150, "{}×{}", crop.img.width, crop.img.height);
        let (r, v) = (crop.rect, want);
        assert!(r.x0 <= v.x0 + 1e-9 && r.y0 <= v.y0 + 1e-9 && r.x1 >= v.x1 - 1e-9 && r.y1 >= v.y1 - 1e-9, "covers it: {r:?}");
        // the first pixel is the mean of its block: the box starts at (200, 100) and the factor is 4
        assert!((crop.img.data[0][0] - 201.5).abs() < 1e-3 && (crop.img.data[0][1] - 101.5).abs() < 1e-3);
        // never enlarged: a small crop comes back at its own size
        let small = Picture(&img).crop(NormRect::new(0.0, 0.0, 0.05, 0.1), 600).expect("a crop");
        assert_eq!((small.img.width, small.img.height), (50, 50));
        for bad in [NormRect::new(0.5, 0.5, 0.5, 0.6), NormRect::new(f64::NAN, 0.0, 1.0, 1.0)] {
            assert!(Picture(&img).crop(bad, 100).is_none());
        }
        assert!(Picture(&Rgb32f::new(0, 0)).crop(want, 100).is_none());
    }

    #[test]
    fn a_crop_is_mapped_into_the_view_like_the_photo() {
        let v = NormView { rect: NormRect::new(0.25, 0.5, 0.75, 1.0), w: 100.0, h: 50.0 };
        assert_eq!(v.to_norm(0.0, 0.0), (0.25, 0.5));
        assert_eq!(v.to_norm(100.0, 50.0), (0.75, 1.0));
        let (x, y) = v.to_px(0.5, 0.75).expect("a point");
        assert!((x - 50.0).abs() < 1e-9 && (y - 25.0).abs() < 1e-9);
        assert!(v.to_px(f64::NAN, 0.0).is_none());
    }
}
