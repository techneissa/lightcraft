//! Stray Hair (fork feature, `fork/retouch-hair.md`): flyaways at the hair's outer edge and stray strands across
//! the face, cleaned per face with one Amount slider (0…100, off at 0).
//!
//! Two steps, as Blemishes finds its spots once and repairs them in every render:
//!
//! - **The search, once per photo** ([`with_strands`]): the face provider runs it with the people parse, on the
//!   parse's picture (the photo before any retouching, at the parse's fixed size), and keeps what it finds with each
//!   face ([`crate::Face::strands`]), in the photo's normalised frame like the masks. A render never searches: a
//!   face whose search has not run yet gets no Stray Hair.
//!   1. **Where** (`region`): on a coarse grid anchored to the face, from the part masks. The main hair mass is the
//!      Hair mask opened at a few hundredths of the face (thin strands drop out of it, the mass stays); strands are
//!      looked for in a band outside it (the Hair matte's soft fringe and past it, so the chosen hair edge decides
//!      where the hair ends) and over the skin, never in the mass, on eyebrows, eyes and lashes, lips and teeth,
//!      glasses and jewellery (Others), or clothes. The one place an area's extent is decided: the Refine brush (#99)
//!      adds or takes away its strokes in `region::weights` (a search), or masks the repair in a render.
//!   2. **Detect** (`detect`): on a grid of cells a fixed share of the face ([`CELLS_PER_FACE`]), an oriented ridge
//!      filter (the Hessian of the luminance's cube root, blurred at a strand's width) finds thin lines; a line is
//!      kept when it stands out from both sides alike (a step, such as the face's or the hair's outline, does not),
//!      is long along its own direction, and stands out from the texture around it (a beard, curls, foliage, a knit
//!      do not). Faces under [`MIN_FACE_PX`] pixels of the searched picture are not searched.
//! - **The repair, every render** (`repair`): each strand pixel is filled from the pixels beside it as the render
//!   has them: samples on both sides across the strand, averaged along it, the side least like the strand weighted
//!   most; a pixel changes by how much it differs from its fill, so the texture beside a strand stays. A Skin drag
//!   or an exposure change re-runs only this.
//!
//! What makes the search run again: a new people parse (another parts set, another hair edge, or another photo
//! version, which the provider keeps its faces by). Sizes are fractions of the face and the strands are kept in the
//! photo's frame, so a draft, the loupe and an export repair the same strands; a zoomed window renders what the
//! repairs read ([`Hair::REACH`]).

mod detect;
mod grid;
mod region;
mod repair;

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::input::{Searched, Strands};
use crate::{Control, Face, Feature, Input, NormRect, Part, Scene, Scope, Spec, Stage, WholePhoto};

/// The Stray Hair settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hair {
    /// 0…100; 0 is off.
    pub amount: f64,
}

/// Cells of the detection grid across the face's size (a strand is about one cell wide), no finer than a pixel of the
/// searched picture. On a head crop cut at full size that is about 2.5 px for a face a thousand pixels across; finer cells took a second a face and counted grain.
pub const CELLS_PER_FACE: f64 = 400.0;
/// The smallest face searched, in pixels of the searched picture: a smaller face's strands are thinner than a pixel
/// there, and are left alone in every render.
pub const MIN_FACE_PX: f64 = 120.0;

/// A slider's value as 0…1 (0 for a value that is not a number).
fn unit(v: f64) -> f32 {
    if v.is_finite() { (v.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 }
}

impl Feature for Hair {
    const KEY: &'static str = "hair";
    const LABEL: &'static str = "Stray Hair";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const PARTS: &'static [Part] = region::PARTS;
    /// Strands lie within the face's box and the Hair and Facial Skin masks' areas grown by `region::BAND`; a repair
    /// reads `region::PAD` past them (the samples beside a strand).
    const REACH: f64 = region::BAND + region::PAD;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.hair.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        let amount = unit(self.amount);
        if amount <= 0.0 || img.width == 0 || img.height == 0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(strands) = scene.faces.get(face).and_then(|f| f.face.strands.as_ref()) else { return };
        if strands.is_empty() {
            return;
        }
        let fills = repair::fills(img, scene.view(), strands);
        repair::blend(img, &fills, amount);
    }
}

/// `faces` with the stray strands around and across each, found on `img`: the picture their masks come from (the
/// people parse's, in linear light, turned like the masks), which is the whole photo. Each face gets its
/// [`Face::strands`] (empty when none were found or the face was not searched; [`Strands::searched`] says which).
/// [`with_strands_on_crops`] is what the face provider runs; this one is for a photo already small enough.
pub fn with_strands(img: &Rgb32f, faces: Vec<Face>) -> Vec<Face> {
    let photo = Crop { photo: (img.width, img.height), x0: 0.0, y0: 0.0, scale: 1.0 };
    let input = Input::new(faces);
    let found: Vec<(usize, Strands)> = {
        let view = WholePhoto { width: img.width, height: img.height };
        let scene = Scene::new(&input, &view, img.width, img.height);
        scene.faces.iter().enumerate().map(|(i, f)| (f.index, search(img, &scene, i, &photo))).collect()
    };
    keep(input, found)
}

