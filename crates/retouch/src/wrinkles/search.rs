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
//! A cancelled search keeps nothing (the face stays with `None`, and Wrinkles shows the old result).

use std::sync::Arc;

use lightcraft_raster::morph::min_filter;
use lightcraft_raster::{Plane, Rgb32f};

use super::fix::Fix;
use super::hairlike;
use super::heal::{self, smoothstep};
use super::{Area, area_weight, areas, ridge, union};
use crate::feature::{Scene, View, WholePhoto};
use crate::input::{Face, Input, NormRect, Part};
use crate::reduced::{BoxView, mean_box};

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
    /// The user stopped the search: no face is finished from here on.
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

/// `faces` (normalised to `img`, the whole picture their masks were made from, in linear light) with their wrinkle
/// change found ([`Face::wrinkles`]): `Some` for a face searched (an empty one when it has no wrinkles), `None` for a
/// face that can't be (no mesh, no Facial Skin mask, too small) or that was cancelled.
pub fn with_fixes(img: &Rgb32f, faces: Vec<Face>, filler: &mut dyn Filler) -> Vec<Face> {
    let view = WholePhoto { width: img.width, height: img.height };
    let input = Input::new(faces);
    let mut found: Vec<(usize, Option<Fix>)> = Vec::new();
    {
        let scene = Scene::new(&input, &view, img.width, img.height);
        for (i, f) in scene.faces.iter().enumerate() {
            if filler.cancelled() {
                break;
            }
            found.push((f.index, find(img, &scene, i, filler)));
        }
    }
    let mut faces = input.faces;
    for (i, fix) in found {
        if let Some(f) = faces.get_mut(i) {
            f.wrinkles = fix.map(Arc::new);
        }
    }
    faces
}

/// One face's change; `None` when it can't be searched or the search was cancelled.
fn find(img: &Rgb32f, scene: &Scene<'_>, face: usize, filler: &mut dyn Filler) -> Option<Fix> {
    let ctx = scene.faces.get(face)?;
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
    let f = cw.max(ch).div_ceil(WORK).max(1);
    let work = mean_box(img, crop, f);
    let (sw, sh) = (work.width, work.height);
    let sview = BoxView::new(scene.view(), [crop[0] as f64, crop[1] as f64], f as f64);
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
    let area = norm_box(&sview, sw, sh)?;
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
    let delta = heal::delta(&lg, &mask, &low, &fine);
    Some(Fix::new(area, &delta).unwrap_or_default())
}

/// The crop's rectangle `0..w × 0..h` as a normalised rectangle.
fn norm_box(view: &dyn View, w: usize, h: usize) -> Option<NormRect> {
    let (a, b) = (view.to_norm(0.0, 0.0), view.to_norm(w as f64, h as f64));
    let r = NormRect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
    r.is_valid().then_some(r)
}
