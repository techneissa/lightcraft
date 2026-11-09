//! Fork feature (`fork/retouch-masks.md`): portrait retouching's faces with their part masks, from the photo's
//! people parse.
//!
//! While the chosen people-parts set is downloaded, a photo's faces come from its parse (the one `parsing.run`
//! makes and caches, `crate::people_parsing`) instead of YuNet alone: per face its box, YuNet's five landmarks then
//! the 478 of the face mesh, and every part mask the parser found. All of it stays on the photo before lens
//! corrections and transform, turned by the photo's orientation ([`Input::on_photo`]); the step maps it through the
//! lens and transform warp, per pixel, as it maps local masks drawn on the photo. The masks keep the resolution the
//! models made them at, so every render samples them at its own size: a draft, the loupe and a 24 MP export agree.
//!
//! The parse runs on the face search's worker (or an export's), never in a render; the search waits for it there.

use std::sync::Arc;

use lightcraft_color::transfer::decode_srgb8;
use lightcraft_develop::retouch::{Face, Input, KeyFrame, MAX_FACES, NormRect, Part, PartMask};
use lightcraft_geom::{Orientation, Rect};
use lightcraft_parsing::{FaceHint, Landmark, Parsing, Person, RgbImage};
use lightcraft_raster::{Image, Plane, Rgb32f, Rgba8};

use super::{Found, arrange, thumb};

mod crop;
use crate::people_parsing::{Choice, Outcome, PeopleParsing};
pub(crate) use crop::Cropper;

/// Most samples in a part mask merged from several planes (one plane is kept as the parser made it).
const MAX_MERGED: usize = 1 << 20;
/// Most samples along either side of a merged mask, before its area is counted: a damaged parse (a sliver of a
/// plane, thousands of samples across a hair's width of the photo) can't size it past this.
const MAX_MERGED_SIDE: f32 = 1536.0;
/// The face mesh's points that stand in for YuNet's five when the parse has no YuNet face to match: the subject's
/// right and left iris centre, the nose tip, the right and left mouth corner.
const MESH_FIVE: [usize; 5] = [468, 473, 1, 61, 291];
/// A parsed face is the YuNet face it was parsed from when their boxes overlap this much.
const SAME_FACE_IOU: f32 = 0.5;

/// Whether Stray Hair's strands are searched on full-size crops of the original (`crop.rs`) instead of the parse's
/// picture. Off: on 13 private test faces the crops found 15 to 70 times as many "strands" on noisy small faces
/// (sensor grain, background lines), so they wait for a noise-aware or model-based detector (`fork/retouch-hair.md`).
/// The code stays and its tests turn it on by passing crops to `found_with`.
pub(crate) const FULL_SIZE_CROPS: bool = false;

/// A face search from the parse: set up on the calling thread, run on a worker.
pub(crate) struct PartsFinder {
    pub(super) prepare: crate::cmd::parsing::Prepare,
    /// What the parse-ahead index knows of the photo's parse under `choice` (`None`: the photo has gone).
    pub(super) known: Option<crate::parse_ahead::Known>,
    pub(super) parsing: PeopleParsing,
    /// The set and hair edges when the search was set up: its faces are kept under their version.
    pub(super) choice: Choice,
    pub(super) orient: Orientation,
    pub(super) regions: Vec<(NormRect, String)>,
    /// Cuts the heads' full-size crops the strands are searched on (`None`: on the parse's picture, and no strand
    /// removal).
    pub(super) cropper: Option<Box<Cropper>>,
    /// Stray Hair v2 (`fork/retouch-hair.md`): the strands outside the hair taken away with a person matte and LaMa;
    /// `None`: not made.
    pub(super) removal: Option<super::strand_removal::StrandRemoval>,
}

impl PartsFinder {
    /// Render the parse's picture and find its YuNet faces (the picker's thumbnails and the faces' five landmarks
    /// come from them); then take the parse the parse-ahead index knows for the photo (made ahead of time or by
    /// `parsing.run`, so no model runs), else the cached one, else wait for its run, and file it in the index.
    pub(super) fn find(self) -> Result<Found, String> {
        let PartsFinder { prepare, known, parsing: store, choice, orient, regions, cropper, removal } = self;
        let cut = |r| cropper.as_ref()?.crop(r);
        let crops = || cropper.as_ref().map(|c| Crops { size: c.photo_size(), cut: &cut, v1: FULL_SIZE_CROPS });
        let (img, hints) = prepare.run()?;
        if let Some(parsing) = known.as_ref().and_then(|k| k.read(&store)) {
            return Ok(found_with(&parsing, &hints, &img, orient, &regions, crops(), removal.as_ref()));
        }
        let (filed, out) = store.parse_as(choice, img.clone(), hints.clone(), true);
        let parsing = match out {
            Outcome::Ready { parsing, .. } => parsing,
            Outcome::Running => return Err("the people parse is still running".into()),
            Outcome::Failed(e) => return Err(e),
        };
        if let Some(k) = &known {
            k.file(filed);
        }
        Ok(found_with(&parsing, &hints, &img, orient, &regions, crops(), removal.as_ref()))
    }
}