/// The most pixels along either side of a head's crop: a larger one is reduced to this first.
pub const MAX_CROP: usize = 3072;

/// A crop of the photo's pixels, cut from the full-size picture: `[x0, y0, x1, y1)` in the photo's pixels (turned like
/// the masks).
pub type PhotoRect = [usize; 4];

/// `faces` with the stray strands around and across each, found on a crop of the head cut from the full-size photo
/// (`photo`, its size in pixels, turned like the masks): the face's box grown to hold the hair band, cut by `crop`
/// (which returns the crop's pixels, in linear light, turned like the masks, at least that size; `None`: it could not
/// be cut) and reduced to [`MAX_CROP`] pixels along its longer side. A face's strands then keep the pixels the camera
/// made them with, and are kept in the photo's normalised frame like those of [`with_strands`].
pub fn with_strands_on_crops(photo: (usize, usize), faces: Vec<Face>, crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>) -> Vec<Face> {
    on_crops(photo, faces, crop, MAX_CROP)
}

fn on_crops(photo: (usize, usize), faces: Vec<Face>, crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>, max_crop: usize) -> Vec<Face> {
    let input = Input::new(faces);
    let found: Vec<(usize, Strands)> = {
        let whole = WholePhoto { width: photo.0, height: photo.1 };
        let scene = Scene::new(&input, &whole, photo.0, photo.1);
        scene.faces.iter().enumerate().map(|(i, f)| (f.index, search_crop(photo, &input, &scene, i, crop, max_crop))).collect()
    };
    keep(input, found)
}

/// `input`'s faces with their `found` strands (every face gets some: empty when not searched).
fn keep(input: Input, found: Vec<(usize, Strands)>) -> Vec<Face> {
    let mut faces = input.faces;
    for f in &mut faces {
        f.strands = Some(Strands::default());
    }
    for (i, s) in found {
        if let Some(f) = faces.get_mut(i) {
            f.strands = Some(s);
        }
    }
    faces
}

/// A search's picture: the photo's pixels `[x0, y0, …)` reduced by `scale` (1: as they are). Its pixel (`x`, `y`) is
/// the photo's normalised `((x0 + x × scale) / width, (y0 + y × scale) / height)`.
#[derive(Clone, Copy, Debug)]
struct Crop {
    photo: (usize, usize),
    x0: f64,
    y0: f64,
    scale: f64,
}

impl crate::View for Crop {
    fn to_norm(&self, x: f64, y: f64) -> (f64, f64) {
        ((self.x0 + x * self.scale) / self.photo.0.max(1) as f64, (self.y0 + y * self.scale) / self.photo.1.max(1) as f64)
    }

    fn to_px(&self, nx: f64, ny: f64) -> Option<(f64, f64)> {
        let p = ((nx * self.photo.0 as f64 - self.x0) / self.scale, (ny * self.photo.1 as f64 - self.y0) / self.scale);
        (p.0.is_finite() && p.1.is_finite()).then_some(p)
    }
}

impl Crop {
    /// The size of the photo reduced as the crop is: the frame a strand's direction and distances are measured in.
    fn picture(&self) -> (usize, usize) {
        let r = |v: usize| ((v as f64 / self.scale).round() as usize).max(1);
        (r(self.photo.0), r(self.photo.1))
    }
}

