//! Stage marks for measuring a parse (issue #200, `fork/parse-ahead.md`): each step of a parse calls [`mark`] with
//! its name when it ends. Nothing listens unless a tool sets a hook ([`on_mark`]); the profiling example takes the wall
//! and CPU time since the previous mark there, so the time between two marks belongs to the later one.

use std::sync::OnceLock;

static HOOK: OnceLock<fn(&'static str)> = OnceLock::new();

/// Call `f` at every mark from now on; `false` when a hook was set already.
pub fn on_mark(f: fn(&'static str)) -> bool {
    HOOK.set(f).is_ok()
}

/// The step `stage` has just ended.
pub fn mark(stage: &'static str) {
    if let Some(f) = HOOK.get() {
        f(stage);
    }
}
