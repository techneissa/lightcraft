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

use crate::input::Strands;
use crate::{Control, Face, Feature, Input, NormRect, Part, Scene, Scope, Spec, Stage, WholePhoto};

/// The Stray Hair settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hair {
    /// 0…100; 0 is off.
    pub amount: f64,
}

/// Cells of the detection grid across the face's size (a strand is about one cell wide).
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
/// people parse's, in linear light, turned like the masks), which is the whole photo. The provider runs it once per
/// parse, on a worker, never in a render; each face gets its [`Face::strands`] (empty when none were found or the
/// face was not searched).
pub fn with_strands(img: &Rgb32f, faces: Vec<Face>) -> Vec<Face> {
    let view = WholePhoto { width: img.width, height: img.height };
    let input = Input::new(faces);
    let found: Vec<(usize, Strands)> = {
        let scene = Scene::new(&input, &view, img.width, img.height);
        scene.faces.iter().enumerate().map(|(i, f)| (f.index, search(img, &scene, i))).collect()
    };
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

/// The strands of face `face` of `scene` (on `img`, the whole searched picture).
fn search(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Strands {
    let Some(ctx) = scene.faces.get(face) else { return Strands::default() };
    if ctx.size.is_nan() || ctx.size < MIN_FACE_PX || img.width == 0 || img.height == 0 {
        return Strands::default();
    }
    let Some(grid) = grid::Grid::for_face(scene, face) else { return Strands::default() };
    let coarse = grid.coarser(grid.cells(region::COARSE).round());
    let Some(area) = region::weights(scene, face, &coarse) else { return Strands::default() };
    let up = |v: &[f32]| grid.resample_from(&coarse, v);
    let (weight, allowed, skin, core) = (up(&area.weight), up(&area.allowed), up(&area.skin), up(&area.core));
    let found = detect::strands(img, &grid, &detect::Near { weight: &weight, allowed: &allowed, skin: &skin, core: &core });
    pack(&grid, &found, img.width, img.height)
}

/// The cells of `found` (on `grid`, over a `width × height` picture) as [`Strands`]: only the tiles holding a strand.
fn pack(grid: &grid::Grid, found: &detect::Found, width: usize, height: usize) -> Strands {
    const TILE: usize = Strands::TILE;
    let (w, h) = (grid.w, grid.h);
    let (fw, fh) = (width as f64, height as f64);
    let x0 = grid.ox + grid.i0 as f64 * grid.cell;
    let y0 = grid.oy + grid.j0 as f64 * grid.cell;
    let area = NormRect::new(x0 / fw, y0 / fh, (x0 + w as f64 * grid.cell) / fw, (y0 + h as f64 * grid.cell) / fh);
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
        return Strands::default();
    }
    Strands {
        area,
        width: w,
        height: h,
        picture: (width, height),
        side: (detect::SIDE * found.sigma + 1.0) * grid.cell,
        step: repair::ALONG_STEP * found.sigma * grid.cell,
        tiles,
        cells,
    }
}
