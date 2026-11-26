//! The integration tests, one module per file, linked into one test binary.

mod coreml;
mod reference;
mod remote;
mod stored;

/// SAM 3 tests load the whole model (several GB each, and Core ML compiles into the shared folder): one at a time, or a
/// parallel run can take the test process down.
pub(crate) static SAM3: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Hold [`SAM3`] for the rest of a test (a failed test leaves it usable).
pub(crate) fn one_sam3_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    SAM3.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
