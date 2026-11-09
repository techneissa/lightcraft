//! Stray Hair v2: the strands outside the hair mass found with a person matte and taken away with a learned fill,
//! once per photo, kept per face as a change the Amount slider scales (`fork/retouch-hair.md`).
//!
//! For each face with the parse's Hair or Facial Skin mask, on a crop of the head cut from the full-size photo (the
//! pixels the face's search grid covers and a margin, at most [`super::MAX_CROP`] across):
//!
//! 1. a [`Remover::matte`] (Apple Vision's person segmentation, in the engine) gives the person's coverage;
//! 2. [`strand_mask`]: the matte less the solid hair mass, thin long pieces only (never over the face's skin);
//! 3. a [`Remover::fill`] (LaMa, in the engine) fills the mask on the crop reduced to at most [`MAX_WORK`] pixels, and
//!    [`replace`] turns the fill into a change: its low band, and the background's grain put back;
//! 4. the change (new over old light, not the background's colour: the step runs on the edited photo) is kept as the face's
//!    [`Removal`] ([`change`]).
//!
//! A face whose search can't run (no matte, no fill, a crop that can't be cut) keeps none, and Stray Hair v1 looks for
//! its strands as before; a face with a change keeps v1 for the skin only. A cancelled search keeps nothing from the
//! face it stopped in.

mod change;
mod replace;
mod strand_mask;

use std::sync::Arc;

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};

pub use change::Removal;

use super::grid::Grid;
use super::{Crop, MAX_CROP, PhotoRect, reduced, region};
use crate::input::Strands;
use crate::{Face, Input, NormRect, Part, Scene, View, WholePhoto};

/// The longest side of the picture the strand mask, the fill and the change work on, pixels.
pub const MAX_WORK: usize = 3072;
/// The matte under this is plain background (what a replacement is held to).
const BACKGROUND_UNDER: f32 = 0.1;
/// The smallest face searched, in pixels of the original: a smaller face has no strands a matte can find.
pub const MIN_FACE_PX: f64 = 160.0;

/// What a [`Remover`]'s fill gives back.
pub enum Fill {
    /// The picture with the hole filled (linear light, the same size).
    Done(Rgb32f),
    /// No fill to be had (no model, or it failed): the face keeps none.
    Unavailable,
    /// The user stopped the search.
    Cancelled,
}

/// What the engine lends the search: a person matte and a learned fill.
pub trait Remover {
    /// The person's coverage (0…1) of the linear picture `crop`, at any size with the crop's aspect; `None`: no matte
    /// can be had (the face then keeps none).
    fn matte(&mut self, crop: &Rgb32f) -> Option<Plane>;
    /// Fill `hole` (1 where, one value per pixel) of the linear picture `rgb`.
    fn fill(&mut self, rgb: &Rgb32f, hole: &Plane) -> Fill;
    /// Whether the search was stopped.
    fn cancelled(&self) -> bool {
        false
    }
}

/// How one face's search went.
enum Found {
    /// Not searched (too small, no parts, no crop, no matte, no fill): the face keeps none.
    Skip,
    Cancelled,
    Done(Box<Removal>),
}

/// `faces` with the strands outside their hair taken away, found on crops of the head cut from the full-size photo
/// (`photo`, its size in pixels, turned like the masks) by `crop`: a face with a change has it in
/// [`Strands::removal`] (an empty one when no strand was found).
pub fn with_removals(
    photo: (usize, usize),
    faces: Vec<Face>,
    crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>,
    tools: &mut dyn Remover,
) -> Vec<Face> {
    let input = Input::new(faces);
    let mut done: Vec<(usize, Removal)> = Vec::new();
    {
        let whole = WholePhoto { width: photo.0, height: photo.1 };
        let scene = Scene::new(&input, &whole, photo.0, photo.1);
        for i in 0..scene.faces.len() {
            if tools.cancelled() {
                break;
            }
            let Some(index) = scene.faces.get(i).map(|f| f.index) else { break };
            match find(photo, &input, &scene, i, crop, tools) {
                Found::Done(r) => done.push((index, *r)),
                Found::Cancelled => break,
                Found::Skip => {}
            }
        }
    }
    let mut faces = input.faces;
    for (i, r) in done {
        if let Some(f) = faces.get_mut(i) {
            f.strands = Some(Strands { count: r.count, removal: Some(Arc::new(r)), ..f.strands.take().unwrap_or_default() });
        }
    }
    faces
}

