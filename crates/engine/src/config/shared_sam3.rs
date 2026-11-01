//! Fork feature (fork/shared-sam3.md): the SAM 3 model is looked for in the folder LightCraft shares with
//! PhotoCraft, `~/Library/Application Support/ArtCraft/models/sam3`, before LightCraft's own
//! `<config>/models/sam3`.
//!
//! Rules (techneissa, 2026-11-01): never move the model back, delete the ArtCraft folder, or replace
//! `<config>/models/sam3` (a symlink to the shared folder) with a real folder. Any new model both apps could use
//! goes in `ArtCraft/models/<model-id>/`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The files that make a SAM 3 model folder (as `lightcraft_segment::is_model_dir` checks, which is an optional dependency here).
const MODEL_FILES: [&str; 3] = ["model.safetensors", "vocab.json", "merges.txt"];

/// The folder the Mac shares with PhotoCraft for the SAM 3 model. `None` off the Mac, or without a home folder.
pub(super) fn shared_sam3_dir(home: Option<OsString>) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = home.filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library").join("Application Support").join("ArtCraft").join("models").join("sam3"))
}

/// Where the SAM 3 model is: a non-empty `var` (`LIGHTCRAFT_SAM3_DIR`) wins; then the `shared` folder when it holds
/// the model; then the old `<config>/models/sam3` when it holds the model; with neither, the `shared` folder (so a
/// download goes there and never into a second copy beside the symlink), or the old one when there is no shared folder.
pub(super) fn sam3_dir(var: Option<OsString>, shared: Option<PathBuf>, config: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(v) = var.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    let old = config.map(|d| d.join("models").join("sam3"));
    let holds = |dir: &Path| MODEL_FILES.iter().all(|f| dir.join(f).is_file());
    if let Some(shared) = &shared
        && holds(shared)
    {
        return Some(shared.clone());
    }
    if let Some(old) = &old
        && holds(old)
    {
        return Some(old.clone());
    }
    shared.or(old)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-shared-sam3-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put_model(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for f in MODEL_FILES {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
    }

    /// (root, shared folder, config folder, old folder), the folders not created.
    fn layout(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root = scratch(name);
        let shared = root.join("ArtCraft").join("models").join("sam3");
        let config = root.join("LightCraft");
        let old = config.join("models").join("sam3");
        (root, shared, config, old)
    }

    #[test]
    fn the_environment_variable_wins() {
        let (root, shared, config, _) = layout("env");
        put_model(&shared);
        assert_eq!(sam3_dir(Some("elsewhere".into()), Some(shared.clone()), Some(config.clone())), Some(PathBuf::from("elsewhere")));
        // an empty value counts as unset
        assert_eq!(sam3_dir(Some(OsString::new()), Some(shared.clone()), Some(config)), Some(shared));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_shared_folder_comes_first_when_it_holds_the_model() {
        let (root, shared, config, old) = layout("both");
        put_model(&shared);
        put_model(&old);
        assert_eq!(sam3_dir(None, Some(shared.clone()), Some(config)), Some(shared));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_old_folder_is_used_when_only_it_holds_the_model() {
        let (root, shared, config, old) = layout("old");
        put_model(&old);
        assert_eq!(sam3_dir(None, Some(shared.clone()), Some(config.clone())), Some(old.clone()));
        // an unfinished shared folder (one file) does not count as the model
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("vocab.json"), b"x").unwrap();
        assert_eq!(sam3_dir(None, Some(shared), Some(config)), Some(old));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn the_old_folder_as_a_symlink_to_the_shared_one_resolves_to_the_shared_one() {
        let (root, shared, config, old) = layout("link");
        put_model(&shared);
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&shared, &old).unwrap();
        assert_eq!(sam3_dir(None, Some(shared.clone()), Some(config)), Some(shared));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn with_neither_the_shared_folder_is_chosen_so_a_download_goes_there() {
        let (root, shared, config, old) = layout("neither");
        assert_eq!(sam3_dir(None, Some(shared.clone()), Some(config.clone())), Some(shared.clone()));
        // a symlink at the old place that points nowhere yet does not pull the download into it
        #[cfg(unix)]
        {
            std::fs::create_dir_all(old.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&shared, &old).unwrap();
            assert_eq!(sam3_dir(None, Some(shared.clone()), Some(config.clone())), Some(shared.clone()));
        }
        // no shared folder (off the Mac): the old one, then nothing
        assert_eq!(sam3_dir(None, None, Some(config)), Some(old));
        assert_eq!(sam3_dir(None, None, None), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_shared_folder_is_under_the_home_folder_on_the_mac_only() {
        let dir = shared_sam3_dir(Some("/h".into()));
        if cfg!(target_os = "macos") {
            assert_eq!(dir, Some(PathBuf::from("/h/Library/Application Support/ArtCraft/models/sam3")));
            assert_eq!(shared_sam3_dir(Some(OsString::new())), None);
            assert_eq!(shared_sam3_dir(None), None);
        } else {
            assert_eq!(dir, None);
        }
    }

    #[cfg(feature = "sam")]
    fn holds_model(dir: &Path) -> bool {
        MODEL_FILES.iter().all(|f| dir.join(f).is_file())
    }

    /// A tiny local server answering every request with `body`.
    #[cfg(feature = "sam")]
    fn serve(body: &'static [u8]) -> String {
        use std::io::{BufRead, BufReader, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/m", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                let _ = s.write_all(body);
            }
        });
        base
    }

    #[cfg(feature = "sam")]
    fn download_into(dir: &Path) {
        use lightcraft_segment::fetch::{self, FileSpec};
        use std::sync::atomic::AtomicBool;
        let files = [
            FileSpec { name: "vocab.json", size: None, sha256: None, max: 1 << 10 },
            FileSpec { name: "merges.txt", size: None, sha256: None, max: 1 << 10 },
            FileSpec { name: "model.safetensors", size: None, sha256: None, max: 1 << 10 },
        ];
        let mirrors = [serve(b"data")];
        fetch::download(&files, &mirrors, dir, &fetch::options(), &AtomicBool::new(false), &mut |_| {}).unwrap();
    }

    /// The download creates the missing shared folder (and its parents) and fills it.
    #[cfg(feature = "sam")]
    #[test]
    fn a_download_into_a_missing_shared_folder_lands_there() {
        let (root, shared, config, old) = layout("download");
        let dir = sam3_dir(None, Some(shared.clone()), Some(config)).unwrap();
        assert!(!dir.exists());
        download_into(&dir);
        assert_eq!(dir, shared);
        assert!(holds_model(&shared));
        assert!(!old.exists(), "nothing is written beside it in LightCraft's folder");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A download into a folder reached through the old symlink leaves the symlink a symlink.
    #[cfg(all(unix, feature = "sam"))]
    #[test]
    fn a_download_never_replaces_the_symlink() {
        let (root, shared, _, old) = layout("keep-link");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&shared, &old).unwrap();
        download_into(&old);
        assert!(std::fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
        assert!(holds_model(&shared));
        let _ = std::fs::remove_dir_all(root);
    }
}
