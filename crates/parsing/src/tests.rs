//! Shared test helpers, and the tests that need the models (they skip without them).

use std::path::PathBuf;

/// A fresh empty folder for one test, removed when the test ends, passed or failed (a converted Core ML model
/// left in each run's folder filled the temp folder by about 840 MB a run).
pub(crate) struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for Scratch {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh empty folder for one test (see [`Scratch`]).
pub(crate) fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("lc-parsing-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

#[cfg(not(target_arch = "wasm32"))]
mod coreml;
#[cfg(not(target_arch = "wasm32"))]
mod models;