/// Where Stray Hair's strands are searched: crops of the photo's original (`size`, its pixels, turned like the masks),
/// cut by `cut`.
pub(crate) struct Crops<'a> {
    pub size: (usize, usize),
    pub cut: &'a dyn Fn(lightcraft_develop::retouch::hair::PhotoRect) -> Option<Rgb32f>,
    /// Whether v1's strand search reads them too ([`FULL_SIZE_CROPS`]); else only strand removal does.
    pub v1: bool,
}

/// The faces of `parsing` (made from `img` with the faces `hints`) for the retouch step, on the photo turned by
/// `orient`, named after `regions`, with a picker thumbnail each.
#[cfg(test)]
pub(crate) fn found(parsing: &Parsing, hints: &[FaceHint], img: &RgbImage, orient: Orientation, regions: &[(NormRect, String)]) -> Found {
    found_with(parsing, hints, img, orient, regions, None, None)
}

/// [`found`] with the strands searched on `crops` of the original instead of on `img` (when `crops.v1`), and the
/// strands outside the hair taken away by `removal` (when it can run: `StrandRemoval::available`).
pub(crate) fn found_with(
    parsing: &Parsing,
    hints: &[FaceHint],
    img: &RgbImage,
    orient: Orientation,
    regions: &[(NormRect, String)],
    crops: Option<Crops<'_>>,
    removal: Option<&super::strand_removal::StrandRemoval>,
) -> Found {
    let mut faces = with_spots_and_strands(faces_of(parsing, hints, orient), img, orient, crops, removal);
    arrange(&mut faces, regions);
    let shown = oriented(&Rgba8::from_fn(img.width, img.height, |x, y| rgba(img, x, y)), orient);
    let thumbs = faces.iter().map(|f| thumb(&shown, &f.bounds)).collect();
    Found { input: Arc::new(Input::new_on_photo(faces)), thumbs: Arc::new(thumbs) }
}

/// `faces` with the spots on their skin (Blemishes) and the stray strands around and across them (Stray Hair), found
/// once here on the parse's picture, in linear light and turned like the masks, and kept with them: a render, and the
/// Amount sliders, only repair them. (Run on the face search's worker, under its activity row.)
fn with_spots_and_strands(
    faces: Vec<Face>,
    img: &RgbImage,
    orient: Orientation,
    crops: Option<Crops<'_>>,
    removal: Option<&super::strand_removal::StrandRemoval>,
) -> Vec<Face> {
    let skin = faces.iter().any(|f| f.mask(Part::FacialSkin).is_some());
    let hair = faces.iter().any(|f| f.mask(Part::Hair).is_some() || f.mask(Part::FacialSkin).is_some());
    if !skin && !hair {
        return faces;
    }
    let linear = oriented(
        &Rgb32f::from_fn(img.width, img.height, |x, y| {
            let p = rgba(img, x, y);
            [decode_srgb8(p[0]), decode_srgb8(p[1]), decode_srgb8(p[2])]
        }),
        orient,
    );
    let faces = if skin { lightcraft_develop::retouch::blemish::with_spots(&linear, faces) } else { faces };
    let crops = crops.filter(|c| same_shape(c.size, (linear.width, linear.height)));
    // the strands outside the hair: taken away here (a face it can't be done for keeps none and v1 looks for its
    // strands as before; a face that has it is looked at by v1 over its skin only)
    let faces = match (&crops, removal) {
        (Some(c), Some(r)) if r.available() => r.run(faces, c.size, &mut |rect| (c.cut)(rect)),
        _ => faces,
    };
    let Some(crops) = crops.filter(|c| c.v1) else {
        return lightcraft_develop::retouch::hair::with_strands(&linear, faces);
    };
    // a head the original can't be cut for is searched on the parse's picture, stretched to the crop's size
    let mut cut = |r: lightcraft_develop::retouch::hair::PhotoRect| {
        (crops.cut)(r).or_else(|| {
            log::warn!("portrait: no full-size crop for stray hair, searching the parse's picture");
            let (fx, fy) = (linear.width as f32 / crops.size.0.max(1) as f32, linear.height as f32 / crops.size.1.max(1) as f32);
            Some(Rgb32f::from_fn(r[2].saturating_sub(r[0]), r[3].saturating_sub(r[1]), |x, y| {
                linear.sample_bilinear((r[0] + x) as f32 * fx, (r[1] + y) as f32 * fy)
            }))
        })
    };
    lightcraft_develop::retouch::hair::with_strands_on_crops(crops.size, faces, &mut cut)
}