/// The strands of face `face` of `scene` (a scene over the whole photo), searched on the crop of its head.
fn search_crop(
    photo: (usize, usize),
    input: &Input,
    scene: &Scene<'_>,
    face: usize,
    crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>,
    max_crop: usize,
) -> Strands {
    let start = std::time::Instant::now();
    let Some(ctx) = scene.faces.get(face) else { return Strands::default() };
    if let Some(skipped) = skipped(ctx, photo) {
        return skipped;
    }
    let Some(grid) = grid::Grid::for_face(scene, face) else {
        return Strands { searched: Searched::NoParts, face_px: face_px(ctx.size), ..Strands::default() };
    };
    // the pixels the grid covers and a margin to read beside them
    let pad = (region::PAD * ctx.size).ceil() as usize + 2;
    let [x0, y0, x1, y1] = grid.px;
    let rect = [x0.saturating_sub(pad), y0.saturating_sub(pad), x1.saturating_add(pad).min(photo.0), y1.saturating_add(pad).min(photo.1)];
    let (w, h) = (rect[2].saturating_sub(rect[0]), rect[3].saturating_sub(rect[1]));
    if w == 0 || h == 0 {
        return Strands { face_px: face_px(ctx.size), ..Strands::default() };
    }
    let cut = std::time::Instant::now();
    let Some(mut img) = crop(rect).filter(|i| i.width >= w && i.height >= h) else {
        return Strands { face_px: face_px(ctx.size), ..Strands::default() };
    };
    // (a crop cut larger than asked keeps its top-left corner)
    if img.width != w || img.height != h {
        img = Rgb32f::from_fn(w, h, |x, y| img.data.get(y * img.width + x).copied().unwrap_or_default());
    }
    let (nw, nh, scale) = reduced(w, h, max_crop);
    if scale > 1.0 {
        img = lightcraft_raster::resample::resize(&img, nw, nh, lightcraft_raster::resample::Filter::Box);
    }
    let crop_ms = u32::try_from(cut.elapsed().as_millis()).unwrap_or(u32::MAX);
    let view = Crop { photo, x0: rect[0] as f64, y0: rect[1] as f64, scale };
    let scene = Scene::new(input, &view, img.width, img.height);
    let Some(at) = scene.faces.iter().position(|f| f.index == ctx.index) else { return Strands::default() };
    let mut s = search(&img, &scene, at, &view);
    s.face_px = face_px(ctx.size);
    s.crop_ms = crop_ms;
    s.ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX).saturating_sub(crop_ms);
    s
}

/// A `w × h` crop reduced so its longer side is at most `max` pixels: the new size and the factor (1: as it is).
fn reduced(w: usize, h: usize, max: usize) -> (usize, usize, f64) {
    let scale = (w.max(h) as f64 / max.max(1) as f64).max(1.0);
    (((w as f64 / scale).round() as usize).max(1), ((h as f64 / scale).round() as usize).max(1), scale)
}

/// Why face `ctx` of a scene over a `photo` is not searched, as an empty search (`None`: it is).
fn skipped(ctx: &crate::FaceCtx<'_>, photo: (usize, usize)) -> Option<Strands> {
    let face_px = face_px(ctx.size);
    if photo.0 == 0 || photo.1 == 0 {
        return Some(Strands { face_px, ..Strands::default() });
    }
    if ctx.size.is_nan() || ctx.size < MIN_FACE_PX {
        return Some(Strands { searched: Searched::TooSmall, face_px, ..Strands::default() });
    }
    let has = |p: Part| ctx.face.mask(p).is_some();
    (!has(Part::Hair) && !has(Part::FacialSkin)).then_some(Strands { searched: Searched::NoParts, face_px, ..Strands::default() })
}

/// A face's size as a whole number of pixels (0 for one that is not a size).
fn face_px(size: f64) -> u32 {
    if size.is_finite() { size.clamp(0.0, f64::from(u32::MAX)) as u32 } else { 0 }
}

/// The strands of face `face` of `scene` (on `img`, the whole searched picture, placed in the photo by `crop`).
fn search(img: &Rgb32f, scene: &Scene<'_>, face: usize, crop: &Crop) -> Strands {
    let start = std::time::Instant::now();
    let Some(ctx) = scene.faces.get(face) else { return Strands::default() };
    if img.width == 0 || img.height == 0 {
        return Strands::default();
    }
    if let Some(skipped) = skipped(ctx, (img.width, img.height)) {
        return skipped;
    }
    let Some(grid) = grid::Grid::for_face(scene, face) else {
        return Strands { searched: Searched::NoParts, face_px: face_px(ctx.size), ..Strands::default() };
    };
    let coarse = grid.coarser(grid.cells(region::COARSE).round());
    let Some(area) = region::weights(scene, face, &coarse) else {
        return Strands { searched: Searched::NoParts, face_px: face_px(ctx.size), ..Strands::default() };
    };
    let up = |v: &[f32]| grid.resample_from(&coarse, v);
    let (weight, allowed, skin, core) = (up(&area.weight), up(&area.allowed), up(&area.skin), up(&area.core));
    let found = detect::strands(img, &grid, &detect::Near { weight: &weight, allowed: &allowed, skin: &skin, core: &core });
    let mut s = pack(&grid, &found, crop);
    s.face_px = face_px(ctx.size);
    s.ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
    s
}

