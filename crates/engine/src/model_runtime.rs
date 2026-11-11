//! Fork feature (`fork/coreml.md`, issue #105): which engine runs each AI model that has a Core ML conversion.
//!
//! One choice per model ([`Runtime`]: Automatic, Core ML, Candle), set in Settings ▸ Performance or with
//! `models.runtime`, kept in `<config>/model-runtimes.json`. Automatic is the default: Core ML when its model is
//! installed and loads, else candle. A missing or damaged file means every model on Automatic.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

pub use lightcraft_coreml::Runtime;

#[cfg(not(target_arch = "wasm32"))]
mod download;
#[cfg(not(target_arch = "wasm32"))]
pub use download::{RELEASES, Release, release};

/// The settings file's name in the config folder.
pub const FILE: &str = "model-runtimes.json";
/// Largest settings file read.
const MAX_FILE: u64 = 64 << 10;

/// The models with a Core ML conversion: (model id, the feature it serves, as Settings names it).
pub const MODELS: &[(&str, &str)] = &[
    (lightcraft_learn::ENCODER_ID, "Learn My Edits"),
    (crate::people_parsing::COREML_MODELS[0], "People Parts: Faces"),
    (crate::people_parsing::COREML_MODELS[1], "People Parts: Bodies"),
    (crate::people_parsing::COREML_MODELS[2], "People Parts: Face Mesh"),
    (crate::people_parsing::COREML_MODELS[3], "People Parts: Bodies (BiSeNet Set)"),
    (crate::people_parsing::COREML_MODELS[4], "People Parts: Sapiens2"),
    (BIREFNET, "Subject Masks: BiRefNet"),
    (crate::hair_edges::MODEL_DIR, "Hair Edges: BiRefNet Matting"),
    (SAM3, "Object and Describe Masks: SAM 3"),
];

/// BiRefNet's id (its folder), the Subject and Background model (issue #264); BiRefNet Matting's is
/// [`crate::hair_edges::MODEL_DIR`].
pub const BIREFNET: &str = "birefnet";

/// SAM 3's id (its folder; issue #268). Its Core ML version is converted on this Mac into the sibling folder
/// `sam3-coreml` (`tools/convert-coreml-sam3.py`) and never published: SAM 3's licence is not on the allowlist.
pub const SAM3: &str = "sam3";

/// `<config>/model-runtimes.json`.
pub fn default_file() -> Option<PathBuf> {
    crate::config::config_dir().map(|c| c.join(FILE))
}

fn read(path: &Path) -> BTreeMap<String, Runtime> {
    let Ok(meta) = std::fs::metadata(path) else { return BTreeMap::new() };
    if meta.len() > MAX_FILE {
        log::warn!("{}: too large, ignored", path.display());
        return BTreeMap::new();
    }
    let parsed: Option<BTreeMap<String, String>> = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let Some(map) = parsed else {
        log::warn!("{}: not a runtimes file, ignored", path.display());
        return BTreeMap::new();
    };
    map.into_iter().filter_map(|(model, id)| Some((model, Runtime::from_id(&id)?))).collect()
}

fn write(path: &Path, map: &BTreeMap<String, Runtime>) -> Result<(), String> {
    let json: BTreeMap<&str, &str> = map.iter().map(|(m, r)| (m.as_str(), r.id())).collect();
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.part");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The runtime choices of a session, and the file they are kept in (`None`: in memory only, as in tests).
#[derive(Default)]
pub struct ModelRuntimes {
    file: Mutex<Option<PathBuf>>,
    choices: Mutex<BTreeMap<String, Runtime>>,
    /// The Core ML downloads, by model.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    downloads: Mutex<HashMap<String, crate::lama_fill::Download>>,
}

/// The web build downloads no Core ML model.
#[cfg(target_arch = "wasm32")]
impl ModelRuntimes {
    /// Where `model`'s Core ML download stands: never started on the web.
    pub fn download_state(&self, _model: &str) -> crate::lama_fill::Download {
        crate::lama_fill::Download::Idle
    }
}

impl ModelRuntimes {
    /// Keep the choices in `file`, reading what it holds.
    pub fn open(&self, file: Option<PathBuf>) {
        let choices = file.as_deref().map(read).unwrap_or_default();
        *self.choices.lock().unwrap_or_else(PoisonError::into_inner) = choices;
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = file;
    }

    /// The runtime chosen for `model` (Automatic when none was).
    pub fn get(&self, model: &str) -> Runtime {
        self.choices.lock().unwrap_or_else(PoisonError::into_inner).get(model).copied().unwrap_or_default()
    }

    /// Choose `runtime` for `model`: the file is saved first, and nothing changes when it can't be. An unknown model is
    /// an error.
    pub fn set(&self, model: &str, runtime: Runtime) -> Result<(), String> {
        if !MODELS.iter().any(|(id, _)| *id == model) {
            return Err(format!("no model `{model}` has a Core ML version"));
        }
        let mut choices = self.choices.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = choices.clone();
        next.insert(model.to_string(), runtime);
        // saved first: a choice that could not be saved is not applied
        if let Some(path) = self.file.lock().unwrap_or_else(PoisonError::into_inner).as_deref() {
            write(path, &next)?;
        }
        *choices = next;
        Ok(())
    }
}
