use super::*;

/// A fresh folder of its own under the system's temporary one; nothing real is ever touched.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-settings-folder-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn by(computer: &str, at: &str) -> SavedBy {
    SavedBy { computer: computer.into(), at: at.into() }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

#[test]
fn a_saved_file_reads_back_with_who_saved_it_and_when() {
    let dir = scratch("round");
    let stamp = by("Studio Mac", "2026-11-02T14:30:05+01:00");
    let saved = save(dir.to_str().unwrap(), json!({"kind": KIND, "version": 1, "settings": {"a": 1}}), stamp.clone()).unwrap();
    assert_eq!(saved.path, dir.join(FILE_NAME));
    let read = read(dir.to_str().unwrap()).unwrap();
    assert_eq!(read.by, Some(stamp));
    let doc: Value = serde_json::from_str(&read.text).unwrap();
    assert_eq!(doc["settings"]["a"], 1);
    assert_eq!(doc["savedBy"]["computer"], "Studio Mac");
    // nothing but the file is left in the folder
    assert_eq!(names(&dir), vec![FILE_NAME.to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_rename_keeps_the_old_file_and_leaves_no_temporary_file() {
    let dir = scratch("atomic");
    let folder = dir.to_str().unwrap();
    save(folder, json!({"settings": {"v": "old"}}), by("A", "2026-11-02T10:00:00+01:00")).unwrap();
    let broken: RenameFn = &|_, _| Err(std::io::Error::other("no space"));
    let err = save_with(folder, json!({"settings": {"v": "new"}}), by("A", "2026-11-02T11:00:00+01:00"), broken).unwrap_err();
    assert!(err.contains("no space"), "{err}");
    assert_eq!(names(&dir), vec![FILE_NAME.to_string()]);
    assert!(read(folder).unwrap().text.contains("old"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_without_saved_by_or_with_a_damaged_one_is_still_read() {
    let dir = scratch("nosaved");
    let folder = dir.to_str().unwrap();
    for text in [
        r#"{"kind": "lightcraft-settings", "version": 1}"#,
        r#"{"kind": "lightcraft-settings", "version": 1, "savedBy": "me"}"#,
        r#"{"kind": "lightcraft-settings", "version": 1, "savedBy": {"computer": 5, "at": []}}"#,
        r#"{"kind": "lightcraft-settings", "version": 1, "savedBy": {"computer": "  ", "at": "x"}}"#,
        r#"{"kind": "lightcraft-settings", "version": 1, "savedBy": {"at": "2026-11-02T10:00:00Z"}}"#,
    ] {
        std::fs::write(dir.join(FILE_NAME), text).unwrap();
        let file = read(folder).unwrap();
        assert_eq!(file.by, None, "{text}");
        assert_eq!(file.text, text);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saved_by_keeps_a_time_it_cannot_read_as_written_and_formats_a_good_one() {
    let odd = by("Mac", "last Tuesday");
    assert_eq!(odd.when(), None);
    assert_eq!(odd.when_text(), "last Tuesday");
    // Z is accepted
    let z = by("Mac", "2026-11-02T13:30:05Z");
    assert!(z.when().is_some());
    assert!(z.when_text().contains("2026"), "{}", z.when_text());
}

#[test]
fn problems_with_the_folder_say_what_to_do() {
    assert!(check_folder("  ").unwrap_err().contains("No sync folder"));
    let dir = scratch("problems");
    let missing = dir.join("gone");
    assert!(check_folder(missing.to_str().unwrap()).unwrap_err().contains("Choose the sync folder again"));
    let plain = dir.join("file.txt");
    std::fs::write(&plain, "x").unwrap();
    assert!(check_folder(plain.to_str().unwrap()).unwrap_err().contains("not a folder"));
    // an empty folder has no file yet
    let empty = dir.join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert!(read(empty.to_str().unwrap()).unwrap_err().contains("There is no lightcraft.lcsettings"));
    // iCloud's stand-in for a file it has not downloaded
    std::fs::write(empty.join(format!(".{FILE_NAME}.icloud")), "").unwrap();
    assert!(read(empty.to_str().unwrap()).unwrap_err().contains("iCloud hasn't downloaded"));
    // not text
    let binary = dir.join("binary");
    std::fs::create_dir(&binary).unwrap();
    std::fs::write(binary.join(FILE_NAME), [0xff, 0xfe, 0x00, 0x9f]).unwrap();
    assert!(read(binary.to_str().unwrap()).unwrap_err().contains("isn't text"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_this_apps_file_is_read_or_written() {
    let dir = scratch("other-app");
    let other = dir.join("photocraft.pcsettings");
    std::fs::write(&other, "photocraft's own").unwrap();
    assert!(read(dir.to_str().unwrap()).is_err());
    save(dir.to_str().unwrap(), json!({}), by("A", "2026-11-02T10:00:00Z")).unwrap();
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "photocraft's own");
    assert_eq!(names(&dir), vec![FILE_NAME.to_string(), "photocraft.pcsettings".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn newer_from_another_mac_is_offered_and_the_rest_is_not() {
    let file = |c: &str, at: &str| by(c, at);
    let last = "2026-11-02T10:00:00+01:00";
    // another Mac, newer than this Mac's last save or load: offered
    assert!(is_newer_from_another_mac(&file("Laptop", "2026-11-02T10:00:01+01:00"), "Studio", last));
    // the same instant written in another offset is not newer
    assert!(!is_newer_from_another_mac(&file("Laptop", "2026-11-02T09:00:00Z"), "Studio", last));
    // older, or this Mac's own file: not offered
    assert!(!is_newer_from_another_mac(&file("Laptop", "2026-11-01T10:00:00+01:00"), "Studio", last));
    assert!(!is_newer_from_another_mac(&file("Studio", "2026-12-01T10:00:00+01:00"), "Studio", last));
    // nothing saved or loaded yet (or an unreadable record): any file from another Mac
    assert!(is_newer_from_another_mac(&file("Laptop", "2026-11-02T10:00:00+01:00"), "Studio", ""));
    assert!(is_newer_from_another_mac(&file("Laptop", "2026-11-02T10:00:00+01:00"), "Studio", "garbage"));
    // a file whose time can't be read is never offered
    assert!(!is_newer_from_another_mac(&file("Laptop", "yesterday"), "Studio", ""));
}

#[test]
fn newer_file_reads_the_folder_for_keep_in_sync() {
    let dir = scratch("newer");
    let folder = dir.to_str().unwrap();
    // no folder chosen, a folder with no file: nothing to offer
    assert_eq!(newer_file("", "Studio", "").unwrap(), None);
    assert_eq!(newer_file(folder, "Studio", "").unwrap(), None);
    save(folder, json!({}), by("Laptop", "2026-11-02T12:00:00+01:00")).unwrap();
    assert!(newer_file(folder, "Studio", "2026-11-02T10:00:00+01:00").unwrap().is_some());
    assert_eq!(newer_file(folder, "Studio", "2026-11-02T12:00:00+01:00").unwrap(), None);
    assert_eq!(newer_file(folder, "Laptop", "").unwrap(), None);
    // a folder that is gone is an error the caller can show
    assert!(newer_file(dir.join("gone").to_str().unwrap(), "Studio", "").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shared_file_is_written_with_the_folder_and_keeps_other_keys() {
    let dir = scratch("shared");
    let file = shared_file_in(&dir);
    assert!(file.ends_with("Library/Application Support/ArtCraft/sync-folder.json"));
    assert_eq!(read_shared(&file), None);
    write_shared(&file, "/Users/me/iCloud/LightCraft").unwrap();
    assert_eq!(read_shared(&file).as_deref(), Some("/Users/me/iCloud/LightCraft"));
    // another app's key stays; the path is replaced
    std::fs::write(&file, r#"{"path": "/old", "photocraftNote": [1, 2]}"#).unwrap();
    write_shared(&file, "/new").unwrap();
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(doc, json!({"path": "/new", "photocraftNote": [1, 2]}));
    // no temporary file is left in the folder
    assert_eq!(names(file.parent().unwrap()), vec![SHARED_NAME.to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_bad_or_relative_shared_path_offers_nothing_and_a_damaged_file_is_replaced() {
    let dir = scratch("shared-bad");
    let file = dir.join("sync-folder.json");
    for text in ["", "not json", "[]", r#"{"path": 5}"#, r#"{"path": ""}"#, r#"{"path": "relative/folder"}"#, r#"{"other": "/abs"}"#] {
        std::fs::write(&file, text).unwrap();
        assert_eq!(read_shared(&file), None, "{text:?}");
    }
    std::fs::write(&file, "{ broken").unwrap();
    write_shared(&file, "/abs/folder").unwrap();
    assert_eq!(read_shared(&file).as_deref(), Some("/abs/folder"));
    // a path with other keys around it is read, the extras ignored
    std::fs::write(&file, r#"{"extra": true, "path": "/abs/two"}"#).unwrap();
    assert_eq!(read_shared(&file).as_deref(), Some("/abs/two"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_computer_name_is_never_empty() {
    assert!(!computer_name().trim().is_empty());
    assert_eq!(SyncEnv { computer: Some("Test Mac".into()), ..Default::default() }.computer(), "Test Mac");
    // RFC 3339 to the second with the Mac's offset
    assert!(DateTime::parse_from_rfc3339(&now_stamp()).is_ok());
}
