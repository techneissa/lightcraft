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

static HOLDER: Mutex<Option<&'static str>> = Mutex::new(None);

/// The GPU turn of a run that says who has it ([`holder`]); the name is cleared before the GPU is let go.
pub(crate) struct NamedTurn {
    _gpu: MutexGuard<'static, ()>,
}

/// [`turn`], noting `name` as the holder until the guard drops, so a job still waiting can say what it waits for.
pub(crate) fn turn_for(name: &'static str) -> NamedTurn {
    let gpu = turn();
    *HOLDER.lock().unwrap_or_else(PoisonError::into_inner) = Some(name);
    NamedTurn { _gpu: gpu }
}

impl Drop for NamedTurn {
    fn drop(&mut self) {
        *HOLDER.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

/// Who holds the GPU now, when the holder named itself ([`turn_for`]).
pub(crate) fn holder() -> Option<&'static str> {
    *HOLDER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a job waiting for the GPU shows while another named run holds it ("Waiting for Glare Repair").
pub(crate) fn waiting_text() -> Option<String> {
    holder().map(|h| format!("Waiting for {h}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_turn_is_the_holder_until_it_drops() {
        {
            let _t = turn_for("Glare Repair");
            assert_eq!(holder(), Some("Glare Repair"));
            assert_eq!(waiting_text().as_deref(), Some("Waiting for Glare Repair"));
        }
        assert_eq!(holder(), None);
        assert_eq!(waiting_text(), None);
    }
}
