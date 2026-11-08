//! The Core ML models' download (`fork/coreml.md`): a zipped `.mlpackage` from the `models-coreml-1` release of
//! techneissa/lightcraft, started only after the user has been asked (Settings ▸ Performance ▸ AI Models; an agent's
//! `models.downloadCoreml` with `confirm`). The zip is fetched by [`lightcraft_fetch`] (size and SHA-256 checked),
//! unpacked next to the model's candle weights in the shared models folder, and deleted. Without the release the
//! download fails with a message and the model keeps running on candle; nothing is ever converted in the app.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, PoisonError};

use super::ModelRuntimes;
use crate::activity::{Activity, Cancel, Unit};
use crate::lama_fill::Download;

/// A converted model on the release: where it is, what it weighs, its hash and its licence (only models under 2 GB
/// whose licence allows redistribution are published).
#[derive(Debug, Clone, Copy)]
pub struct Release<'a> {
    pub model: &'a str,
    /// The zip's name on the release.
    pub file: &'a str,
    pub url: &'a str,
    pub bytes: u64,
    pub sha256: &'a str,
    /// The package folder the zip holds (its entries all start with it).
    pub package: &'a str,
    pub license: &'a str,
}

pub const RELEASES: &[Release<'static>] = &[Release {
    model: lightcraft_learn::ENCODER_ID,
    file: "dinov2-small.mlpackage.zip",
    url: "https://github.com/techneissa/lightcraft/releases/download/models-coreml-1/dinov2-small.mlpackage.zip",
    bytes: 43_397_268,
    sha256: "c69bd095b956fd60e74b726e7d0ec85417ef0c1f0ced52ee9a6d162091e4eaed",
    package: lightcraft_learn::image_encoder::COREML_PACKAGE,
    license: "Apache-2.0",
}];

/// The release of `model`, if it has one.
pub fn release(model: &str) -> Option<&'static Release<'static>> {
    RELEASES.iter().find(|r| r.model == model)
}

/// Most entries a package zip may hold.
const MAX_ENTRIES: usize = 256;

/// Fetch `r`'s zip from `url` into `dir` (the model's `coreml` folder), unpack its package there (replacing an older
/// one) and delete the zip. Errors in words the user can act on.
pub(crate) fn install(r: &Release<'_>, url: &str, dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64)) -> Result<(), Download> {
    use lightcraft_fetch::{DownloadError, FileSpec, Options, download_url};
    std::fs::create_dir_all(dir).map_err(|e| Download::Failed(format!("{}: {e}", dir.display())))?;
    let spec = FileSpec { name: r.file, size: Some(r.bytes), sha256: Some(r.sha256), max: r.bytes };
    let fetched = download_url(&spec, url, dir, &Options::default(), cancel, &mut |p| progress(p.done.min(r.bytes)));
    match fetched {
        Ok(()) => {}
        Err(DownloadError::Cancelled) => return Err(Download::Cancelled),
        Err(DownloadError::AllMirrorsFailed { errors, .. }) => {
            let why: String = errors.join("; ").chars().take(300).collect();
            return Err(Download::Failed(format!("could not download the Core ML model: {why}")));
        }
        Err(e) => return Err(Download::Failed(e.to_string())),
    }
    let zip = dir.join(r.file);
    let unpacked = unpack(&zip, dir, r.package, r.bytes.saturating_mul(2));
    let _ = std::fs::remove_file(&zip);
    unpacked.map_err(Download::Failed)
}

/// Unpack the `package` folder of `zip` into `dir`, through `<package>.part`, with at most `max` bytes in all. An
/// entry outside `package`, a link, too many entries or bytes: an error, and nothing is installed.
fn unpack(zip: &Path, dir: &Path, package: &str, max: u64) -> Result<(), String> {
    let file = std::fs::File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("the Core ML download is not a zip: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("the Core ML download holds {} files", archive.len()));
    }
    let part = dir.join(format!("{package}.part"));
    let _ = std::fs::remove_dir_all(&part);
    let written = (0..archive.len()).try_fold(0u64, |total, i| {
        let mut entry = archive.by_index(i).map_err(|e| format!("the Core ML download is damaged: {e}"))?;
        let rest: PathBuf = match entry.enclosed_name().and_then(|p| p.strip_prefix(package).ok().map(Path::to_path_buf)) {
            Some(rest) if !entry.is_symlink() => rest,
            _ => return Err(format!("the Core ML download holds `{}`, outside {package}", entry.name())),
        };
        let target = part.join(&rest);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
            return Ok(total);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let room = max.saturating_sub(total);
        let mut out = std::fs::File::create(&target).map_err(|e| format!("{}: {e}", target.display()))?;
        let n = std::io::copy(&mut (&mut entry).take(room.saturating_add(1)), &mut out).map_err(|e| format!("{}: {e}", target.display()))?;
        if n > room {
            return Err("the Core ML download unpacks to more than expected".into());
        }
        Ok(total + n)
    });
    let installed = written.and_then(|_| {
        if !part.join("Manifest.json").is_file() {
            return Err(format!("the Core ML download holds no {package}"));
        }
        let dest = dir.join(package);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))
    });
    if installed.is_err() {
        let _ = std::fs::remove_dir_all(&part);
    }
    installed
}

impl ModelRuntimes {
    /// Where `model`'s Core ML download stands.
    pub fn download_state(&self, model: &str) -> Download {
        self.downloads.lock().unwrap_or_else(PoisonError::into_inner).get(model).cloned().unwrap_or(Download::Idle)
    }

    fn set_state(&self, model: &str, state: Download) {
        self.downloads.lock().unwrap_or_else(PoisonError::into_inner).insert(model.to_string(), state);
    }

    /// Download `r` into `dir` on a thread, with a row in the activity stack; `done` runs after it is installed.
    pub fn start_download(
        self: &Arc<Self>,
        activity: &Activity,
        r: &'static Release<'static>,
        dir: PathBuf,
        done: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        {
            let mut downloads = self.downloads.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(downloads.get(r.model), Some(Download::Running { .. })) {
                return Err("the Core ML model is already downloading".into());
            }
            downloads.insert(r.model.to_string(), Download::Running { bytes: 0, total: r.bytes });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let row = activity.start("download", "Downloading model", Cancel::Flag(cancel.clone()));
        row.set_unit(Unit::Bytes);
        row.detail("Core ML");
        row.progress(0, r.bytes);
        let this = self.clone();
        std::thread::Builder::new()
            .name("coreml-download".into())
            .spawn(move || {
                let row = row;
                let run = || {
                    install(r, r.url, &dir, &cancel, &mut |n| {
                        row.progress(n, r.bytes);
                        this.set_state(r.model, Download::Running { bytes: n, total: r.bytes });
                    })
                };
                let state = match crate::guard::catch("the Core ML download", run) {
                    Ok(Ok(())) => {
                        done();
                        Download::Done
                    }
                    Ok(Err(state)) => state,
                    Err(e) => Download::Failed(e),
                };
                this.set_state(r.model, state);
            })
            .map(|_| ())
            .map_err(|e| {
                self.set_state(r.model, Download::Idle);
                format!("could not start the download: {e}")
            })
    }
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