/// Whether two picture sizes are the same shape (within a pixel of rounding at the smaller one's size).
fn same_shape(a: (usize, usize), b: (usize, usize)) -> bool {
    let (ar, br) = (a.0 as f64 / a.1.max(1) as f64, b.0 as f64 / b.1.max(1) as f64);
    a.0 > 0 && a.1 > 0 && b.0 > 0 && b.1 > 0 && (ar / br - 1.0).abs() < 0.01
}

fn rgba(img: &RgbImage, x: usize, y: usize) -> [u8; 4] {
    let i = (y * img.width + x) * 3;
    match img.data.get(i..i + 3) {
        Some([r, g, b]) => [*r, *g, *b, 255],
        _ => [0, 0, 0, 255],
    }
}

/// One face per person of `parsing` who has a face (a person seen from behind has none, and no face to retouch),
/// largest first, at most [`MAX_FACES`], with the masks of the parts some feature reads
/// ([`lightcraft_develop::retouch::parts_read`]).
pub(crate) fn faces_of(parsing: &Parsing, hints: &[FaceHint], orient: Orientation) -> Vec<Face> {
    let read = lightcraft_develop::retouch::parts_read();
    parsing.people.iter().filter_map(|p| face_of(p, hints, orient, &read)).take(MAX_FACES).collect()
}

fn face_of(p: &Person, hints: &[FaceHint], orient: Orientation, read: &[Part]) -> Option<Face> {
    let r = p.face.filter(lightcraft_parsing::NormRect::is_valid)?;
    let upright = NormRect::new(f64::from(r.x0), f64::from(r.y0), f64::from(r.x1), f64::from(r.y1));
    let point = |x: f32, y: f32| orient.map(f64::from(x), f64::from(y), 1.0, 1.0);
    let five: Option<Vec<(f64, f64)>> = match hint_for(&r, hints).and_then(|h| h.landmarks) {
        Some(l) => Some(l.iter().map(|(x, y)| point(*x, *y)).collect()),
        None => MESH_FIVE.iter().map(|&i| p.landmarks.get(i).map(|l: &Landmark| point(l.x, l.y))).collect(),
    };
    let landmarks = match five {
        Some(mut l) => {
            l.extend(p.landmarks.iter().map(|m| point(m.x, m.y)));
            l
        }
        None => Vec::new(),
    };
    let masks = p
        .parts
        .iter()
        .map(|(part, planes)| (part_of(*part), planes))
        .filter(|(part, _)| read.contains(part))
        .filter_map(|(part, planes)| Some((part, part_mask(planes, orient)?)))
        .collect();
    Some(Face {
        bounds: map_rect(orient, upright),
        upright: Some(upright),
        landmarks,
        score: if p.face_score.is_finite() { p.face_score.clamp(0.0, 1.0) } else { 0.0 },
        person: None,
        masks,
        key_frame: KeyFrame::Photo,
        spots: None,
        strands: None,
    })
}

/// The YuNet face the parse's face `r` was parsed from.
fn hint_for<'a>(r: &lightcraft_parsing::NormRect, hints: &'a [FaceHint]) -> Option<&'a FaceHint> {
    let iou = |a: &lightcraft_parsing::NormRect, b: &lightcraft_parsing::NormRect| {
        let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
        let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
        let union = a.width() * a.height() + b.width() * b.height() - w * h;
        if union > 0.0 { w * h / union } else { 0.0 }
    };
    hints.iter().map(|h| (iou(r, &h.rect), h)).filter(|(o, _)| *o >= SAME_FACE_IOU).max_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, h)| h)
}

