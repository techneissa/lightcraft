//! Shared test helpers, and the tests that need the models (they skip without them).

use std::path::PathBuf;

/// A fresh empty folder for one test.
pub(crate) fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-parsing-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(not(target_arch = "wasm32"))]
mod models;