fn find(
    photo: (usize, usize),
    input: &Input,
    scene: &Scene<'_>,
    face: usize,
    crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>,
    tools: &mut dyn Remover,
) -> Found {
    let start = std::time::Instant::now();
    let Some(ctx) = scene.faces.get(face) else { return Found::Skip };
    let has = |p: Part| ctx.face.mask(p).is_some();
    if photo.0 == 0 || photo.1 == 0 || ctx.size.is_nan() || ctx.size < MIN_FACE_PX || (!has(Part::Hair) && !has(Part::FacialSkin)) {
        return Found::Skip;
    }
    let Some(grid) = Grid::for_face(scene, face) else { return Found::Skip };
    // the pixels the grid covers and a margin to read beside them
    let pad = (region::PAD * ctx.size).ceil() as usize + 2;
    let [x0, y0, x1, y1] = grid.px;
    let rect = [x0.saturating_sub(pad), y0.saturating_sub(pad), x1.saturating_add(pad).min(photo.0), y1.saturating_add(pad).min(photo.1)];
    let (w, h) = (rect[2].saturating_sub(rect[0]), rect[3].saturating_sub(rect[1]));
    if w == 0 || h == 0 {
        return Found::Skip;
    }
    let cut = std::time::Instant::now();
    let Some(mut img) = crop(rect).filter(|i| i.width >= w && i.height >= h) else { return Found::Skip };
    // (a crop cut larger than asked keeps its top-left corner)
    if img.width != w || img.height != h {
        img = Rgb32f::from_fn(w, h, |x, y| img.data.get(y * img.width + x).copied().unwrap_or_default());
    }
    let (nw, nh, first) = reduced(w, h, MAX_CROP);
    if first > 1.0 {
        img = resize(&img, nw, nh, Filter::Box);
    }
    let crop_ms = u32::try_from(cut.elapsed().as_millis()).unwrap_or(u32::MAX);
    if tools.cancelled() {
        return Found::Cancelled;
    }
    let Some(matte) = tools.matte(&img).filter(|m| m.width > 0 && m.height > 0 && m.data.len() == m.width.saturating_mul(m.height)) else {
        return if tools.cancelled() { Found::Cancelled } else { Found::Skip };
    };
    if tools.cancelled() {
        return Found::Cancelled;
    }

    // the picture the rest works on
    let (ww, wh, second) = reduced(img.width, img.height, MAX_WORK);
    let work = if second > 1.0 { resize(&img, ww, wh, Filter::Box) } else { img };
    let view = Crop { photo, x0: rect[0] as f64, y0: rect[1] as f64, scale: first * second };
    let scene = Scene::new(input, &view, work.width, work.height);
    let Some(at) = scene.faces.iter().position(|f| f.index == ctx.index) else { return Found::Skip };
    let Some(size) = scene.faces.get(at).map(|f| f.size).filter(|s| s.is_finite() && *s > 0.0) else { return Found::Skip };
    let matte = resize(&matte, work.width, work.height, Filter::Bilinear).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
    let planes: Vec<(Part, Option<Plane>)> =
        [Part::Hair, Part::FacialSkin, Part::BodySkin, Part::Others, Part::EntirePerson].iter().map(|p| (*p, scene.mask_plane(at, *p))).collect();
    let get = |part: Part| planes.iter().find(|(p, _)| *p == part).and_then(|(_, m)| m.as_ref());
    let parts = strand_mask::Parts {
        hair: get(Part::Hair),
        skin: get(Part::FacialSkin),
        body: get(Part::BodySkin),
        others: get(Part::Others),
        person: get(Part::EntirePerson),
    };
    let luma = Plane::from_fn(work.width, work.height, |x, y| {
        let p = work.get(x, y);
        (0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2]).max(0.0).cbrt()
    });
    let strands = strand_mask::strands(&matte, &luma, &parts, size);

    let area = norm_box(&view, work.width, work.height);
    let finish = |mut change: Removal| {
        let ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX).saturating_sub(crop_ms);
        (change.ms, change.crop_ms) = (ms, crop_ms);
        Found::Done(Box::new(change))
    };
    let Some(area) = area else { return Found::Skip };
    if strands.count == 0 {
        return finish(Removal::new(area, &Plane::new(0, 0), &Rgb32f::new(0, 0), &Rgb32f::new(0, 0), size, 0));
    }
    let filled = match tools.fill(&work, &replace::hole(&strands.mask)) {
        Fill::Done(f) if f.width == work.width && f.height == work.height => f,
        Fill::Cancelled => return Found::Cancelled,
        _ => return Found::Skip,
    };
    let weight = replace::weight(&strands.mask);
    let background = matte.map(|m| if m < BACKGROUND_UNDER { 1.0 } else { 0.0 });
    let colour = replace::colour(&work, &strands.mask, &background, &filled, size);
    let change = replace::log_ratio(&work, &colour, &weight);
    if tools.cancelled() {
        return Found::Cancelled;
    }
    finish(Removal::new(area, &weight, &change, &work, size, strands.count))
}

/// The work grid's rectangle `0..w × 0..h` as a normalised rectangle.
fn norm_box(view: &dyn View, w: usize, h: usize) -> Option<NormRect> {
    let (a, b) = (view.to_norm(0.0, 0.0), view.to_norm(w as f64, h as f64));
    let r = NormRect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
    r.is_valid().then_some(r)
}
