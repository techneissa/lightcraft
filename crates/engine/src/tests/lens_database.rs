//! The lens database folder chosen in Settings: precedence of `LIGHTCRAFT_LENSFUN_DIR`, the status
//! for a missing folder, an empty one and one with a small synthetic lensfun file, and reloading.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::lens_profiles::{self, FolderSource, Problem};

const XML: &str = r#"<lensdatabase>
  <camera><maker>Testcam</maker><model>TC-1</model><mount>T</mount><cropfactor>1.5</cropfactor></camera>
  <lens><maker>Testcam</maker><model>Testcam 18-55mm f/3.5-5.6</model><mount>T</mount><cropfactor>1.5</cropfactor>
    <calibration><distortion model="poly3" focal="18" k1="-0.03" /></calibration>
  </lens></lensdatabase>"#;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-lens-db-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_variable_wins_over_the_setting_unless_empty() {
    let setting = Path::new("/chosen");
    assert_eq!(lens_profiles::resolve(Some("/env".into()), Some(setting)), (FolderSource::Env, Some("/env".into())));
    assert_eq!(lens_profiles::resolve(Some(OsString::new()), Some(setting)), (FolderSource::Setting, Some("/chosen".into())));
    assert_eq!(lens_profiles::resolve(None, Some(setting)), (FolderSource::Setting, Some("/chosen".into())));
    assert_eq!(lens_profiles::resolve(Some("/env".into()), None), (FolderSource::Env, Some("/env".into())));
    assert_eq!(lens_profiles::resolve(None, None), (FolderSource::None, None));
}

#[test]
fn a_missing_folder_says_so() {
    let gone = std::env::temp_dir().join(format!("lc-lens-db-gone-{}", std::process::id()));
    let l = lens_profiles::load(&gone);
    assert!(l.db.is_none());
    assert_eq!((l.profiles, l.problem), (0, Some(Problem::Missing)));
    // a file is not a folder
    let dir = folder("file");
    let file = dir.join("x.xml");
    std::fs::write(&file, XML).unwrap();
    assert!(matches!(lens_profiles::load(&file).problem, Some(Problem::Unreadable(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_without_xml_or_without_lenses_says_so() {
    let dir = folder("empty");
    std::fs::write(dir.join("notes.txt"), "hello").unwrap();
    assert_eq!(lens_profiles::load(&dir).problem, Some(Problem::NoXml));
    std::fs::write(dir.join("broken.xml"), "<not-lensfun/>").unwrap();
    let l = lens_profiles::load(&dir);
    assert_eq!((l.problem, l.skipped, l.profiles), (Some(Problem::NoLenses), 1, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_with_a_small_lensfun_file_gives_its_profile() {
    let dir = folder("small");
    std::fs::write(dir.join("testcam.XML"), XML).unwrap();
    let l = lens_profiles::load(&dir);
    assert_eq!((l.problem, l.profiles, l.skipped), (None, 1, 0));
    assert!(l.db.is_some_and(|db| db.lenses.len() == 1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The process-wide setting: chosen, read, reloaded after a change, cleared. (One test, since the
/// state is shared; skipped when the variable is set, which would win.)
#[test]
fn the_chosen_folder_is_read_reloaded_and_cleared() {
    if std::env::var_os(lens_profiles::ENV).is_some_and(|v| !v.is_empty()) {
        lightcraft_testkit::skip!(Input, "LIGHTCRAFT_LENSFUN_DIR is set and wins over the setting");
        return;
    }
    let dir = folder("state");
    lens_profiles::set_folder(Some(&dir));
    let s = lens_profiles::status();
    assert_eq!((s.source, s.dir.as_deref(), s.problem), (FolderSource::Setting, Some(dir.as_path()), Some(Problem::NoXml)));
    assert!(lens_profiles::database().is_none());
    // files added later are found by reload
    std::fs::write(dir.join("testcam.xml"), XML).unwrap();
    assert!(lens_profiles::database().is_none(), "the folder is not read again by itself");
    lens_profiles::reload();
    assert_eq!(lens_profiles::status().profiles, 1);
    assert!(lens_profiles::database().is_some());
    // another folder replaces it at once
    let other = folder("state-other");
    lens_profiles::set_folder(Some(&other));
    assert!(lens_profiles::database().is_none());
    // cleared: nothing configured
    lens_profiles::set_folder(None);
    let s = lens_profiles::status();
    assert_eq!((s.source, s.dir, s.setting, s.profiles), (FolderSource::None, None, None, 0));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
}
