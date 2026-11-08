//! The one-click download of Learn My Edits' image model (DINOv2-small's `model.safetensors`, 88 MB, from Hugging
//! Face, pinned to one revision), started only after the user has been asked (the window's dialog; an agent's
//! `learn.downloadModel` with `confirm`).
//!
//! The transfer is [`lightcraft_fetch`] (HTTPS that resumes, and checks the size and SHA-256 pinned in
//! [`lightcraft_learn`]), on a thread with a row in the activity stack whose ✕ stops it. The file goes into the
//! model folder shared with PhotoCraft.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_learn::{ENCODER_BYTES, ENCODER_FILE};

use super::{Download, LearnEdits, lock};
use crate::activity::{Cancel, Unit};

impl LearnEdits {
    /// Start the download, on a thread. Errors in words the user can act on.
    pub fn start_download(&self) -> Result<(), String> {
        if self.installed() {
            return Err("the image model is already installed".into());
        }
        let Some(dir) = self.model_dir() else { return Err("Learn My Edits has no model folder on this computer".into()) };
        {
            let mut state = lock(&self.inner.download);
            if matches!(*state, Download::Running { .. }) {
                return Err("the image model is already downloading".into());
            }
            *state = Download::Running { bytes: 0, total: ENCODER_BYTES };
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let row = self.inner.activity.start("download", "Downloading model", Cancel::Flag(cancel.clone()));
        row.set_unit(Unit::Bytes);
        row.detail("DINOv2-small");
        row.progress(0, ENCODER_BYTES);
        let this = self.clone();
        std::thread::Builder::new()
            .name("learn-download".into())
            .spawn(move || {
                let row = row;
                let state = crate::guard::catch("the download", || this.fetch(&dir, &cancel, &row)).unwrap_or_else(Download::Failed);
                *lock(&this.inner.download) = state;
            })
            .map(|_| ())
            .map_err(|e| {
                *lock(&self.inner.download) = Download::Idle;
                format!("could not start the download: {e}")
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fetch(&self, dir: &std::path::Path, cancel: &AtomicBool, row: &crate::activity::TaskGuard) -> Download {
        use lightcraft_fetch::{DownloadError, FileSpec, Options, download_url};
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Download::Failed(format!("{}: {e}", dir.display()));
        }
        let file = FileSpec { name: ENCODER_FILE, size: Some(ENCODER_BYTES), sha256: Some(lightcraft_learn::ENCODER_SHA256), max: ENCODER_BYTES };
        let result = download_url(&file, lightcraft_learn::ENCODER_URL, dir, &Options::default(), cancel, &mut |p| {
            let done = p.done.min(ENCODER_BYTES);
            row.progress(done, ENCODER_BYTES);
            *lock(&self.inner.download) = Download::Running { bytes: done, total: ENCODER_BYTES };
        });
        match result {
            Ok(()) => Download::Done,
            Err(DownloadError::Cancelled) => Download::Cancelled,
            Err(DownloadError::AllMirrorsFailed { errors, .. }) => {
                Download::Failed(format!("could not download the image model: {}", errors.join("; ").chars().take(300).collect::<String>()))
            }
            Err(e) => Download::Failed(e.to_string()),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn fetch(&self, _: &std::path::Path, _: &AtomicBool, _: &crate::activity::TaskGuard) -> Download {
        Download::Failed("the web build downloads no models".into())
    }
}
