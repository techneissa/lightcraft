//! A mask's evaluated alpha held for the pixels it touches, and the cache that keeps it from one
//! render to the next.
//!
//! The alpha does not include the mask's Amount: that scales its effect terms in the finish stage
//! ([`super::Evaluated::amount`]), so no effect slider, Amount included, changes it. What the alpha
//! depends on is in [`alpha_key`].

use std::cell::Cell;
use std::sync::Arc;

use lightcraft_develop::{Mask, MaskShape};
use lightcraft_raster::Plane;

use super::Mattes;
use super::brush::BrushCache;
use crate::for_rows;

thread_local! {
    static EVALUATIONS: Cell<usize> = const { Cell::new(0) };
}

/// How many masks have been evaluated (a mask's whole alpha built from its components) on this
/// thread: a test counts what a slider drag evaluates.
pub fn mask_evaluations() -> usize {
    EVALUATIONS.with(Cell::get)
}

/// Count one mask evaluation ([`mask_evaluations`]): for a renderer that evaluates masks itself.
pub fn note_evaluation() {
    EVALUATIONS.with(|c| c.set(c.get().wrapping_add(1)));
}

/// An alpha plane covering a `width × height` frame, stored for its non-zero bounds only: a small
/// mask holds a small plane. Outside the bounds the alpha is exactly 0.
#[derive(Clone, Debug, PartialEq)]
pub struct MaskAlpha {
    pub width: usize,
    pub height: usize,
    /// Left and top of the stored plane in the frame.
    x0: usize,
    y0: usize,
    /// The alpha inside the bounds (0 × 0 when the whole alpha is 0).
    plane: Plane,
}

impl MaskAlpha {
    /// An alpha that is 0 everywhere.
    pub fn zero(width: usize, height: usize) -> MaskAlpha {
        MaskAlpha { width, height, x0: 0, y0: 0, plane: Plane::new(0, 0) }
    }

    /// The alpha of a full-frame plane, cropped to its non-zero values (a NaN counts as non-zero:
    /// it is kept, not hidden).
    pub fn from_plane(p: Plane) -> MaskAlpha {
        let (w, h) = (p.width, p.height);
        let nonzero = |v: &f32| *v != 0.0;
        let (mut y0, mut y1, mut x0, mut x1) = (usize::MAX, 0, usize::MAX, 0);
        for y in 0..h {
            let row = p.row(y);
            if let Some(first) = row.iter().position(nonzero) {
                let last = row.iter().rposition(nonzero).unwrap_or(first);
                y0 = y0.min(y);
                y1 = y + 1;
                x0 = x0.min(first);
                x1 = x1.max(last + 1);
            }
        }
        if y1 == 0 {
            return MaskAlpha::zero(w, h);
        }
        if (x0, y0, x1, y1) == (0, 0, w, h) {
            return MaskAlpha { width: w, height: h, x0: 0, y0: 0, plane: p };
        }
        let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            data.extend_from_slice(&p.row(y)[x0..x1]);
        }
        MaskAlpha { width: w, height: h, x0, y0, plane: Plane { width: x1 - x0, height: y1 - y0, data } }
    }

    /// `[left, top, right, bottom]` of the non-zero pixels (right and bottom exclusive); empty
    /// (right ≤ left) for an alpha that is 0 everywhere.
    pub fn bounds(&self) -> [usize; 4] {
        if self.plane.data.is_empty() { [0, 0, 0, 0] } else { [self.x0, self.y0, self.x0 + self.plane.width, self.y0 + self.plane.height] }
    }

    /// Whether the alpha is 0 everywhere.
    pub fn is_zero(&self) -> bool {
        self.plane.data.is_empty()
    }

    /// The alpha at pixel `(x, y)` (0 outside the bounds and the frame).
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        let (Some(rx), Some(ry)) = (x.checked_sub(self.x0), y.checked_sub(self.y0)) else { return 0.0 };
        if rx >= self.plane.width || ry >= self.plane.height {
            return 0.0;
        }
        self.plane.data.get(ry * self.plane.width + rx).copied().unwrap_or(0.0)
    }

    /// [`Self::at`].
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.at(x, y)
    }

    /// Row `y`'s stored values and the frame column the first one is at (`None`: the row is 0).
    #[inline]
    pub fn row(&self, y: usize) -> Option<(usize, &[f32])> {
        let ry = y.checked_sub(self.y0).filter(|ry| *ry < self.plane.height)?;
        let w = self.plane.width;
        self.plane.data.get(ry * w..(ry + 1) * w).map(|r| (self.x0, r))
    }

    /// The whole frame as a plane.
    pub fn to_plane(&self) -> Plane {
        let mut out = Plane::new(self.width, self.height);
        if self.plane.data.is_empty() {
            return out;
        }
        for ry in 0..self.plane.height {
            let y = self.y0 + ry;
            if let (Some(dst), Some(src)) = (out.data.get_mut(y * self.width + self.x0..y * self.width + self.x0 + self.plane.width), self.row_at(ry))
            {
                dst.copy_from_slice(src);
            }
        }
        out
    }

    fn row_at(&self, ry: usize) -> Option<&[f32]> {
        let w = self.plane.width;
        self.plane.data.get(ry * w..(ry + 1) * w)
    }

    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.plane.data.len() * std::mem::size_of::<f32>()
    }
}

/// `f(a, b)` for each value `a` of `p` and the value `b` at the same place in `q` (same size), row-parallel.
pub(super) fn zip_rows(p: &mut Plane, q: &Plane, f: impl Fn(&mut f32, f32) + Sync + Send) {
    let w = p.width;
    for_rows(&mut p.data, w, |y, row| {
        let Some(src) = q.data.get(y * w..y * w + row.len()) else { return };
        for (a, b) in row.iter_mut().zip(src) {
            f(a, *b);
        }
    });
}

