//! Downloading a model set's files (asked first: the Settings button's dialog, or an agent's `parsing.download` with
//! `confirm`), one after another into their folders under the models folder, with one row in the activity stack whose
//! ✕ stops it. [`lightcraft_fetch`] resumes what arrived (a `.part` file) and checks each file's size and SHA-256.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use lightcraft_parsing::ModelSet;

use super::{PeopleParsing, lock};
use crate::activity::{Cancel, Unit};

/// Where a set's download stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Download {
    Idle,
    Running {
        bytes: u64,
        total: u64,
    },
    /// Every file arrived and was checked.
    Done,
    Cancelled,
    Failed(String),
}

impl PeopleParsing {
    pub fn download_state(&self, set: ModelSet) -> Download {
        lock(&self.inner.downloads).get(&set).cloned().unwrap_or(Download::Idle)
    }

    fn set_download(&self, set: ModelSet, d: Download) {
        lock(&self.inner.downloads).insert(set, d);
    }

    /// Start downloading the files of `set` that are missing, on a thread. Errors (in words the user can act on):
    /// already installed, already downloading, no models folder, the thread could not start.
    pub fn start_download(&self, set: ModelSet) -> Result<u64, String> {
        let Some(models) = self.models_dir() else { return Err("this build has nowhere to keep people parsing models".into()) };
        let missing: Vec<_> = set.files().into_iter().filter(|f| !f.installed_in(&models)).collect();
        if missing.is_empty() {
            return Err(format!("the {} models are already downloaded", set.label()));
        }
        let total: u64 = missing.iter().map(|f| f.size).sum();
        {
            let mut d = lock(&self.inner.downloads);
            if matches!(d.get(&set), Some(Download::Running { .. })) {
                return Err(format!("the {} models are already downloading", set.label()));
            }
            d.insert(set, Download::Running { bytes: 0, total });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.inner.download_flags).push(cancel.clone());
        let row = self.inner.activity.start("download", "Downloading model", Cancel::Flag(cancel.clone()));
        row.set_unit(Unit::Bytes);
        row.detail(&format!("People parts: {}", set.label()));
        row.progress(0, total);
        let this = self.clone();
        std::thread::Builder::new()
            .name("parsing-download".into())
            .spawn(move || {
                let row = row;
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.fetch(set, &models, &missing, total, &cancel, &row)));
                this.set_download(set, outcome.unwrap_or_else(|_| Download::Failed("the download stopped unexpectedly".into())));
            })
            .map(|_| total)
            .map_err(|e| {
                self.set_download(set, Download::Idle);
                format!("could not start the download: {e}")
            })
    }

    /// Stop every download going (what arrived stays; asking again resumes).
    pub fn cancel_downloads(&self) {
        for flag in lock(&self.inner.download_flags).drain(..) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fetch(
        &self,
        set: ModelSet,
        models: &std::path::Path,
        files: &[lightcraft_parsing::ModelFile],
        total: u64,
        cancel: &AtomicBool,
        row: &crate::activity::TaskGuard,
    ) -> Download {
        use lightcraft_fetch::{DownloadError, FileSpec, Options, download_url};
        let mut before = 0u64;
        for f in files {
            let spec = FileSpec { name: f.name, size: Some(f.size), sha256: Some(f.sha256), max: f.size };
            let dir = models.join(f.folder);
            let result = download_url(&spec, f.url, &dir, &Options::default(), cancel, &mut |p| {
                let done = (before + p.done.min(f.size)).min(total);
                row.progress(done, total);
                self.set_download(set, Download::Running { bytes: done, total });
            });
            match result {
                Ok(()) => before += f.size,
                Err(DownloadError::Cancelled) => return Download::Cancelled,
                Err(DownloadError::AllMirrorsFailed { errors, .. }) => {
                    return Download::Failed(format!(
                        "could not download the {}: {}",
                        f.label,
                        errors.join("; ").chars().take(300).collect::<String>()
                    ));
                }
                Err(e) => return Download::Failed(format!("{}: {e}", f.label)),
            }
        }
        Download::Done
    }

    #[cfg(target_arch = "wasm32")]
    fn fetch(
        &self,
        _: ModelSet,
        _: &std::path::Path,
        _: &[lightcraft_parsing::ModelFile],
        _: u64,
        _: &AtomicBool,
        _: &crate::activity::TaskGuard,
    ) -> Download {
        Download::Failed("the web build downloads no models".into())
    }
}
