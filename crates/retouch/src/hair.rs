//! Stray Hair (fork feature, `fork/retouch-hair.md`): flyaways at the hair's outer edge and stray strands across
//! the face, cleaned per face with one Amount slider (0…100, off at 0).
//!
//! How, per face:
//! 1. **Where** ([`region`]): on a coarse grid anchored to the face, from the part masks. The main hair mass is the
//!    Hair mask opened at a few hundredths of the face (thin strands drop out of it, the mass stays); strands are
//!    looked for in a band just outside it (the Hair matte's soft fringe and past it, so the chosen hair edge decides
//!    where the hair ends) and over Facial Skin (closed, so the strands it was cut around are inside), never in the
//!    mass, on eyebrows, eyes and lashes, lips and teeth, glasses and jewellery (Others), or clothes (the person's
//!    pixels that are neither hair nor skin). The one place an area's extent is decided: the Refine brush (#99)
//!    adds or takes away its strokes in [`region::weights`].
//! 2. **Detect** ([`detect`]): on a fine grid of cells a fixed share of the face ([`CELLS_PER_FACE`]), an oriented
//!    ridge filter (the Hessian of the luminance's cube root, blurred at a strand's width) finds thin lines; a line is
//!    kept when it stands out from both sides alike (a step, such as the face's or the hair's outline, does not), is
//!    long along its own direction, and is not part of a dense texture (a beard, curls, foliage, a knit).
//! 3. **Repair** ([`repair`]): each strand pixel is filled from the background or skin beside it: samples on both
//!    sides across the strand, averaged along it, the side least like the strand weighted most; only pixels darker
//!    (or, for a bright strand, lighter) than the fill change, so the texture beside a strand is kept.
//!
//! Detection and the fills are kept ([`cache`]) by a hash of the pixels the face's search reads, its masks and its
//! place, so moving the slider only blends the kept fills (no search runs while the pixels before the step stay the
//! same). Sizes are fractions of the face, so a draft, the loupe and an export find the same strands, and everything
//! is anchored to the face's box, so a zoomed window that holds what it reads ([`Hair::REACH`]) finds them too.

mod cache;
mod detect;
mod grid;
mod region;
mod repair;

use lightcraft_raster::Rgb32f;
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

/// The Stray Hair settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hair {
    /// 0…100; 0 is off.
    pub amount: f64,
}

/// Cells of the detection grid across the face's size (a strand is about one cell wide).
pub const CELLS_PER_FACE: f64 = 400.0;

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
    /// The search covers the face's box and the Hair and Facial Skin masks' areas grown by [`region::BAND`], and
    /// reads [`region::PAD`] past that (the fills' samples beside a strand, the blurs).
    const REACH: f64 = region::BAND + region::PAD;
    const CONTROLS: &'static [Control<Self>] =
        &[Control { spec: Spec::amount("retouch.hair.amount", "Amount"), get: |s| s.amount, set: |s, v| s.amount = v }];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        let amount = unit(self.amount);
        if amount <= 0.0 || img.width == 0 || img.height == 0 || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(grid) = grid::Grid::for_face(scene, face) else { return };
        let key = cache::key(img, scene, face, &grid);
        let fills = cache::get_or(key, || find(img, scene, face, &grid));
        repair::blend(img, &fills, amount);
    }
}

/// The fills of face `face`'s stray strands: where to look, the strands, then what goes in their place.
fn find(img: &Rgb32f, scene: &Scene<'_>, face: usize, grid: &grid::Grid) -> Vec<repair::Fill> {
    let coarse = grid.coarser(grid.cells(region::COARSE).round());
    let Some(area) = region::weights(scene, face, &coarse) else { return Vec::new() };
    let (weight, skin, core) =
        (grid.resample_from(&coarse, &area.weight), grid.resample_from(&coarse, &area.skin), grid.resample_from(&coarse, &area.core));
    let cells = detect::strands(img, grid, &detect::Near { weight: &weight, skin: &skin, core: &core });
    repair::fills(img, grid, &cells)
}