/// `f` on each value of `p`, row-parallel.
pub(super) fn map_rows(p: &mut Plane, f: impl Fn(&mut f32) + Sync + Send) {
    let w = p.width;
    for_rows(&mut p.data, w, |_, row| row.iter_mut().for_each(&f));
}

/// What a render's masks are evaluated against besides their own definitions: [`alpha_key`] keys
/// each alpha by the parts of it the mask reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlphaKeys {
    /// The output frame and size (`Plan::geo`): every mask depends on it.
    pub geo: u64,
    /// The image the masks see, before exposure (`Plan::lin_key`): range masks, the Sky / Subject /
    /// Background heuristics, Auto Mask brushes and Refine Edges read it.
    pub image: u64,
}

/// The alphas of the last render's visible masks, keyed by [`alpha_key`], and the brush parts
/// they were built from ([`BrushCache`]). An effect slider, Exposure for gradients and brushes,
/// Clarity, a mask's Amount: nothing a mask's alpha reads changes, so no mask is evaluated again.
/// Holds the masks of the last render only, tied to the segmentation mattes they were evaluated
/// with.
#[derive(Clone, Default)]
pub struct AlphaCache {
    mattes: Option<Arc<Mattes>>,
    slots: Vec<(u64, Arc<MaskAlpha>)>,
    pub(super) brushes: BrushCache,
}

impl AlphaCache {
    /// The alphas held for a render with `mattes` (none when the mattes changed), taken out of the
    /// cache: hand back the ones the render used with [`Self::keep`].
    pub(super) fn take(&mut self, mattes: Option<&Arc<Mattes>>) -> Vec<(u64, Arc<MaskAlpha>)> {
        let same = match (&self.mattes, mattes) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        let slots = std::mem::take(&mut self.slots);
        if same { slots } else { Vec::new() }
    }

    /// Hold `slots` (the alphas a render with `mattes` used), dropping everything else.
    pub(super) fn keep(&mut self, mattes: Option<&Arc<Mattes>>, slots: Vec<(u64, Arc<MaskAlpha>)>) {
        self.mattes = mattes.cloned();
        self.slots = slots;
    }

    /// Memory held (bytes): the alphas and the brush parts' planes. A plane shared with a render in
    /// progress is counted here as well.
    pub fn bytes(&self) -> usize {
        self.slots.iter().map(|(_, a)| a.bytes()).sum::<usize>() + self.brushes.bytes()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

/// Whether the alpha of mask `m` reads the image (`.0`) and the global exposure (`.1`), beyond its
/// own definition and the frame. Errs towards yes: a mask said to read something it doesn't is
/// only evaluated more often than it needs to be.
pub fn reads(m: &Mask) -> (bool, bool) {
    let (mut image, mut exposure) = (m.refine > 0.0, false);
    for c in &m.components {
        let (i, e) = match &c.shape {
            MaskShape::Linear { .. } | MaskShape::Radial { .. } | MaskShape::DepthRange { .. } | MaskShape::Landscape { .. } => (false, false),
            // a stored segmentation is guided-filtered by the photo's log luminance (`sample_seg`);
            // clicks without one take the heuristic; no clicks and no segmentation read nothing
            MaskShape::Prompt { seg, detail, .. } => (seg.is_some() || !detail.is_empty(), false),
            MaskShape::Object { hint, seg, detail, .. } => (!hint.is_empty() || seg.is_some() || !detail.is_empty(), false),
            MaskShape::Brush { strokes } => (strokes.iter().any(|s| s.auto_mask), false),
            // Sky: the heuristic reads the exposure (`shape_alpha`); a stored segmentation is filtered by the log
            // luminance before exposure and doesn't (a source matte doesn't either: counted as if it did)
            MaskShape::Sky { seg, detail, .. } => (true, seg.is_none() && detail.is_empty()),
            MaskShape::LuminanceRange { .. } | MaskShape::ColorRange { .. } => (true, true),
            MaskShape::Subject { .. } | MaskShape::Background { .. } | MaskShape::People { .. } => (true, false),
        };
        image |= i;
        exposure |= e;
    }
    (image, exposure)
}

/// The key of mask `m`'s alpha in a render with `keys` and global exposure `ev`: everything the
/// alpha is built from: its components, Refine Edges, Invert, the frame its coordinates are on
/// ([`Mask::on_photo`]), the render's frame, and the image and exposure for the masks that read
/// them ([`reads`]). Not its effects or Amount (they scale terms, not the alpha), not its
/// components' names, its name or id.
pub fn alpha_key(m: &Mask, keys: AlphaKeys, ev: f32) -> u64 {
    use std::hash::Hasher;
    let (image, exposure) = reads(m);
    let mut h = super::key::Words::new();
    h.write_u64(keys.geo);
    h.write_u64(if image { keys.image } else { 0 });
    h.write_u64(u64::from(image));
    h.write_u64(if exposure { u64::from(ev.to_bits()) | 1 << 32 } else { 0 });
    h.write_u64(m.refine.to_bits());
    h.write_u64(u64::from(m.invert) | u64::from(m.on_photo) << 1);
    h.write_u64(m.components.len() as u64);
    for c in &m.components {
        // (an op by its place in the enum: `MaskOp` has no `Hash`)
        h.write_u64(u64::from(c.invert) | (c.op as u64) << 1);
        super::key::shape(&mut h, &c.shape);
    }
    h.finish()
}