/// The cells of `found` (on `grid`, over the picture `crop` places in the photo) as [`Strands`]: only the tiles
/// holding a strand.
fn pack(grid: &grid::Grid, found: &detect::Found, crop: &Crop) -> Strands {
    use crate::View;
    const TILE: usize = Strands::TILE;
    let (w, h) = (grid.w, grid.h);
    let x0 = grid.ox + grid.i0 as f64 * grid.cell;
    let y0 = grid.oy + grid.j0 as f64 * grid.cell;
    let (nx0, ny0) = crop.to_norm(x0, y0);
    let (nx1, ny1) = crop.to_norm(x0 + w as f64 * grid.cell, y0 + h as f64 * grid.cell);
    let area = NormRect::new(nx0, ny0, nx1, ny1);
    let count = count_strands(found, w, h);
    let cell = |x: usize, y: usize| -> [u8; 3] {
        let k = y * w + x;
        let q = (found.weight.get(k).copied().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
        let pol = found.polarity.get(k).copied().unwrap_or(0.0);
        if q < repair::MIN_STORED || pol == 0.0 {
            return [0, 0, 0];
        }
        let (nx, ny) = found.across.get(k).copied().unwrap_or((1.0, 0.0));
        let a = f64::from(ny).atan2(f64::from(nx)).rem_euclid(std::f64::consts::PI);
        let a = (a / std::f64::consts::PI * 255.0).round().clamp(0.0, 255.0) as u8;
        [q, a, if pol < 0.0 { 1 } else { 2 }]
    };
    let (tw, th) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let mut tiles = vec![u32::MAX; tw * th];
    let mut cells = Vec::new();
    for ty in 0..th {
        for tx in 0..tw {
            let block: Vec<[u8; 3]> = (0..TILE * TILE)
                .map(|i| (tx * TILE + i % TILE, ty * TILE + i / TILE))
                .map(|(x, y)| if x < w && y < h { cell(x, y) } else { [0; 3] })
                .collect();
            if block.iter().any(|c| c[0] > 0) {
                if let Some(t) = tiles.get_mut(ty * tw + tx) {
                    *t = u32::try_from(cells.len() / (TILE * TILE)).unwrap_or(u32::MAX);
                }
                cells.extend(block);
            }
        }
    }
    if cells.is_empty() {
        return Strands { count, ..Strands::default() };
    }
    Strands {
        area,
        width: w,
        height: h,
        picture: crop.picture(),
        side: (detect::SIDE * found.sigma + 1.0) * grid.cell,
        step: repair::ALONG_STEP * found.sigma * grid.cell,
        tiles,
        cells,
        count,
        ..Strands::default()
    }
}

/// The fewest cells a run of strand cells holds to count as one strand (a grown strand is five cells across).
const MIN_STRAND_CELLS: usize = 20;

/// How many strands `found` holds: the runs (8-connected) of cells above half a weight that hold at least
/// [`MIN_STRAND_CELLS`] cells.
fn count_strands(found: &detect::Found, w: usize, h: usize) -> u32 {
    let on = |k: usize| found.weight.get(k).is_some_and(|v| *v > 0.5) && found.polarity.get(k).is_some_and(|p| *p != 0.0);
    let mut seen = vec![false; w.saturating_mul(h)];
    let mut stack = Vec::new();
    let mut count = 0u32;
    for start in 0..seen.len() {
        if seen.get(start).copied().unwrap_or(true) || !on(start) {
            continue;
        }
        let mut size = 0usize;
        stack.push(start);
        if let Some(v) = seen.get_mut(start) {
            *v = true;
        }
        while let Some(k) = stack.pop() {
            size += 1;
            let (x, y) = ((k % w) as isize, (k / w) as isize);
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if on(n)
                        && let Some(v) = seen.get_mut(n)
                        && !*v
                    {
                        *v = true;
                        stack.push(n);
                    }
                }
            }
        }
        if size >= MIN_STRAND_CELLS {
            count = count.saturating_add(1);
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_is_reduced_to_the_cap_and_never_enlarged() {
        assert_eq!(reduced(3000, 2000, 3072), (3000, 2000, 1.0));
        let (w, h, k) = reduced(6144, 4096, MAX_CROP);
        assert_eq!((w, h), (3072, 2048));
        assert!((k - 2.0).abs() < 1e-9);
        assert_eq!(reduced(1, 5000, 3072).0, 1, "a sliver keeps a pixel");
        assert_eq!(reduced(0, 0, 0).2, 1.0);
    }

    /// The crop's view maps its pixels into the photo's frame and back, and says the photo's size at its reduction.
    #[test]
    fn a_crops_pixels_sit_in_the_photos_frame() {
        use crate::View;
        let c = Crop { photo: (4000, 2000), x0: 1000.0, y0: 500.0, scale: 2.0 };
        assert_eq!(c.to_norm(0.0, 0.0), (0.25, 0.25));
        assert_eq!(c.to_norm(500.0, 250.0), (0.5, 0.5));
        assert_eq!(c.to_px(0.5, 0.5), Some((500.0, 250.0)));
        assert_eq!(c.picture(), (2000, 1000));
    }
}
