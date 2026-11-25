//! A slider's drafts take the image a full render kept (issue #372).
//!
//! A drag renders drafts at another size than the render at rest (the canvas size, or smaller when full-size drafts
//! are too slow). A draft at a size the view has no linear image for drew every spot again, at that size (the Classic
//! and Content-Aware fills differ with the scale, so the spots seemed to render again), and ran the retouching, white
//! balance and noise reduction again; and with a LaMa spot a draft's keys always differed from a full render's, so even
//! a draft at the same size (zoomed in) did all of that. Now a draft whose edit comes after the linear stage (tone,
//! colour, effects, masks) takes the full render's linear image: at its own size as it is, at a smaller one resampled
//! from it. A resampled image is kept for drafts alone (its keys are [`LinKeys::for_draft`]), and the stage cache drops
//! such images first, so the full size's survives a drag and the render at rest after it reuses it too.

use std::sync::Arc;

use lightcraft_pipeline::{LinKeys, Plan};
use lightcraft_raster::Rgb32f;

use super::{Cx, Entry, GpuStages, resize};
use crate::ctx::Buf;

thread_local! {
    static RESAMPLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LINEAR: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Drafts on this thread made from a full render's image at another size so far (a test counts them).
pub fn drafts_resampled() -> usize {
    RESAMPLED.with(std::cell::Cell::get)
}

/// Linear stages (spots, retouching, white balance, noise reduction) this thread's renders made rather than reused so
/// far (a test counts the work of one edit with it).
pub fn linear_stages_made() -> usize {
    LINEAR.with(std::cell::Cell::get)
}

/// Count one linear stage made ([`linear_stages_made`]).
pub(super) fn note_linear() {
    LINEAR.with(|c| c.set(c.get().wrapping_add(1)));
}

/// A full render's linear image of the whole frame a draft is made from, and its size.
pub(super) struct Larger {
    lin: Arc<Buf>,
    size: (usize, usize),
}

/// For a draft: the keys of the linear image it takes, set on `plan` (a full render's at this size, or one made from
/// a full render's at a larger size for drafts), and the larger image to make it from (`None`: it has its own, or
/// takes the one at this size, or there is none to take). `cached`: the view's stages at this size.
pub(super) fn adopt(stages: Option<&GpuStages>, src: &Arc<Rgb32f>, plan: &mut Plan<'_>, cached: Option<&Entry>) -> Option<Larger> {
    if !plan.draft {
        return None;
    }
    let own = cached.and_then(|e| e.lin.as_ref()).map(|(k, _)| *k);
    if own == Some(plan.lin_key) {
        return None;
    }
    // (a full render's image is the draft's, or better: its LaMa spots have their fill)
    if own == Some(plan.full_keys.lin) {
        plan.use_keys(plan.full_keys);
        return None;
    }
    let shape = plan.shape?;
    let drafted: LinKeys = plan.full_keys.for_draft();
    if own == Some(drafted.lin) {
        plan.use_keys(drafted);
        return None;
    }
    let (w, h) = (plan.w, plan.h);
    let from = stages?
        .lock()
        .iter()
        .filter(|e| Arc::ptr_eq(&e.src, src) && e.full && e.shape == Some(shape) && e.size.0 >= w && e.size.1 >= h && e.size != (w, h))
        .filter_map(|e| e.lin.as_ref().map(|(_, b)| Larger { lin: b.clone(), size: e.size }))
        .max_by_key(|f| f.size.0 * f.size.1)?;
    plan.use_keys(drafted);
    Some(from)
}

/// The draft's linear image: `from` resampled to the plan's size.
pub(super) fn resample(cx: &mut Cx<'_>, from: &Larger, plan: &Plan<'_>) -> Arc<Buf> {
    RESAMPLED.with(|c| c.set(c.get().wrapping_add(1)));
    // (bilinear taps widened by the ratio: no weight below zero, so no ringing around the spots and highlights)
    Arc::new(resize(cx, &from.lin, from.size, (plan.w, plan.h), 3, lightcraft_raster::resample::Filter::Bilinear))
}

/// Which entry the stage cache drops when it holds too many: the oldest made for drafts alone (no full render's
/// image), else the oldest. `entries` are oldest first; the last was just added and is never dropped for another.
pub(super) fn to_drop(entries: &[Entry]) -> usize {
    let older = entries.len().saturating_sub(1);
    entries.iter().take(older).position(|e| !e.full).unwrap_or(0)
}
