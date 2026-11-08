//! Fork feature (fork/shared-models.md): every AI model on the Mac lives in the folder LightCraft shares with the other
//! ArtCraft apps, `~/Library/Application Support/ArtCraft/models/<model-id>/`.
//!
//! This is the one place that works the folder out ([`shared_models_dir`]); SAM 3, LaMa, BiRefNet, the reflection models,
//! the face models and the denoise models all ask it. Face and denoise models also have the old folder
//! (`<config>/models`, `<config>/denoise-models`), which keeps their `settings.json`, the user's `catalog.json` and the
//! download staging folder: [`ModelRoots`] looks in the shared folder first, then in the old one, and puts new
//! downloads in the shared one. [`migrate_old_models`] moves what the old folder holds, once, at start.
//!
//! Rules (techneissa, 2026-11-01/02): never delete the ArtCraft folder, never touch `ArtCraft/models/sam3` or the `sam3`
//! link in the old folder, never download a model that is already there. Off the Mac there is no shared folder and
//! nothing changes.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod migrate;

pub use migrate::{Migration, migrate_old_models, migrate_old_models_in};

/// The folder the Mac shares with the other ArtCraft apps for models, `<home>/Library/Application Support/ArtCraft/models`.
/// `None` off the Mac, or without a home folder.
pub fn shared_models_dir(home: Option<OsString>) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = home.filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library").join("Application Support").join("ArtCraft").join("models"))
}

/// [`shared_models_dir`] for this user (`$HOME`).
pub fn default_shared_models_dir() -> Option<PathBuf> {
    shared_models_dir(std::env::var_os("HOME"))
}

/// Where the models of one kind (faces, denoise) are: the `old` folder, which also holds their settings, and the
/// `shared` one (`None`: no shared folder, then only `old` is used).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRoots {
    pub old: PathBuf,
    pub shared: Option<PathBuf>,
}

impl ModelRoots {
    /// Only the `old` folder (a host without a shared one, a custom folder set by the environment, a test).
    pub fn old_only(old: impl Into<PathBuf>) -> Self {
        Self { old: old.into(), shared: None }
    }

    /// The folders to look in, the shared one first.
    pub fn search(&self) -> impl Iterator<Item = &Path> {
        self.shared.as_deref().into_iter().chain(std::iter::once(self.old.as_path()))
    }

    /// The folder of the model `id` that holds all of `files` (its model and manifest): the shared one first, then the old one.
    pub fn existing_home(&self, id: &str, files: &[&str]) -> Option<PathBuf> {
        self.search().map(|root| root.join(id)).find(|home| files.iter().all(|f| home.join(f).is_file()))
    }

    /// Where a new copy of the model `id` goes: the shared folder when there is one.
    pub fn install_home(&self, id: &str) -> PathBuf {
        self.shared.as_deref().unwrap_or(&self.old).join(id)
    }

    /// Whether `home` is a model folder inside the shared folder.
    pub fn is_shared(&self, home: &Path) -> bool {
        self.shared.as_deref().is_some_and(|s| home.starts_with(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_shared_folder_is_artcraft_models_in_application_support() {
        assert_eq!(shared_models_dir(Some("/Users/x".into())), Some(PathBuf::from("/Users/x/Library/Application Support/ArtCraft/models")));
        assert_eq!(shared_models_dir(Some(OsString::new())), None);
        assert_eq!(shared_models_dir(None), None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn off_the_mac_there_is_no_shared_folder() {
        assert_eq!(shared_models_dir(Some("/home/x".into())), None);
    }

    fn layout(name: &str) -> (PathBuf, ModelRoots) {
        let root = std::env::temp_dir().join(format!("lc-model-roots-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let roots = ModelRoots { old: root.join("old"), shared: Some(root.join("shared")) };
        (root, roots)
    }

    fn put(path: &Path) {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
        let _ = std::fs::write(path, b"x");
    }

    #[test]
    fn the_shared_folder_comes_first_and_new_models_go_there() {
        let (root, roots) = layout("order");
        put(&roots.old.join("m").join("m.json"));
        assert_eq!(roots.existing_home("m", &["m.json"]), Some(roots.old.join("m")));
        put(&roots.shared.clone().unwrap_or_default().join("m").join("m.json"));
        assert_eq!(roots.existing_home("m", &["m.json"]), Some(root.join("shared").join("m")));
        assert_eq!(roots.install_home("n"), root.join("shared").join("n"));
        assert!(roots.is_shared(&root.join("shared").join("m")));
        assert!(!roots.is_shared(&root.join("old").join("m")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn without_a_shared_folder_only_the_old_one_is_used() {
        let roots = ModelRoots::old_only("/old");
        assert_eq!(roots.install_home("m"), PathBuf::from("/old/m"));
        assert!(!roots.is_shared(Path::new("/old/m")));
        assert_eq!(roots.search().count(), 1);
    }
}
