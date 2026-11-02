//! The one-click download of the LaMa model (`big-lama.pt`, 206 MB, from Hugging Face), started only after the user has
//! been asked (the Remove panel's dialog; an agent's `lama.download` is the asking).
//!
//! The transfer is [`lightcraft_fetch`] (pure-Rust HTTPS that resumes, and checks the size and SHA-256 pinned below), on a
//! background thread with a row in the activity stack whose ✕ stops it (what arrived stays as a `.part` file; asking
//! again resumes it). The file goes straight into the model folder, which is shared with PhotoCraft.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError};

use super::{LamaFill, MODEL_FILE};
use crate::activity::{Cancel, Unit};

/// Where `big-lama.pt` is fetched from.
pub const DOWNLOAD_URL: &str = "https://huggingface.co/fashn-ai/LaMa/resolve/main/big-lama.pt";
/// Its exact size, bytes.
pub const DOWNLOAD_BYTES: u64 = 205_803_670;
const DOWNLOAD_SHA256: &str = "7ba7aa7ac37a4d41fdbbeba3a2af7ead18058552997e3a3cd1a3b2210c9e6b4c";

/// Where the download stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Download {
    Idle,
    Running { bytes: u64, total: u64 },
    /// Arrived and checked: the model is installed.
    Done,
    Cancelled,
    Failed(String),
}

impl LamaFill {
    /// Start the download into the model folder, on a thread. Errors (in words the user can act on): already installed,
    /// already downloading, no folder, the thread could not start.
    pub fn start_download(&self) -> Result<(), String> {
        if self.installed() {
            return Err("LaMa is already installed".into());
        }
        let Some(dir) = self.dir() else { return Err("LaMa has no model folder on this computer".into()) };
        {
            let mut state = self.inner.download.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*state, Download::Running { .. }) {
                return Err("LaMa is already downloading".into());
            }
            *state = Download::Running { bytes: 0, total: DOWNLOAD_BYTES };
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let row = self.inner.activity.start("download", "Downloading model", Cancel::Flag(cancel.clone()));
        row.set_unit(Unit::Bytes);
        row.detail("LaMa");
        row.progress(0, DOWNLOAD_BYTES);
        let this = self.clone();
        std::thread::Builder::new()
            .name("lama-download".into())
            .spawn(move || {
                // the row goes when the thread ends, however it ends
                let row = row;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.fetch(&dir, &cancel, &row)));
                let state = outcome.unwrap_or_else(|_| Download::Failed("the download stopped unexpectedly".into()));
                if state == Download::Done {
                    this.clear_message();
                }
                *this.inner.download.lock().unwrap_or_else(PoisonError::into_inner) = state;
            })
            .map(|_| ())
            .map_err(|e| {
                *self.inner.download.lock().unwrap_or_else(PoisonError::into_inner) = Download::Idle;
                format!("could not start the download: {e}")
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fetch(&self, dir: &std::path::Path, cancel: &AtomicBool, row: &crate::activity::TaskGuard) -> Download {
        use lightcraft_fetch::{DownloadError, FileSpec, Options, download_url};
        let file = FileSpec { name: MODEL_FILE, size: Some(DOWNLOAD_BYTES), sha256: Some(DOWNLOAD_SHA256), max: DOWNLOAD_BYTES };
        let result = download_url(&file, DOWNLOAD_URL, dir, &Options::default(), cancel, &mut |p| {
            let done = p.done.min(DOWNLOAD_BYTES);
            row.progress(done, DOWNLOAD_BYTES);
            *self.inner.download.lock().unwrap_or_else(PoisonError::into_inner) = Download::Running { bytes: done, total: DOWNLOAD_BYTES };
        });
        match result {
            Ok(()) => Download::Done,
            Err(DownloadError::Cancelled) => Download::Cancelled,
            Err(DownloadError::AllMirrorsFailed { errors, .. }) => Download::Failed(format!("could not download LaMa: {}", errors.join("; ").chars().take(300).collect::<String>())),
            Err(e) => Download::Failed(e.to_string()),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn fetch(&self, _: &std::path::Path, _: &AtomicBool, _: &crate::activity::TaskGuard) -> Download {
        Download::Failed("the web build downloads no models".into())
    }
}
