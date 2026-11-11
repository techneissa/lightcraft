//! Wrinkles v2, the search that runs once per photo ([`with_fixes`], next to Blemishes' [`crate::blemish::with_spots`]):
//! for each face with the face mesh and a Facial Skin mask, on a crop where the face is at most [`FACE_PX`] across:
//!
//! 1. the skin: Facial Skin, and the skin seen through and beside a pair of glasses ([`super::glasses`]);
//! 2. the part of the crop a wrinkle may lie in: that skin (eroded, so the hairline and the brows' edges stay out),
//!    inside a wrinkle area, and not an ear, a nostril, an eye or the face's edge ([`super::exclude`]), nor a hair
//!    ([`super::hairlike`]);
//! 3. the wrinkles: their centre lines, each zone's own widths and directions ([`super::zones`], [`super::ridge`]), and
//!    the round pigment spots (freckles, age spots), which are left as they are;
//! 4. the change that evens the skin out across the lines and keeps it along them ([`super::flatten`]), kept as the
//!    face's [`Fix`].
//!
//! A cancelled search keeps nothing (every face stays with `None`, and Wrinkles shows the old result).
//!
//! The pixels come from a [`Source`]: in the app, float crops of the photo before any retouching (the 8-bit parse picture
//! would quantise the shadows, where a wrinkle is a step or two deep).

use std::sync::Arc;

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::morph::min_filter;
use lightcraft_raster::{Plane, Rgb32f};

use super::fix::Fix;
use super::heal::{self, smoothstep};
use super::{Area, areas, flatten, glasses, hairlike, ridge, union, zones};
use crate::feature::{Scene, View, WholePhoto};
use crate::input::{Face, Input, NormRect, Part};
use crate::reduced::mean_box;

/// The longest side of the crop the search works on, pixels.
pub const WORK: usize = 2560;
/// The face's size the search works at, pixels: the finest lines (0.12 % of the face) are then a pixel and a half wide.
const FACE_PX: f64 = 1280.0;
/// A round spot is pigment when it is browner (log red over blue) than the skin around it (a blur of this fraction of
/// the face) by this much, from the first value to the second.
const PIGMENT_AROUND: f64 = 0.01;
const PIGMENT: (f32, f32) = (0.06, 0.12);
/// How far past the areas the crop reaches (a fraction of the face): the context the line search and the evening read.
const MARGIN: f64 = 0.08;
/// The smallest crop worth searching, pixels on a side (a smaller face has no wrinkles to see).
const MIN_SIDE: usize = 48;
/// A pixel darker than the skin around it (a blur of this fraction of the face) by this much (log light, from the first
/// value to the second) is not skin.
const DARK_AROUND: f64 = 0.03;
const DARK: (f32, f32) = (1.0, 1.4);
/// The skin mask's erosion, a fraction of the face: the hairline, the brows and the lips' edges stay out.
const ERODE: f64 = 0.006;

/// What a [`Filler`] gives back.
pub enum Fill {
    /// The picture with the hole filled (linear light, the crop's size).
    Done(Rgb32f),
    /// No fill to be had (no model, or it failed).
    Unavailable,
    /// The user stopped the search: every face's change is dropped.
    Cancelled,
}

/// The search's link to the engine: whether the user stopped it. (It filled the wrinkles with LaMa before; the search
/// now evens the lines out itself and no longer calls [`Filler::fill`], which the engine's LaMa filler still implements
/// until its Wrinkles plumbing is removed.)
pub trait Filler {
    /// Fill `hole` (1 where, one value per pixel) of the linear picture `rgb`.
    fn fill(&mut self, rgb: &Rgb32f, hole: &Plane) -> Fill;
    /// Whether the search was stopped.
    fn cancelled(&self) -> bool {
        false
    }
}

/// A search that can't be stopped.
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

/// [`with_fixes_from`] for a whole picture (linear light) the faces are normalised to.
pub fn with_fixes(img: &Rgb32f, faces: Vec<Face>, filler: &mut dyn Filler) -> Vec<Face> {
    with_fixes_from(&Picture(img), faces, filler)
}

