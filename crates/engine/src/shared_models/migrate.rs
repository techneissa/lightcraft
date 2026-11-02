//! Moving the face and denoise models of the old folders (`<config>/models/<id>/`, `<config>/denoise-models/<id>/`)
//! into the folder shared with the other ArtCraft apps (`ArtCraft/models/<id>/`), once, when the app starts. The same
//! rules as PhotoCraft's move (`crates/ml/src/migrate.rs` there):
//!
//! * Only a model LightCraft knows (its id, size and SHA-256 are published) moves, and only when the old `model.onnx`
//!   passes the size and SHA-256 check. A model the user added by hand stays where it is; it is still found there.
//! * A move is a `rename` on the same disk: nothing is copied (but the few small files beside the model), nothing is
//!   downloaded.
//! * A checked copy already in the shared folder is never touched, overwritten or downloaded again; only the old
//!   duplicate goes. A shared copy that fails the check is no model anyone can use, so the checked old one replaces it.
//! * A move that fails leaves both copies, says why in the [`Migration`], and the app starts anyway; the old copy is
//!   still found by [`ModelRoots`].
//! * `ArtCraft/models/sam3` and the `sam3` link in the old folder are not models of these kinds and never touched.
//!
//! Nothing here runs while the matching environment variable (`LIGHTCRAFT_FACE_MODELS`, `LIGHTCRAFT_DENOISE_MODELS`)
//! picks another folder, or off the Mac.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lightcraft_faces::hash::sha256_file;

/// What a migration did, in sentences for a log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Migration {
    /// Models moved, old duplicates removed.
    pub done: Vec<String>,
    /// What could not be done, and why; the files involved are still where they were.
    pub failed: Vec<String>,
}

impl Migration {
    /// Whether there was nothing to do and nothing went wrong.
    pub fn is_empty(&self) -> bool {
        self.done.is_empty() && self.failed.is_empty()
    }
}

/// One model LightCraft knows: its folder name and what its file must be.
struct Known {
    id: String,
    size: Option<u64>,
    sha256: Option<String>,
}

/// The model file of a face or denoise model folder.
const MODEL_FILE: &str = "model.onnx";

/// A kind of model: where its old folder is, its manifest name, and the models LightCraft knows.
struct Kind {
    label: &'static str,
    old: PathBuf,
    manifest: &'static str,
    models: Vec<Known>,
}

/// Moves the face and denoise models of the old folders into the shared folder (see the module docs), for this user
/// (`$HOME`, the config folder, the two environment variables). Blocking: hashing a 200 MB file takes about a second,
/// so call it from a worker thread. Never panics.
pub fn migrate_old_models() -> Migration {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    let config = crate::config::config_dir();
    migrate_old_models_in(
        std::env::var_os("HOME"),
        config.as_deref().filter(|_| !set("LIGHTCRAFT_FACE_MODELS")).map(|c| c.join("models")).as_deref(),
        config.as_deref().filter(|_| !set("LIGHTCRAFT_DENOISE_MODELS")).map(|c| c.join("denoise-models")).as_deref(),
    )
}

/// [`migrate_old_models`] with the home folder and the two old folders (`None`: that kind is left alone) as parameters.
pub fn migrate_old_models_in(home: Option<OsString>, faces_old: Option<&Path>, denoise_old: Option<&Path>) -> Migration {
    let Some(shared) = super::shared_models_dir(home) else { return Migration::default() };
    let mut kinds = Vec::new();
    if let Some(old) = faces_old {
        let models = lightcraft_faces::known::all().into_iter().map(|m| Known { id: m.id, size: m.size_bytes, sha256: m.sha256 }).collect();
        kinds.push(Kind { label: "face", old: old.to_path_buf(), manifest: "face-model.json", models });
    }
    if let Some(old) = denoise_old {
        let models = lightcraft_denoise_core::known::all()
            .into_iter()
            .map(|k| Known { id: k.manifest.id, size: k.manifest.size_bytes, sha256: k.manifest.sha256 })
            .collect();
        kinds.push(Kind { label: "denoise", old: old.to_path_buf(), manifest: "denoise-model.json", models });
    }
    migrate(&shared, &kinds)
}

