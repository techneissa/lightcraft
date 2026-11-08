use super::*;

/// A fresh folder of its own under the system's temporary one; nothing real is ever touched.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-settings-folder-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn by(computer: &str, at: &str) -> SavedBy {
    SavedBy { computer: computer.into(), at: at.into(), id: None }
}

fn with_id(mut by: SavedBy, id: &str) -> SavedBy {
    by.id = Some(id.into());
    by
}

fn me(computer: &str, id: &str) -> Me {
    Me { computer: computer.into(), id: id.into() }
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
    let saved = save(dir.to_str().unwrap(), json!({"kind": "lightcraft-settings", "version": 1, "settings": {"a": 1}}), stamp.clone()).unwrap();
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
fn a_file_stands_to_a_mac_by_who_saved_it_and_when() {
    let last = "2026-11-02T10:00:00+01:00";
    let studio = me("Studio", "");
    // another Mac, newer than this Mac's last save or load
    assert_eq!(standing(Some(&by("Laptop", "2026-11-02T10:00:01+01:00")), &studio, last), Standing::Newer);
    // the same instant written in another offset is not newer
    assert_eq!(standing(Some(&by("Laptop", "2026-11-02T09:00:00Z")), &studio, last), Standing::Older);
    assert_eq!(standing(Some(&by("Laptop", "2026-11-01T10:00:00+01:00")), &studio, last), Standing::Older);
    // this Mac's own file, whatever its time
    assert_eq!(standing(Some(&by("Studio", "2026-12-01T10:00:00+01:00")), &studio, last), Standing::Own);
    assert_eq!(standing(Some(&by("Studio", "yesterday")), &studio, last), Standing::Own);
    // nothing saved or loaded yet (or an unreadable record): any file from another Mac is newer
    assert_eq!(standing(Some(&by("Laptop", "2026-11-02T10:00:00+01:00")), &studio, ""), Standing::Newer);
    assert_eq!(standing(Some(&by("Laptop", "2026-11-02T10:00:00+01:00")), &studio, "garbage"), Standing::Newer);
    // the saver or the time can't be read: not told apart from a newer one
    assert_eq!(standing(None, &studio, last), Standing::Unknown);
    assert_eq!(standing(Some(&by("Laptop", "yesterday")), &studio, last), Standing::Unknown);
    assert!(Standing::Unknown.would_lose_settings() && Standing::Newer.would_lose_settings());
    assert!(!Standing::Own.would_lose_settings() && !Standing::Older.would_lose_settings());
}

#[test]
fn two_macs_with_one_name_are_told_apart_by_their_ids() {
    let last = "2026-11-02T10:00:00+01:00";
    let newer = "2026-11-02T12:00:00+01:00";
    // both have an id: it decides, whatever the names say
    assert_eq!(standing(Some(&with_id(by("Mac", newer), "aaa")), &me("Mac", "bbb"), last), Standing::Newer);
    assert_eq!(standing(Some(&with_id(by("Mac", newer), "aaa")), &me("Other", "aaa"), last), Standing::Own);
    // the file has none (PhotoCraft's or an older file), or this Mac has none yet: by name
    assert_eq!(standing(Some(&by("Mac", newer)), &me("Mac", "bbb"), last), Standing::Own);
    assert_eq!(standing(Some(&with_id(by("Mac", newer), "aaa")), &me("Mac", ""), last), Standing::Own);
    assert_eq!(standing(Some(&by("Mac", newer)), &me("Other", "bbb"), last), Standing::Newer);
}

#[test]
fn saved_by_id_is_written_read_and_optional() {
    let dir = scratch("id");
    let folder = dir.to_str().unwrap();
    let id = new_mac_id();
    assert_eq!(id.len(), 32);
    assert_ne!(id, new_mac_id(), "random");
    let by = SyncEnv { computer: Some("Studio".into()), ..Default::default() }.saved_by(&id);
    assert_eq!(by.id.as_deref(), Some(id.as_str()));
    let saved = save(folder, json!({}), by.clone()).unwrap();
    assert_eq!(read(folder).unwrap().by, Some(by));
    assert!(std::fs::read_to_string(saved.path).unwrap().contains("\"id\""));
    // no id made yet: the key is left out, and a file without one reads fine
    let none = SyncEnv::default().saved_by("");
    assert_eq!(none.id, None);
    save(folder, json!({}), SavedBy { computer: "Studio".into(), ..none }).unwrap();
    assert!(!std::fs::read_to_string(dir.join(FILE_NAME)).unwrap().contains("\"id\""));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inspect_reads_the_folder_for_keep_in_sync_and_save() {
    let dir = scratch("newer");
    let folder = dir.to_str().unwrap();
    let studio = me("Studio", "");
    // no folder chosen, a folder with no file: nothing there
    assert_eq!(inspect("", &studio, "").unwrap(), None);
    assert_eq!(inspect(folder, &studio, "").unwrap(), None);
    save(folder, json!({}), by("Laptop", "2026-11-02T12:00:00+01:00")).unwrap();
    assert_eq!(inspect(folder, &studio, "2026-11-02T10:00:00+01:00").unwrap().map(|(_, s)| s), Some(Standing::Newer));
    assert!(newer_file(folder, &studio, "2026-11-02T10:00:00+01:00").unwrap().is_some());
    assert_eq!(inspect(folder, &studio, "2026-11-02T12:00:00+01:00").unwrap().map(|(_, s)| s), Some(Standing::Older));
    assert_eq!(newer_file(folder, &studio, "2026-11-02T12:00:00+01:00").unwrap(), None);
    assert_eq!(newer_file(folder, &me("Laptop", ""), "").unwrap(), None);
    // a file that doesn't say who saved it stops a save, but is never offered at start
    std::fs::write(dir.join(FILE_NAME), "{}").unwrap();
    assert_eq!(inspect(folder, &studio, "").unwrap().map(|(_, s)| s), Some(Standing::Unknown));
    assert_eq!(newer_file(folder, &studio, "").unwrap(), None);
    // a folder that is gone is an error the caller can show
    assert!(inspect(dir.join("gone").to_str().unwrap(), &studio, "").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn leftover_temporary_files_of_this_app_go_when_old_and_nothing_else_does() {
    let dir = scratch("leftovers");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let put = |name: &str, aged: bool| {
        let path = dir.join(name);
        std::fs::write(&path, "x").unwrap();
        if aged {
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        }
    };
    put(".lightcraft.lcsettings.123-0.tmp", true);
    put(".lightcraft.lcsettings.4-17.tmp", true);
    // too new (another Mac may be saving), another app's, or not our pattern
    put(".lightcraft.lcsettings.999-0.tmp", false);
    put(".photocraft.pcsettings.123-0.tmp", true);
    put(".lightcraft.lcsettings.abc-0.tmp", true);
    put(".lightcraft.lcsettings.123-0.tmp.keep", true);
    put("notes.tmp", true);
    put(FILE_NAME, true);
    assert_eq!(remove_leftovers(&dir), 2);
    let left = names(&dir);
    assert_eq!(left.len(), 6, "{left:?}");
    assert!(left.contains(&FILE_NAME.to_string()) && left.contains(&".lightcraft.lcsettings.999-0.tmp".to_string()));
    assert_eq!(remove_leftovers(&dir.join("gone")), 0);
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
