//! DIRS-MuGI and DIRS-PAIR in the models folder: the files are used as published, so installing is copying them
//! (after checking they are the published ones) or downloading them, never a conversion.
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub const MODEL: &str = lightcraft_reflections::DIRS_MUGI;
pub const PAIR: &str = lightcraft_reflections::DIRS_PAIR;
/// The models that come as published files rather than a converted folder.
pub const RDNET: &str = lightcraft_reflections::RDNET;
pub const MODELS: [&str; 3] = [MODEL, PAIR, RDNET];

/// Size of the download, for the question asked before it.
pub fn download_bytes(model: &str) -> u64 {
    lightcraft_reflections::download::model_bytes(model)
}

/// Where the files of `model` come from, for the question and the instructions link.
pub fn page(model: &str) -> &'static str {
    lightcraft_reflections::download::page(model)
}

/// A folder the user picked holds the two files of one DIRS model; which one.
pub fn is_dirs_folder(path: &Path) -> Option<&'static str> {
    MODELS.into_iter().find(|m| lightcraft_reflections::installed_model(path, m))
}

/// The two file names a model folder holds.
fn file_names(model: &str) -> [&'static str; 2] {
    match model {
        PAIR => [lightcraft_reflections::PAIR_CHECKPOINT, lightcraft_reflections::SWIN_PRIOR],
        RDNET => [lightcraft_reflections::RDNET_NETWORK, lightcraft_reflections::RDNET_CLASSIFIER],
        _ => [lightcraft_reflections::MUGI_CHECKPOINT, lightcraft_reflections::SWIN_PRIOR],
    }
}

/// Copies the picked folder's two files into `<root>/<model>` once they check out. The folder is created
/// exclusively and removed again on failure; an existing model folder is never touched.
pub fn install_folder(model: &str, source: &Path, root: &Path) -> Result<String, String> {
    lightcraft_reflections::download::verify_model(model, source)?;
    let target = root.join(model);
    if target.exists() {
        return Err("Reflection model already installed".into());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    std::fs::create_dir(&target).map_err(|e| e.to_string())?;
    let result = file_names(model)
        .iter()
        .try_for_each(|name| std::fs::copy(source.join(name), target.join(name)).map(|_| ()).map_err(|e| format!("{name}: {e}")));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&target);
    }
    result.map(|_| model.to_string())
}

/// Downloads the two files into `<root>/<model>` (resuming an earlier try). Only called after the user said yes.
pub fn download(model: &str, root: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<String, String> {
    let target = root.join(model);
    if lightcraft_reflections::installed_model(&target, model) {
        return Err("Reflection model already installed".into());
    }
    lightcraft_reflections::download::download_model(model, &target, cancel, progress)?;
    Ok(model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_that_is_not_the_published_files_is_refused_and_nothing_is_created() {
        let base = std::env::temp_dir().join(format!("lc-dirs-install-{}", std::process::id()));
        let (source, root) = (base.join("source"), base.join("models"));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join(lightcraft_reflections::MUGI_CHECKPOINT), b"small").unwrap();
        std::fs::write(source.join(lightcraft_reflections::SWIN_PRIOR), b"small").unwrap();
        assert_eq!(is_dirs_folder(&source), Some(MODEL));
        assert!(install_folder(MODEL, &source, &root).unwrap_err().contains("not the published file"));
        assert!(!root.exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn an_installed_model_is_not_downloaded_again() {
        let root = std::env::temp_dir().join(format!("lc-dirs-have-{}", std::process::id()));
        let target = root.join(MODEL);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(lightcraft_reflections::MUGI_CHECKPOINT), b"x").unwrap();
        std::fs::write(target.join(lightcraft_reflections::SWIN_PRIOR), b"x").unwrap();
        let cancel = AtomicBool::new(false);
        assert!(download(MODEL, &root, &cancel, &mut |_, _| {}).unwrap_err().contains("already installed"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
