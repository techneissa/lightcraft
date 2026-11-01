//! Fork feature (fork/reflection-removal.md): the reflection models (`dsrnet-s`, `dsrnet-l`, `dsrnet-l-nature`) live in
//! the folder LightCraft shares with PhotoCraft, `~/Library/Application Support/ArtCraft/models/<model-id>/`, like the
//! SAM 3 model (fork/shared-sam3.md). Official puts them in `<config>/reflection-models/<model-id>/`.
//!
//! Rules (techneissa, 2026-11-01): never move a model back, delete the ArtCraft folder, or replace a folder in it with
//! another. The installer only creates `<model-id>` there (it refuses when it exists).

use std::ffi::OsString;
use std::path::PathBuf;

/// The folder the Mac shares with PhotoCraft for models. `None` off the Mac, or without a home folder.
pub(super) fn shared_models_dir(home: Option<OsString>) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = home.filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library").join("Application Support").join("ArtCraft").join("models"))
}

/// The folder that holds one sub-folder per reflection model: a non-empty `var` (`LIGHTCRAFT_REFLECTION_MODELS`) wins,
/// then the `shared` folder, then the old `<config>/reflection-models` (off the Mac, or without a home folder).
pub(super) fn models_dir(var: Option<OsString>, shared: Option<PathBuf>, config: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(v) = var.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    shared.or_else(|| config.map(|c| c.join("reflection-models")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_variable_wins() {
        let shared = Some(PathBuf::from("/shared/models"));
        let config = Some(PathBuf::from("/config"));
        assert_eq!(models_dir(Some("elsewhere".into()), shared.clone(), config.clone()), Some(PathBuf::from("elsewhere")));
        // an empty value counts as unset
        assert_eq!(models_dir(Some(OsString::new()), shared.clone(), config), shared);
    }

    #[test]
    fn the_shared_folder_comes_before_the_config_folder() {
        let shared = Some(PathBuf::from("/shared/models"));
        assert_eq!(models_dir(None, shared.clone(), Some(PathBuf::from("/config"))), shared);
    }

    #[test]
    fn without_a_shared_folder_the_config_folder_is_used() {
        assert_eq!(models_dir(None, None, Some(PathBuf::from("/config"))), Some(PathBuf::from("/config/reflection-models")));
        assert_eq!(models_dir(None, None, None), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_shared_folder_is_artcraft_models_in_application_support() {
        assert_eq!(shared_models_dir(Some("/Users/x".into())), Some(PathBuf::from("/Users/x/Library/Application Support/ArtCraft/models")));
        assert_eq!(shared_models_dir(Some(OsString::new())), None);
        assert_eq!(shared_models_dir(None), None);
    }
}