fn part_of(p: lightcraft_parsing::Part) -> Part {
    use lightcraft_parsing::Part as P;
    match p {
        P::EntirePerson => Part::EntirePerson,
        P::FacialSkin => Part::FacialSkin,
        P::BodySkin => Part::BodySkin,
        P::Hair => Part::Hair,
        P::Eyebrows => Part::Eyebrows,
        P::EyeWhites => Part::EyeWhites,
        P::Iris => Part::Iris,
        P::Lips => Part::Lips,
        P::Teeth => Part::Teeth,
        P::ClothesTop => Part::ClothesTop,
        P::ClothesBottom => Part::ClothesBottom,
        P::Dress => Part::Dress,
        P::Shoes => Part::Shoes,
        P::Hat => Part::Hat,
        P::Bag => Part::Bag,
        P::Others => Part::Others,
    }
}

/// A part's planes as one mask, turned by `orient`: one plane as it is; several merged (their maximum) over the box
/// holding them all, at the finest plane's resolution, at most [`MAX_MERGED_SIDE`] samples a side and
/// [`MAX_MERGED`] in all. `None` when no plane is valid.
pub(crate) fn part_mask(planes: &[lightcraft_parsing::Plane], orient: Orientation) -> Option<PartMask> {
    let valid: Vec<&lightcraft_parsing::Plane> = planes.iter().filter(|p| p.is_valid()).collect();
    let (rect, plane) = match valid.as_slice() {
        [] => return None,
        [one] => (one.rect, Plane { width: one.width, height: one.height, data: one.data.iter().map(|v| clean(*v)).collect() }),
        many => merged(many)?,
    };
    let area = NormRect::new(f64::from(rect.x0), f64::from(rect.y0), f64::from(rect.x1), f64::from(rect.y1));
    Some(PartMask { plane: oriented(&plane, orient), area: map_rect(orient, area) })
}

fn clean(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

fn merged(planes: &[&lightcraft_parsing::Plane]) -> Option<(lightcraft_parsing::NormRect, Plane)> {
    let first = planes.first()?;
    let rect = planes.iter().fold(first.rect, |u, p| u.union(&p.rect));
    // samples per unit of the photo, the finest plane's
    let dx = planes.iter().map(|p| p.width as f32 / p.rect.width()).fold(1.0f32, f32::max);
    let dy = planes.iter().map(|p| p.height as f32 / p.rect.height()).fold(1.0f32, f32::max);
    // (each side first: a sliver gives one side millions of samples and the other one)
    let side = |v: f32| if v.is_finite() { v.ceil().clamp(1.0, MAX_MERGED_SIDE) } else { MAX_MERGED_SIDE };
    let (mut w, mut h) = (side(rect.width() * dx), side(rect.height() * dy));
    let n = w * h;
    if n > MAX_MERGED as f32 {
        let k = (MAX_MERGED as f32 / n).sqrt();
        (w, h) = ((w * k).floor().max(1.0), (h * k).floor().max(1.0));
    }
    let (w, h) = (w as usize, h as usize);
    if w.saturating_mul(h) > MAX_MERGED {
        return None;
    }
    let plane = Plane::from_fn(w, h, |x, y| {
        let px = rect.x0 + (x as f32 + 0.5) / w as f32 * rect.width();
        let py = rect.y0 + (y as f32 + 0.5) / h as f32 * rect.height();
        clean(planes.iter().fold(0.0f32, |a, p| a.max(p.sample(px, py))))
    });
    Some((rect, plane))
}

fn map_rect(orient: Orientation, r: NormRect) -> NormRect {
    let m = orient.map_norm_rect(Rect { x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y1 });
    NormRect::new(m.x0, m.y0, m.x1, m.y1)
}

/// `img` turned by `orient` (each sample moved, none resampled).
pub(crate) fn oriented<T: Copy + Default>(img: &Image<T>, orient: Orientation) -> Image<T> {
    if orient == Orientation::Normal {
        return img.clone();
    }
    let (w, h) = (img.width, img.height);
    let (dw, dh) = if orient.swaps_axes() { (h, w) } else { (w, h) };
    let mut out = Image::<T>::new(dw, dh);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = orient.map(x as f64 + 0.5, y as f64 + 0.5, w as f64, h as f64);
            if !(fx >= 0.0 && fy >= 0.0) {
                continue;
            }
            let (dx, dy) = (fx as usize, fy as usize);
            if dx < dw
                && dy < dh
                && let (Some(v), Some(d)) = (img.data.get(y * w + x), out.data.get_mut(dy * dw + dx))
            {
                *d = *v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