/// `faces` (normalised to the photo `src` cuts) with their wrinkle change found ([`Face::wrinkles`]): `Some` for a face
/// searched (an empty one when it has no wrinkles), `None` for a face that can't be (no mesh, no Facial Skin mask, too
/// small). A cancelled search
/// ([`Filler::cancelled`]) drops every change, the ones of faces it had finished too: the photo is then "not searched".
pub fn with_fixes_from(src: &dyn Source, faces: Vec<Face>, filler: &mut dyn Filler) -> Vec<Face> {
    let (w, h) = src.size();
    let view = WholePhoto { width: w, height: h };
    let input = Input::new(faces);
    let mut found: Vec<(usize, Option<Fix>)> = Vec::new();
    {
        let scene = Scene::new(&input, &view, w, h);
        for (i, f) in scene.faces.iter().enumerate() {
            if filler.cancelled() {
                break;
            }
            found.push((f.index, find(src, &scene, i, filler)));
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
    // the face at most FACE_PX across, the crop at most WORK on its longer side
    let scale = (FACE_PX / ctx.size).min(1.0);
    let side = ((cw.max(ch) as f64 * scale).ceil() as usize).clamp(MIN_SIDE, WORK);
    let Crop { img: work, rect } = src.crop(wanted, side)?;
    let (sw, sh) = (work.width, work.height);
    if sw < MIN_SIDE / 2 || sh < MIN_SIDE / 2 || !rect.is_valid() {
        return None;
    }
    let sview = NormView { rect, w: sw as f64, h: sh as f64 };
    let sub = scene.through(&sview, sw, sh);
    let at = sub.faces.iter().position(|c| c.index == ctx.index)?;
    let sctx = sub.faces.get(at)?;
    let size = sctx.size;
    let lg = heal::log_image(&work);
    // the skin: Facial Skin, and the skin the people parse gave to a pair of glasses (behind and beside the lenses)
    let facial = sub.mask_plane(at, Part::FacialSkin)?;
    let skin = match sub.mask_plane(at, Part::Others) {
        Some(others) => facial.zip_map(&glasses::seen_through(&lg, &facial, &others, size), f32::max),
        None => facial,
    };
    let body = sub.mask_plane(at, Part::BodySkin);
    let excluded = super::exclude::excluded(sctx, sw, sh)?;
    let corner = [sctx.bounds[0].floor(), sctx.bounds[1].floor()];
    let zones = zones::zones(sctx, sw, sh, corner, sctx.px(super::LATTICE))?;

    // where a wrinkle may be: eroded skin inside an area, not an ear, a nostril, an eye or the face's edge
    let erode = (ERODE * size).ceil().max(1.0) as usize;
    let core = |p: &Plane| min_filter(&p.map(|s| smoothstep(0.5, 0.9, s)), erode);
    let face_core = core(&skin);
    let body_core = body.as_ref().map(core);
    let allowed = Plane::from_fn(sw, sh, |x, y| {
        let a = (zones.weight.get(x, y) * 4.0).min(1.0);
        let s = match zones.at(x, y) {
            Some(z) if z.area == Area::Neck => body_core.as_ref().map_or(0.0, |b| b.get(x, y)),
            Some(_) => face_core.get(x, y),
            None => 0.0,
        };
        a * s * (1.0 - excluded.get(x, y))
    });

    // a hair across the skin is left as it is: no line is looked for or read on it
    let any_skin = match &body {
        Some(b) => skin.zip_map(b, f32::max),
        None => skin.clone(),
    };
    let hair = hairlike::hair_like(&lg, &any_skin, size);
    let allowed = allowed.zip_map(&hair, |a, hl| a * (1.0 - hl));
    if filler.cancelled() {
        return None;
    }

    let lum = lg.map(|p| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]);
    // far darker than the skin around (a lash line, an eye's dark corner past where the mesh puts it, a deep fold): not
    // skin to read or to change
    let near_skin = {
        let sigma = (DARK_AROUND * size) as f32;
        let num = gaussian(&lum.zip_map(&any_skin, |l, s| l * s), sigma);
        let den = gaussian(&any_skin, sigma);
        num.zip_map(&den, |n, d| if d > 1e-3 { n / d } else { 0.0 })
    };
    let dark = lightcraft_raster::morph::max_filter(&lum.zip_map(&near_skin, |l, a| smoothstep(DARK.0, DARK.1, a - l)), 1);
    let within = allowed.clone();
    let allowed = allowed.zip_map(&dark, |a, d| a * (1.0 - d));
    let ridge::Found { centres, spots } = ridge::centres(&lum, size, &zones, &allowed);
    // a spot that matters is pigment (a freckle, an age spot: browner than the skin around), not a pore (only darker)
    let pigment = lg.map(|p| p[0] - p[2]);
    let around = gaussian(&pigment, (PIGMENT_AROUND * size) as f32);
    let spots = spots.zip_map(&pigment.zip_map(&around, |p, a| p - a), |s, d| s * smoothstep(PIGMENT.0, PIGMENT.1, d));
    // a dark pit inside a crease is the crease's, not a spot to keep
    let on_line = flatten::cores(&centres, sw, sh);
    let spots = gaussian(&lightcraft_raster::morph::max_filter(&spots.zip_map(&on_line, |s, l| s * (1.0 - l)), 1), 1.0);
    if centres.is_empty() {
        return Some(Fix::default());
    }
    // the profiles are read on the picture blurred by a little less than a pixel, so a single noisy pixel is no side
    let field = flatten::presence(&centres, sw, sh, size);
    // the skin the evening reads: not a hair, not a spot; and a spot is not changed
    let clear = hair.zip_map(&spots, |hl, sp| (1.0 - hl) * (1.0 - sp)).zip_map(&dark, |c, d| c * (1.0 - d));
    let readable = any_skin.zip_map(&clear, |s, c| smoothstep(0.5, 0.9, s) * c);
    let delta = flatten::flatten(&lg, &field, &readable, &allowed.zip_map(&spots, |a, sp| a * (1.0 - sp)), size);
    let held = spots.zip_map(&dark, f32::max);
    let delta = carry_over(&delta, &held, &within);
    if filler.cancelled() {
        return None;
    }
    Some(Fix::new(rect, &delta).unwrap_or_default())
}

/// How far a held pixel takes its neighbours' change (pixels, Gaussian sigma), and the share of its neighbourhood that
/// must be changed skin (not held) for it to take all of it.
const CARRY: f32 = 3.0;
const CARRY_FULL: (f32, f32) = (0.2, 0.5);

/// The change at the held pixels (`held`, 0…1: a freckle, a pit far darker than the skin) taken from the change round
/// them, as far as `within` lets them change: a freckle in a crease comes up with the crease and stays a freckle, rather
/// than staying down as a dark dot in the evened skin. A held patch too big to be inside a line (a lash line) keeps its
/// pixels.
fn carry_over(delta: &Rgb32f, held: &Plane, within: &Plane) -> Rgb32f {
    let n = delta.data.len();
    if held.data.len() != n || within.data.len() != n {
        return delta.clone();
    }
    let free = held.map(|h| 1.0 - h);
    let num = gaussian(&delta.zip_map(&free, |d, f| d.map(|c| c * f)), CARRY);
    let den = gaussian(&free, CARRY);
    let mut out = delta.clone();
    for (i, o) in out.data.iter_mut().enumerate() {
        let (Some(h), Some(w), Some(m), Some(d)) = (held.data.get(i), within.data.get(i), num.data.get(i), den.data.get(i)) else {
            continue;
        };
        if *h <= 0.0 || *d <= 1e-4 {
            continue;
        }
        let k = h * w * smoothstep(CARRY_FULL.0, CARRY_FULL.1, *d) / d;
        *o = [0, 1, 2].map(|c| o[c] * (1.0 - h) + m[c] * k);
    }
    out
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

    #[test]
    fn a_freckle_in_a_crease_comes_up_with_it_and_a_big_dark_patch_keeps_its_pixels() {
        // a crease lifted by 0.2 with a 3 px freckle held in it, and a 30 px held patch beside it
        let delta = Rgb32f::from_fn(120, 60, |x, _| if x < 60 { [0.2; 3] } else { [0.0; 3] });
        let held = Plane::from_fn(120, 60, |x, y| {
            let freckle = (29..32).contains(&x) && (29..32).contains(&y);
            let patch = (75..105).contains(&x) && (15..45).contains(&y);
            if freckle || patch { 1.0 } else { 0.0 }
        });
        let within = Plane::from_fn(120, 60, |_, _| 1.0);
        let out = carry_over(&delta.zip_map(&held, |d, h| d.map(|c| c * (1.0 - h))), &held, &within);
        assert!((out.get(30, 30)[1] - 0.2).abs() < 0.01, "the freckle comes up {:?}", out.get(30, 30));
        assert!(out.get(90, 30)[1].abs() < 1e-6, "the patch's middle keeps its pixels");
        assert_eq!(out.get(10, 10), [0.2; 3], "the rest as it was");
        let none = carry_over(&delta, &held, &Plane::new(120, 60));
        assert_eq!(none.get(30, 30), [0.0; 3], "not where nothing may change");
    }
}
