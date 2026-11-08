//! Fork feature (`fork/hair-edges.md`): one AI model run at a time on the GPU across people parsing, hair edges
//! (BiRefNet Matting) and the LaMa fill. Each holds its own model and Metal device; two graphs in flight at once
//! fight for the GPU's memory and its command queue, and on a busy Mac one of them can fail or stall.
//!
//! Lock order (so nothing deadlocks): a feature's own run lock (`one_run`, LaMa's `run`) first, then [`turn`], then
//! the feature's loaded-model slot. Nothing takes a feature's run lock, or calls into another feature, while it holds
//! [`turn`]. People parsing releases its turn before it asks for a matte.

use std::sync::{Mutex, MutexGuard, PoisonError};

static GPU_RUN: Mutex<()> = Mutex::new(());

/// Wait for the GPU, and hold it until the guard drops.
pub(crate) fn turn() -> MutexGuard<'static, ()> {
    GPU_RUN.lock().unwrap_or_else(PoisonError::into_inner)
}
