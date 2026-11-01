//! macOS: the open and save panels open at least [`MIN_HEIGHT`] points tall and can be resized.
//!
//! rfd builds and runs the `NSSavePanel` / `NSOpenPanel` itself and gives no handle to it, and
//! AppKit restores the size the panel last had in this app (`NSNavPanelExpandedSizeFor…Mode`),
//! which was barely taller than its file list. So before each dialog, [`fit_next_panel`] puts a
//! timer on the main run loop in its common modes, which the panel's modal loop runs too. (A
//! main-queue block would not do: rfd runs the panel inside one, and the main queue runs nothing
//! else until it returns.) The timer finds the panel as the app's modal window, makes it
//! resizable, raises its minimum height and grows it to that height, keeping its top edge, then
//! stops. AppKit keeps a size the user drags to for the next dialog. Nothing found within
//! [`MAX_TRIES`] ticks: the timer stops and nothing changes. Elsewhere it does nothing.
//!
//! The `unsafe` (creating the timer, reading the common-modes constant) is in `macos`; the API is
//! safe. This and lightcraft-sysmem are the only crates allowed `unsafe` (AGENTS.md).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// The panels' minimum height in points (it was about 360).
pub const MIN_HEIGHT: f64 = 520.0;
/// How often the timer looks for the panel, in seconds, and how many times before it stops (5 s).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const TICK: f64 = 0.05;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MAX_TRIES: u32 = 100;

/// Fit the next open or save panel. Call it right before rfd shows one, from any thread.
pub fn fit_next_panel() {
    #[cfg(target_os = "macos")]
    macos::fit_next_panel();
}

/// [`MIN_HEIGHT`], or the screen's usable height when that is smaller.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn min_height(screen_height: Option<f64>) -> f64 {
    match screen_height {
        Some(h) if h.is_finite() && h > 0.0 => MIN_HEIGHT.min(h),
        _ => MIN_HEIGHT,
    }
}

/// A window frame as AppKit gives it: origin at the bottom left, y pointing up.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `frame` grown to `min_height` with its top edge kept, moved up when that puts its bottom below
/// the bottom of the screen's usable area (`visible_bottom`). None when it is tall enough.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn grown_frame(frame: Frame, min_height: f64, visible_bottom: Option<f64>) -> Option<Frame> {
    if !frame.h.is_finite() || frame.h >= min_height {
        return None;
    }
    let top = frame.y + frame.h;
    let y = match visible_bottom {
        Some(b) if b.is_finite() => (top - min_height).max(b),
        _ => top - min_height,
    };
    Some(Frame { y, h: min_height, ..frame })
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::cell::Cell;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSSavePanel, NSWindowStyleMask};
    use objc2_core_foundation::{CFAbsoluteTimeGetCurrent, CFRunLoop, CFRunLoopTimer, kCFRunLoopCommonModes};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    use super::{Frame, MAX_TRIES, TICK, grown_frame, min_height};

    pub fn fit_next_panel() {
        let Some(main) = CFRunLoop::main() else { return };
        let tries = Cell::new(0u32);
        let block = RcBlock::new(move |timer: *mut CFRunLoopTimer| {
            let n = tries.get() + 1;
            tries.set(n);
            if fit_modal_panel() || n >= MAX_TRIES {
                // SAFETY: CoreFoundation calls the handler with the timer that fired, which it
                // keeps alive (it is still in the run loop) for the length of the call.
                if let Some(timer) = unsafe { timer.as_ref() } {
                    timer.invalidate();
                }
            }
        });
        let first = CFAbsoluteTimeGetCurrent() + TICK;
        // SAFETY: the default allocator (None) is allowed, and the block is a valid,
        // retained handler; CoreFoundation copies it. The timer and the block live until the
        // timer is invalidated, which removes it from the run loop. The handler only runs on
        // the main thread, the run loop it is added to, so its `Cell` is never shared.
        let Some(timer) = (unsafe { CFRunLoopTimer::with_handler(None, first, TICK, 0, 0, Some(&block)) }) else {
            return;
        };
        // SAFETY: `kCFRunLoopCommonModes` is an immutable constant exported by CoreFoundation.
        let common = unsafe { kCFRunLoopCommonModes };
        // CFRunLoop is thread-safe: a timer may be added to the main run loop from any thread.
        main.add_timer(Some(&timer), common);
    }

    /// Fit the app's modal window when it is an open or save panel. False when there is none yet.
    fn fit_modal_panel() -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return true };
        let Some(window) = NSApplication::sharedApplication(mtm).modalWindow() else { return false };
        let Some(panel) = window.downcast_ref::<NSSavePanel>() else { return false };
        panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::Resizable);
        let visible = panel.screen().map(|s| s.visibleFrame());
        let min_height = min_height(visible.map(|v| v.size.height));
        let min = panel.minSize();
        if min.height < min_height {
            panel.setMinSize(NSSize::new(min.width, min_height));
        }
        let f = panel.frame();
        let frame = Frame { x: f.origin.x, y: f.origin.y, w: f.size.width, h: f.size.height };
        if let Some(g) = grown_frame(frame, min_height, visible.map(|v| v.origin.y)) {
            panel.setFrame_display(NSRect::new(NSPoint::new(g.x, g.y), NSSize::new(g.w, g.h)), true);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, w: f64, h: f64) -> Frame {
        Frame { x, y, w, h }
    }

    #[test]
    fn a_short_panel_grows_down_from_its_top_edge() {
        let grown = grown_frame(frame(520.0, 400.0, 861.0, 363.0), MIN_HEIGHT, Some(0.0));
        assert_eq!(grown, Some(frame(520.0, 400.0 + 363.0 - MIN_HEIGHT, 861.0, MIN_HEIGHT)));
    }

    #[test]
    fn a_panel_near_the_bottom_moves_up_to_stay_on_screen() {
        let grown = grown_frame(frame(10.0, 30.0, 861.0, 363.0), MIN_HEIGHT, Some(25.0));
        assert_eq!(grown, Some(frame(10.0, 25.0, 861.0, MIN_HEIGHT)));
    }

    #[test]
    fn a_tall_enough_panel_keeps_its_size() {
        assert_eq!(grown_frame(frame(0.0, 0.0, 900.0, 700.0), MIN_HEIGHT, None), None);
        assert_eq!(grown_frame(frame(0.0, 0.0, 900.0, MIN_HEIGHT), MIN_HEIGHT, None), None);
        assert_eq!(grown_frame(frame(0.0, 0.0, 900.0, f64::NAN), MIN_HEIGHT, None), None);
    }

    #[test]
    fn the_minimum_never_exceeds_a_small_screen() {
        assert_eq!(min_height(Some(400.0)), 400.0);
        assert_eq!(min_height(Some(1050.0)), MIN_HEIGHT);
        assert_eq!(min_height(Some(f64::NAN)), MIN_HEIGHT);
        assert_eq!(min_height(None), MIN_HEIGHT);
    }

    #[test]
    fn fitting_without_a_panel_is_harmless() {
        // No run loop runs in a test: the timer never fires and nothing changes.
        fit_next_panel();
    }
}