fn migrate(shared: &Path, kinds: &[Kind]) -> Migration {
    let mut report = Migration::default();
    for kind in kinds {
        if !kind.old.is_dir() {
            continue;
        }
        for model in &kind.models {
            move_model(&mut report, shared, kind, model);
        }
    }
    report
}

fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

/// Whether `path` is the published file: its size and SHA-256, those that are known.
fn verify(path: &Path, model: &Known) -> Result<(), String> {
    if let Some(size) = model.size {
        let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        if len != size {
            return Err(format!("its size is {len} bytes, not {size}"));
        }
    }
    if let Some(want) = &model.sha256 {
        let got = sha256_file(path).map_err(|e| e.to_string())?;
        if !got.eq_ignore_ascii_case(want) {
            return Err("its SHA-256 differs".into());
        }
    }
    Ok(())
}

/// Moves `old/<id>/` to `shared/<id>/` if it holds the model and it is fine.
fn move_model(report: &mut Migration, shared: &Path, kind: &Kind, model: &Known) {
    let id = &model.id;
    let from = kind.old.join(id);
    let Ok(meta) = std::fs::symlink_metadata(&from) else { return };
    let from_file = from.join(MODEL_FILE);
    // A link or a stray file where the folder should be, or a folder with no model in it: not ours to move.
    if !meta.file_type().is_dir() || !is_plain_file(&from_file) {
        return;
    }
    // A download or install is writing there right now.
    if exists(&from.join("model.onnx.part")) {
        report.failed.push(format!("left {} in the old folder: a {} model is being installed there", from.display(), kind.label));
        return;
    }
    // Only a copy that is the published model moves; a wrong or half-written one stays where it is.
    if let Err(e) = verify(&from_file, model) {
        report.failed.push(format!("left {} in the old folder: it is not the published {id} file ({e})", from_file.display()));
        return;
    }
    let known = model.size.is_some() || model.sha256.is_some();
    let dir = shared.join(id);
    let to = dir.join(MODEL_FILE);
    if exists(&to) {
        // A checked copy is already there: it stays exactly as it is, and only the old duplicate goes.
        if known && verify(&to, model).is_ok() {
            remove_old(report, &from, &dir, kind);
            return;
        }
        if !known {
            report.failed.push(format!(
                "left {} and {} where they are: the {id} model has no published hash to tell them apart",
                from.display(),
                dir.display()
            ));
            return;
        }
        // The shared copy fails the check: nobody can use it, so the checked old one takes its place (below).
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        report.failed.push(format!("left {} in the old folder: cannot create {} ({e})", from.display(), dir.display()));
        return;
    }
    // The small files go first (as copies), the model last, so that at no moment is a folder half a model.
    let mut copied: Vec<PathBuf> = Vec::new();
    for name in small_files(&from, kind) {
        let target = dir.join(&name);
        if let Err(e) = copy_file(&from.join(&name), &target) {
            for c in &copied {
                let _ = std::fs::remove_file(c);
            }
            report.failed.push(format!("left {} in the old folder: could not copy {name} to {} ({e})", from.display(), dir.display()));
            return;
        }
        copied.push(target);
    }
    match std::fs::rename(&from_file, &to) {
        Ok(()) => {
            report.done.push(format!("moved {} to {}", from.display(), dir.display()));
            for name in small_files(&from, kind) {
                let _ = std::fs::remove_file(from.join(name));
            }
            // Only an empty folder goes; a file of someone else keeps it.
            let _ = std::fs::remove_dir(&from);
        }
        Err(e) => {
            // Leave no half model in the shared folder: the old folder is still complete.
            for c in &copied {
                let _ = std::fs::remove_file(c);
            }
            let _ = std::fs::remove_dir(&dir);
            report.failed.push(format!("could not move {} to {}: {e}; both stay where they are", from_file.display(), to.display()));
        }
    }
}

/// The plain files of a model folder other than the model: its manifest and acceptance record, and what the app wrote
/// beside them.
fn small_files(home: &Path, kind: &Kind) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(home) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n != MODEL_FILE && !n.ends_with(".part"))
        .collect();
    names.sort_by_key(|n| n != kind.manifest);
    names
}

fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let part = to.with_extension("migrating");
    std::fs::copy(from, &part)?;
    std::fs::rename(&part, to).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// A checked copy is in the shared folder `dir`: removes the old duplicate in `from`, and gives the shared folder the
/// small files it lacks (the user's acceptance record) before they go.
fn remove_old(report: &mut Migration, from: &Path, dir: &Path, kind: &Kind) {
    for name in small_files(from, kind) {
        let target = dir.join(&name);
        if !exists(&target) && copy_file(&from.join(&name), &target).is_err() {
            report.failed.push(format!("left {} in the old folder: could not copy it to {}", from.join(&name).display(), dir.display()));
            return;
        }
        let _ = std::fs::remove_file(from.join(&name));
    }
    match std::fs::remove_file(from.join(MODEL_FILE)) {
        Ok(()) => {
            let _ = std::fs::remove_dir(from);
            report.done.push(format!("removed the old duplicate in {} ({} is already in the shared folder)", from.display(), dir.display()));
        }
        Err(e) => report.failed.push(format!("could not remove the old duplicate {}: {e}", from.join(MODEL_FILE).display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> std::io::Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("lc-migrate-models-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
        fn shared(&self) -> PathBuf {
            self.0.join("shared")
        }
        fn old(&self) -> PathBuf {
            self.0.join("old")
        }
        fn kinds(&self, sha: Option<&str>) -> Vec<Kind> {
            vec![Kind {
                label: "face",
                old: self.old(),
                manifest: "face-model.json",
                models: vec![Known { id: "m".into(), size: Some(3), sha256: sha.map(str::to_string) }],
            }]
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn put(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, bytes)
    }

    fn old_model(t: &Temp, bytes: &[u8]) -> std::io::Result<()> {
        put(&t.old().join("m").join("model.onnx"), bytes)?;
        put(&t.old().join("m").join("face-model.json"), b"{}")?;
        put(&t.old().join("m").join("installed.json"), b"{\"a\":1}")
    }

    #[test]
    fn a_model_moves_with_its_small_files_and_the_old_folder_goes() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        old_model(&t, b"abc")?;
        put(&t.old().join("settings.json"), b"{}")?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert!(report.failed.is_empty(), "{report:?}");
        let home = t.shared().join("m");
        assert_eq!(std::fs::read(home.join("model.onnx"))?, b"abc");
        assert_eq!(std::fs::read(home.join("installed.json"))?, b"{\"a\":1}");
        assert!(home.join("face-model.json").is_file());
        assert!(!t.old().join("m").exists(), "the then empty model folder goes");
        assert!(t.old().join("settings.json").exists(), "the settings stay in the old folder");
        // A second start finds nothing to do.
        assert!(migrate(&t.shared(), &t.kinds(Some(ABC))).is_empty());
        Ok(())
    }

    #[test]
    fn a_checked_shared_copy_is_left_alone_and_only_the_old_duplicate_goes() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        let shared = t.shared().join("m").join("model.onnx");
        put(&shared, b"abc")?;
        put(&t.shared().join("m").join("installed.json"), b"{\"shared\":1}")?;
        let before = std::fs::metadata(&shared)?.modified()?;
        old_model(&t, b"abc")?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(std::fs::read(&shared)?, b"abc");
        assert_eq!(std::fs::metadata(&shared)?.modified()?, before, "the shared file is not rewritten");
        assert_eq!(std::fs::read(t.shared().join("m").join("installed.json"))?, b"{\"shared\":1}", "the shared record stays");
        assert!(t.shared().join("m").join("face-model.json").is_file(), "a file the shared folder lacked is given to it");
        assert!(!t.old().join("m").exists());
        assert!(report.done.iter().any(|l| l.contains("old duplicate")), "{report:?}");
        Ok(())
    }

    #[test]
    fn a_corrupt_shared_copy_is_replaced_by_the_checked_old_one() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        put(&t.shared().join("m").join("model.onnx"), b"abd")?;
        old_model(&t, b"abc")?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(std::fs::read(t.shared().join("m").join("model.onnx"))?, b"abc");
        assert!(!t.old().join("m").exists());
        Ok(())
    }

    #[test]
    fn a_corrupt_old_file_is_not_moved() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        old_model(&t, b"abd")?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert_eq!(std::fs::read(t.old().join("m").join("model.onnx"))?, b"abd");
        assert!(!t.shared().exists());
        assert!(report.failed.iter().any(|l| l.contains("not the published")), "{report:?}");
        Ok(())
    }

    #[test]
    fn a_failed_rename_leaves_both_copies_and_says_why() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        old_model(&t, b"abc")?;
        // A folder where the model file should go: the rename cannot succeed, and nothing is deleted to make room.
        std::fs::create_dir_all(t.shared().join("m").join("model.onnx"))?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert_eq!(std::fs::read(t.old().join("m").join("model.onnx"))?, b"abc", "the old file stays");
        assert!(t.old().join("m").join("face-model.json").is_file());
        assert!(t.shared().join("m").join("model.onnx").is_dir(), "the shared folder is as it was");
        assert!(!t.shared().join("m").join("face-model.json").exists(), "no half model is left in the shared folder");
        assert!(report.failed.iter().any(|l| l.contains("could not move") && l.contains("both stay")), "{report:?}");
        Ok(())
    }

    #[test]
    fn a_shared_folder_that_cannot_be_made_is_a_failure_not_a_crash() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        old_model(&t, b"abc")?;
        put(&t.shared(), b"not a folder")?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert_eq!(std::fs::read(t.old().join("m").join("model.onnx"))?, b"abc");
        assert!(report.failed.iter().any(|l| l.contains("cannot create")), "{report:?}");
        Ok(())
    }

    #[test]
    fn a_model_being_installed_is_not_moved() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        old_model(&t, b"abc")?;
        put(&t.old().join("m").join("model.onnx.part"), b"ab")?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert!(t.old().join("m").join("model.onnx").is_file());
        assert!(!t.shared().exists());
        assert!(report.failed.iter().any(|l| l.contains("being installed")), "{report:?}");
        Ok(())
    }

    #[test]
    fn unknown_models_the_sam3_link_and_a_missing_folder_are_left_alone() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        put(&t.old().join("mine").join("model.onnx"), b"abc")?;
        put(&t.old().join("sam3-real").join("model.safetensors"), b"x")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.old().join("sam3-real"), t.old().join("sam3"))?;
        let report = migrate(&t.shared(), &t.kinds(Some(ABC)));
        assert!(report.is_empty(), "{report:?}");
        assert!(t.old().join("mine").join("model.onnx").is_file());
        assert!(!t.shared().exists());
        let none = vec![Kind { label: "face", old: t.0.join("nowhere"), manifest: "face-model.json", models: Vec::new() }];
        assert!(migrate(&t.shared(), &none).is_empty());
        Ok(())
    }

    #[test]
    fn a_model_without_a_published_hash_never_overwrites_the_shared_one() -> Result<(), Box<dyn std::error::Error>> {
        let t = Temp::new()?;
        old_model(&t, b"abc")?;
        put(&t.shared().join("m").join("model.onnx"), b"other")?;
        let kinds =
            vec![Kind { label: "face", old: t.old(), manifest: "face-model.json", models: vec![Known { id: "m".into(), size: None, sha256: None }] }];
        let report = migrate(&t.shared(), &kinds);
        assert_eq!(std::fs::read(t.shared().join("m").join("model.onnx"))?, b"other");
        assert!(t.old().join("m").join("model.onnx").is_file());
        assert!(!report.failed.is_empty());
        Ok(())
    }

    #[test]
    fn nothing_moves_without_a_home_folder_or_off_the_mac() {
        let old = std::env::temp_dir().join(format!("lc-migrate-none-{}", std::process::id()));
        assert!(migrate_old_models_in(None, Some(&old), Some(&old)).is_empty());
        assert!(migrate_old_models_in(Some(OsString::new()), Some(&old), Some(&old)).is_empty());
        assert!(!old.exists());
    }
}
