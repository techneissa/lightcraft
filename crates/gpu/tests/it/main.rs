//! The integration tests, one module per file, linked into one test binary.
//! Binaries of their own: `tests/shutdown.rs` and `tests/shutdown_first.rs` close the GPU for the whole
//! process, `tests/memory.rs` checks the process's device-memory counters, which renders on other test
//! threads would change, and `tests/display.rs` and `tests/window_pending.rs` adopt (or wait for) the
//! window's device, once per process.

mod decontam;
mod dirty_render;
mod equivalence;
mod fallback;
mod fit;
#[cfg(not(target_arch = "wasm32"))]
mod gpu_probe;
mod hidden_spot;
mod mask_cache;
// fork: `fork/perf-portrait-draft.md`
mod portrait_drag;
mod shaped_spot;
mod spot_cache;
mod spot_reuse;
mod thumbs_bench;
