//! DIRS-MuGI in the models folder: the files are used as published, so installing is copying them
//! (after checking they are the published ones) or downloading them, never a conversion.
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub const MODEL: &str = lightcraft_reflections::DIRS_MUGI;

/// Size of the download, for the question asked before it.
pub fn download_bytes() -> u64 {
    lightcraft_reflections::download::mugi_bytes()
}

/// Where the files come from, for the question and the instructions link.
pub const PAGE: &str = lightcraft_reflections::download::MUGI_PAGE;

/// A folder the user picked holds the two DIRS-MuGI files.
pub fn is_dirs_folder(path: &Path) -> bool {
    lightcraft_reflections::installed_model(path, MODEL)
}

/// Copies the picked folder's two files into `<root>/dirs-mugi` once they check out. The folder is created
/// exclusively and removed again on failure; an existing model folder is never touched.
pub fn install_folder(source: &Path, root: &Path) -> Result<String, String> {
    lightcraft_reflections::download::verify_mugi(source)?;
    let target = root.join(MODEL);
    if target.exists() {
        return Err("Reflection model already installed".into());
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    std::fs::create_dir(&target).map_err(|e| e.to_string())?;
    let result = [lightcraft_reflections::MUGI_CHECKPOINT, lightcraft_reflections::SWIN_PRIOR]
        .iter()
        .try_for_each(|name| std::fs::copy(source.join(name), target.join(name)).map(|_| ()).map_err(|e| format!("{name}: {e}")));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&target);
    }
    result.map(|_| MODEL.to_string())
}

/// Downloads the two files into `<root>/dirs-mugi` (resuming an earlier try). Only called after the user said yes.
pub fn download(root: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<String, String> {
    let target = root.join(MODEL);
    if lightcraft_reflections::installed_model(&target, MODEL) {
        return Err("Reflection model already installed".into());
    }
    lightcraft_reflections::download::download_mugi(&target, cancel, progress)?;
    Ok(MODEL.to_string())
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
        assert!(is_dirs_folder(&source));
        assert!(install_folder(&source, &root).unwrap_err().contains("not the published file"));
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
        assert!(download(&root, &cancel, &mut |_, _| {}).unwrap_err().contains("already installed"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
