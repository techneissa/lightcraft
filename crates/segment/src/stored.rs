//! Text features of the fixed phrases Subject, Sky and Background ask for, kept beside the model (issue #437).
//!
//! A prompt's text features (`[1, 32, 256]`, the CLIP text encoder and its projection) depend only on the phrase and
//! the model, and the encoder is the largest part of SAM 3's prompt model (354 M parameters, about 1.4 GB in float32).
//! So the first time a fixed phrase runs, its features are stored in [`FOLDER`] (a sibling of the `sam3` folder,
//! which is never written to), and every later run reads them instead of loading the encoder; Describe, whose words
//! change, still runs it.
//!
//! One file per engine and device (`coreml-<units>.json`, `candle-<device>.json`): each holds the features its own
//! encoder gave, so the stored path feeds the detector the very numbers the encoder path would. Each file carries the
//! key of the encoder it came from (the converted package, or the weights' size and time); a file with another key is
//! ignored and written again. A file that can't be written only costs the encoder run again after the next load.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The folder beside the `sam3` model folder.
pub const FOLDER: &str = "sam3-text-features";
/// Numbers in one phrase's features: 32 tokens of 256.
pub const LEN: usize = 32 * 256;
/// Largest file read (a few phrases take well under a megabyte).
const MAX_FILE: u64 = 16 << 20;
/// Most phrases one file keeps.
const MAX_PHRASES: usize = 64;

#[derive(Serialize, Deserialize, Default)]
struct File {
    key: String,
    /// Each phrase's features as the bits of its `f32`s, so they read back exactly.
    phrases: BTreeMap<String, Vec<u32>>,
}

/// The stored features of one engine's encoder.
pub struct Store {
    /// Its file; `None`: kept in memory only.
    path: Option<PathBuf>,
    file: File,
}

/// The file of `name` (an engine and device) in the folder beside the model folder `model_dir`.
pub fn path(model_dir: &Path, name: &str) -> Option<PathBuf> {
    model_dir.parent().map(|p| p.join(FOLDER).join(format!("{name}.json")))
}

impl Store {
    /// The store at `path` for the encoder `key`: what the file holds when it was made by that encoder, else empty.
    pub fn open(path: Option<PathBuf>, key: &str) -> Self {
        let read = |p: &Path| -> Option<File> {
            if std::fs::metadata(p).ok()?.len() > MAX_FILE {
                return None;
            }
            serde_json::from_slice(&std::fs::read(p).ok()?).ok()
        };
        let file = path.as_deref().and_then(read).filter(|f| f.key == key).unwrap_or_else(|| File { key: key.into(), phrases: BTreeMap::new() });
        Self { path, file }
    }

    /// Whether this store is the one of the encoder `key` at `path`.
    pub fn is(&self, path: Option<&Path>, key: &str) -> bool {
        self.path.as_deref() == path && self.file.key == key
    }

    /// `phrase`'s features, when stored whole and finite.
    pub fn get(&self, phrase: &str) -> Option<Vec<f32>> {
        let bits = self.file.phrases.get(phrase).filter(|b| b.len() == LEN)?;
        let v: Vec<f32> = bits.iter().map(|b| f32::from_bits(*b)).collect();
        v.iter().all(|x| x.is_finite()).then_some(v)
    }

    /// Keep `phrase`'s features and write the file (a new file renamed over the old one). Features of another size,
    /// or not finite, are not kept; a write that fails is logged, and the features stay in memory.
    pub fn put(&mut self, phrase: &str, features: &[f32]) {
        if features.len() != LEN || features.iter().any(|x| !x.is_finite()) {
            return;
        }
        if self.file.phrases.len() >= MAX_PHRASES && !self.file.phrases.contains_key(phrase) {
            return;
        }
        self.file.phrases.insert(phrase.into(), features.iter().map(|x| x.to_bits()).collect());
        let Some(path) = &self.path else { return };
        if let Err(e) = self.write(path) {
            log::warn!("SAM 3: could not keep the text features in {} ({e})", path.display());
        }
    }

    fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let part = path.with_extension(format!("json.{}.part", std::process::id()));
        std::fs::write(&part, serde_json::to_vec(&self.file).map_err(std::io::Error::other)?)?;
        std::fs::rename(&part, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&part);
        })
    }
}

/// The key of candle's encoder: the weights file's size and time (a new download is a new key).
pub fn weights_key(weights: &Path) -> String {
    let meta = std::fs::metadata(weights).ok();
    let time = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    format!("{}-{time}", meta.map_or(0, |m| m.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stored features read back bit for bit after a new open, and only for the encoder that made them.
    #[test]
    fn features_read_back_exactly_for_their_encoder_only() {
        let root = std::env::temp_dir().join(format!("lc-sam3-stored-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = path(&root.join("sam3"), "candle-cpu");
        let f: Vec<f32> = (0..LEN).map(|i| (i as f32 * 0.37).sin() / 3.0).collect();
        let mut s = Store::open(path.clone(), "a");
        assert!(s.get("sky").is_none());
        s.put("sky", &f);
        let again = Store::open(path.clone(), "a");
        assert_eq!(again.get("sky").map(|v| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>()), Some(f.iter().map(|x| x.to_bits()).collect()));
        assert!(Store::open(path.clone(), "b").get("sky").is_none(), "another encoder's features are not used");
        let mut short = Store::open(path, "a");
        short.put("animal", &f[1..]);
        assert!(short.get("animal").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
