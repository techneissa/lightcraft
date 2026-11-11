//! Mask edge refinement (fork, issue #240): an AI selection drawn on a coarse grid (a model's 1024² output, a people
//! parse's plane) snapped to the photo at a much finer resolution, once, so the stored mask is right at 1:1.
//!
//! A model sees the photo at about 1000 px; on a 24 MP photo each of its cells is 6 px wide, so a strand of hair, the
//! narrow end of a gap between fingers or a crisp sleeve edge falls inside one cell and comes out as a soft blur. Three
//! pure steps fix that, given the coarse coverage at the working size and the photo at that size:
//!
//! - [`snap`]: a colour guided filter (He, Sun and Tang, "Guided Image Filtering", 2010/2013, the feathering use) in a
//!   band around the coarse edge. Within each small window the coverage becomes a linear function of the pixel's
//!   colour fitted to the coarse coverage, so the edge lands on the photo's own colour edge and thin strands keep
//!   their shape. Away from the edge nothing changes.
//! - [`decide`]: the coarse selection decides what the subject is, a matte (an edge model's finer selection) where
//!   its edge runs: in a band around the coarse edge, and in the gaps [`gaps`] finds.
//! - [`gaps`]: background regions an edge model (a matte) is sure of inside a coarse selection (the gap between an arm
//!   and the body, between fingers), each small next to the selection, so the matte may open them even far from the
//!   coarse edge.
//! - [`store`]: the refined coverage as the grids a mask keeps: one over the whole photo and up to four finer ones
//!   tiling the edge, each at most 1024² (the format's cap), so a render at any size samples the same edge.
//!
//! Never panics: sizes that do not add up return nothing or leave the coverage as it was; every allocation is bounded
//! by the working image the caller passes.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod decide;
mod gaps;
mod guided;
mod store;

pub use decide::{decide, distance_to_edge, upsample};
pub use gaps::gaps;
pub use guided::snap;
pub use store::{Grid, store, tile};

/// Logits are kept within this (a stored grid's quantization holds ±15.9: fully in or out).
pub const LOGIT: f32 = 15.9;
/// The finest side a stored grid may have (`lightcraft_develop::segmask::MAX_SIDE`).
pub const MAX_SIDE: usize = 1024;
/// Most finer grids one selection keeps (`lightcraft_develop::segmask::MAX_DETAIL`).
pub const MAX_TILES: usize = 4;

/// An 8-bit RGB picture, row-major (`w × h × 3` bytes): the photo at the working size.
#[derive(Clone, Copy)]
pub struct Rgb<'a> {
    pub data: &'a [u8],
    pub w: usize,
    pub h: usize,
}

impl Rgb<'_> {
    /// Whether the sizes add up.
    pub fn is_valid(&self) -> bool {
        self.w > 0 && self.h > 0 && self.w.checked_mul(self.h).and_then(|n| n.checked_mul(3)) == Some(self.data.len())
    }
}

/// Coverage (0 to 1) at each pixel of the working size, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Alpha {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Alpha {
    /// Whether the sizes add up.
    pub fn is_valid(&self) -> bool {
        self.w > 0 && self.h > 0 && self.w.checked_mul(self.h) == Some(self.data.len())
    }

    /// `w × h` pixels, each the coverage `f` gives at its centre (`x`, `y` as fractions of the size).
    pub fn from_fn(w: usize, h: usize, f: impl Fn(f32, f32) -> f32 + Sync) -> Alpha {
        use rayon::prelude::*;
        let mut data = vec![0.0f32; w.saturating_mul(h)];
        if w > 0 {
            data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let fy = (y as f32 + 0.5) / h as f32;
                for (x, v) in row.iter_mut().enumerate() {
                    let c = f((x as f32 + 0.5) / w as f32, fy);
                    *v = if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.0 };
                }
            });
        }
        Alpha { w, h, data }
    }
}

/// The guided filter's window radius, in working pixels, for an input whose cells are `cell_px` pixels wide: one and a
/// half cells, the band a bilinear enlargement leaves uncertain (at least 2, at most 64).
pub fn radius_for(cell_px: f32) -> usize {
    if !cell_px.is_finite() {
        return 2;
    }
    (1.5 * cell_px).round().clamp(2.0, 64.0) as usize
}

/// The logit of a coverage, within ±[`LOGIT`].
pub fn logit(a: f32) -> f32 {
    if !a.is_finite() {
        return -LOGIT;
    }
    let a = a.clamp(1e-7, 1.0 - 1e-7);
    (a / (1.0 - a)).ln().clamp(-LOGIT, LOGIT)
}

/// The coverage of a logit.
pub fn sigmoid(l: f32) -> f32 {
    1.0 / (1.0 + (-l).exp())
}

/// The work was stopped (the caller's `stop` said so).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

#[cfg(test)]
mod tests;
