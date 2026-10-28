//! How many renders run at once: Core Image's own buffers are large (826 MB resident for one
//! full-size A7 V render), so at most one render above preview size runs at a time and at most two
//! at preview size or below, whatever the caller's memory gate lets through (export batches start
//! several photos side by side).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// Renders with a long edge above this count as full size.
pub(crate) const PREVIEW_EDGE: u32 = 2560;
pub(crate) const FULL_SLOTS: u32 = 1;
pub(crate) const SMALL_SLOTS: u32 = 2;

/// Peak footprint a render may reach whatever its output size: measured 471–773 MB at 512 px (draft),
/// 374–759 MB at 2560 px and 578–687 MB at full size on A7 V files (macOS 27.0.1). Core Image's own
/// buffers dominate, so a small render costs about as much as a large one.
pub const FLOOR_BYTES: u64 = 800_000_000;

/// The memory gate weight of one render with this output size, in bytes: macOS's peak footprint (what
/// it counts against the app), not resident memory. A flat [`FLOOR_BYTES`] up to 2560 px and at the
/// A7 V's full 7008 × 4672 (200 MB + 16 B a pixel = 724 MB, under the floor); larger sensors grow
/// with their pixels (the float RGBA bands and the float result).
pub fn weight_bytes(width: u32, height: u32) -> u64 {
    FLOOR_BYTES.max(200_000_000 + 16 * u64::from(width) * u64::from(height))
}

pub(crate) struct Slots {
    /// Running renders: (full size, preview size or below).
    busy: Mutex<(u32, u32)>,
    freed: Condvar,
}

pub(crate) struct Slot<'a> {
    slots: &'a Slots,
    full: bool,
}

impl Slots {
    pub(crate) const fn new() -> Self {
        Slots { busy: Mutex::new((0, 0)), freed: Condvar::new() }
    }

    /// Wait for a slot; `None` when `cancel` is set while waiting.
    pub(crate) fn acquire(&self, full: bool, cancel: &AtomicBool) -> Option<Slot<'_>> {
        let mut busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let (count, cap) = if full { (&mut busy.0, FULL_SLOTS) } else { (&mut busy.1, SMALL_SLOTS) };
            if *count < cap {
                *count += 1;
                return Some(Slot { slots: self, full });
            }
            busy = self.freed.wait_timeout(busy, Duration::from_millis(50)).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut busy = self.slots.busy.lock().unwrap_or_else(PoisonError::into_inner);
        let count = if self.full { &mut busy.0 } else { &mut busy.1 };
        *count = count.saturating_sub(1);
        drop(busy);
        self.slots.freed.notify_all();
    }
}

/// True when a render of this long edge (`None` = full size) takes a full-size slot.
pub(crate) fn is_full(long_edge: Option<u32>) -> bool {
    long_edge.is_none_or(|e| e > PREVIEW_EDGE)
}

#[cfg(test)]
mod tests {
    use super::{FULL_SLOTS, SMALL_SLOTS, Slots, is_full, weight_bytes};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;

    /// Most renders running at once when 6 threads ask for `full` slots.
    fn most_at_once(full: bool) -> u32 {
        let slots = Slots::new();
        let (running, most) = (AtomicU32::new(0), AtomicU32::new(0));
        let cancel = AtomicBool::new(false);
        std::thread::scope(|s| {
            for _ in 0..6 {
                s.spawn(|| {
                    let Some(_slot) = slots.acquire(full, &cancel) else { return };
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    running.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        most.load(Ordering::SeqCst)
    }

    /// T16d: two full-size renders never run together; small ones two at a time.
    #[test]
    fn full_size_renders_run_one_at_a_time() {
        assert_eq!(most_at_once(true), FULL_SLOTS);
        assert!(most_at_once(false) <= SMALL_SLOTS);
    }

    #[test]
    fn a_cancelled_wait_gives_up() {
        let slots = Slots::new();
        let cancel = AtomicBool::new(false);
        let held = slots.acquire(true, &cancel);
        assert!(held.is_some());
        cancel.store(true, Ordering::Relaxed);
        assert!(slots.acquire(true, &cancel).is_none());
        drop(held);
        assert!(slots.acquire(true, &AtomicBool::new(false)).is_some(), "the slot came back");
    }

    /// T16d: the weight follows the measured peak footprint: 800 MB up to full A7 V size, more above.
    #[test]
    fn weight_formula() {
        assert_eq!(weight_bytes(0, 0), 800_000_000);
        assert_eq!(weight_bytes(512, 341), 800_000_000);
        assert_eq!(weight_bytes(2560, 1707), 800_000_000);
        assert_eq!(weight_bytes(7008, 4672), 800_000_000, "the A7 V's measured 687 MB fits under the floor");
        assert_eq!(weight_bytes(9504, 6336), 200_000_000 + 16 * 9504 * 6336);
        assert!(is_full(None) && is_full(Some(2561)) && !is_full(Some(2560)) && !is_full(Some(512)));
    }
}
