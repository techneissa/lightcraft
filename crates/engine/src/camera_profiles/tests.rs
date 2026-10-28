use super::*;

fn profile() -> CameraProfile {
    let table = HsvTable { hue_divisions: 4, sat_divisions: 2, val_divisions: 1, data: vec![[5.0, 1.2, 1.0]; 8], srgb_value: false };
    CameraProfile::new("ILCE-7M4", 12, 3456, Mat3([[1.6, -0.5, -0.1], [-0.2, 1.3, -0.1], [-0.1, -0.2, 1.3]]), Some(table))
}

#[test]
fn profiles_round_trip_and_reject_bad_files() {
    let dir = std::env::temp_dir().join(format!("lc-camera-profiles-{}", std::process::id()));
    let path = save(&profile(), &dir).unwrap();
    assert_eq!(path.file_name().unwrap(), "ILCE-7M4.json");
    assert_eq!(load(&path).unwrap(), profile());
    let bad = |p: CameraProfile| {
        let path = dir.join("bad.json");
        std::fs::write(&path, serde_json::to_vec(&p).unwrap()).unwrap();
        load(&path).is_err()
    };
    let mut p = profile();
    p.matrix[0][0] = f64::NAN;
    assert!(bad(p), "non-finite matrix");
    let mut p = profile();
    p.matrix = [[0.0; 3]; 3];
    assert!(bad(p), "singular matrix");
    let mut p = profile();
    if let Some(t) = p.hue_sat.as_mut() {
        t.data.truncate(3);
    }
    assert!(bad(p), "table data doesn't match its shape");
    let mut p = profile();
    p.version = 99;
    assert!(bad(p), "unknown version");
    std::fs::write(dir.join("junk.json"), b"{not json").unwrap();
    assert!(load(&dir.join("junk.json")).is_err());
    assert!(load(&dir.join("missing.json")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bundled_profiles_are_valid_and_named_after_their_model() {
    assert!(!BUNDLED.is_empty());
    for (model, json) in BUNDLED {
        let p = parse(json.as_bytes(), model).unwrap();
        assert_eq!(p.model, *model);
        assert!(p.files >= 5 && p.hue_sat.is_some(), "{model}: {} photos", p.files);
        assert_eq!(bundled(model), Some(p));
    }
    assert!(bundled("No Such Camera").is_none());
}

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-local-profiles-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn put(dir: &Path, file: &str, p: &CameraProfile) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, serde_json::to_vec(p).unwrap()).unwrap();
    path
}

fn for_model(model: &str, name: Option<&str>) -> CameraProfile {
    let mut p = profile();
    p.model = model.into();
    p.name = name.map(Into::into);
    p
}

/// LT2: every valid `*.json` directly in the folder is a local profile, named from its `name` (else the file
/// name), found for the cameras it is made for; damaged files are listed as unusable and never offered.
#[test]
fn local_profiles_are_named_listed_and_matched_by_camera() {
    let dir = temp("list");
    put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("  A7 V Chart  ")));
    put(&dir, "plain.json", &for_model("ILCE-7M5", None));
    put(&dir, "blank name.json", &for_model("ILCE-7M5", Some("   ")));
    put(&dir, "long.json", &for_model("ILCE-7M4", Some(&"x".repeat(200))));
    put(&dir, "UPPER.JSON", &for_model("ILCE-7M4", Some("Upper Case Extension")));
    std::fs::write(dir.join("junk.json"), b"{not json").unwrap();
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
    std::fs::write(dir.join(".hidden.json"), serde_json::to_vec(&for_model("ILCE-7M5", None)).unwrap()).unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    put(&dir.join("sub"), "nested.json", &for_model("ILCE-7M5", None));
    std::fs::create_dir_all(dir.join("folder.json")).unwrap();
    let _dir = use_dir_for_tests(&dir);
    let all = local_profiles();
    let listed: Vec<_> = all.iter().map(|p| (p.id.as_str(), p.name.chars().count(), p.valid())).collect();
    assert_eq!(
        listed,
        [("A7 V Chart", 10, true), ("blank name", 10, true), ("junk", 4, false), ("plain", 5, true), ("UPPER", 20, true), ("long", MAX_NAME, true)],
        "sorted by display name; the stem is the id; a long name is cut; junk, notes, hidden files, folders and sub-folders are not profiles"
    );
    let names = |model: &str| local_profiles_for(model).iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    assert_eq!(names("ILCE-7M5"), ["A7 V Chart", "blank name", "plain"]);
    assert_eq!(names("  ILCE-7M5 "), ["A7 V Chart", "blank name", "plain"], "the camera model is compared trimmed");
    assert_eq!(names("ILCE-7M4").len(), 2);
    assert!(names("ILCE-7CR").is_empty() && names("").is_empty());
    let by_camera = |camera: &str| local_profiles_for_camera(camera).len();
    assert_eq!((by_camera("SONY ILCE-7M5"), by_camera("ILCE-7M5"), by_camera("SONY ILCE-7M4"), by_camera("SONY XILCE-7M5")), (3, 3, 2, 0));
    let junk = local_profile("junk").unwrap();
    assert!(!junk.valid() && !junk.is_for("ILCE-7M5"));
    assert!(local_profile("nothing").is_none() && local_profile("../A7 V Chart").is_none() && local_profile("sub/nested").is_none());
    drop(_dir);
    let _empty = use_dir_for_tests(&dir.join("does-not-exist"));
    assert!(local_profiles().is_empty(), "no folder, no profiles");
    let _ = std::fs::remove_dir_all(&dir);
}

/// LT3: Camera Matching reads only the built-in profiles: a local `ILCE-7M4.json` does not replace the built-in
/// one, and a local file for a camera that has none gives Camera Matching nothing.
#[test]
fn camera_matching_reads_only_the_built_in_profiles() {
    let dir = temp("get");
    let mut mine = for_model("ILCE-7M4", None);
    mine.files = 999;
    put(&dir, "ILCE-7M4.json", &mine);
    put(&dir, "ILCE-7M5.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
    let _dir = use_dir_for_tests(&dir);
    let used = get("ILCE-7M4").expect("a built-in profile for the model");
    assert_eq!(used.files, bundled("ILCE-7M4").unwrap().files);
    assert_ne!(used.files, 999, "the local file did not replace it");
    assert!(get("ILCE-7M5").is_none(), "a local profile of a camera without a built-in one is no input of Camera Matching");
    assert!(local_profiles_for("ILCE-7M5").iter().any(|p| p.name == "A7 V Chart"), "it is a local profile instead");
    let _ = std::fs::remove_dir_all(&dir);
}

/// LT4 (profiles half): the render key every photo carries is the built-in profiles' alone, the same value as
/// before local profiles were choices, whatever the user's folder holds.
#[test]
fn the_render_key_of_camera_matching_ignores_the_users_folder() {
    let formula = || {
        // (the key as it was for a user whose folder was empty)
        let mut h = lightcraft_preview::Hasher128::new();
        for (model, json) in BUNDLED {
            h.str(model).str(json);
        }
        h.finish().0 as u64
    };
    let dir = temp("key");
    let _dir = use_dir_for_tests(&dir);
    // (`bundled_key` is what `cache_key` remembers: computed afresh here, so a key that read the folder would differ)
    let before = bundled_key();
    put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
    put(&dir, "ILCE-7M4.json", &for_model("ILCE-7M4", None));
    assert_eq!(bundled_key(), before, "adding local profiles does not move the key");
    assert_eq!(before, formula());
    assert_eq!(cache_key(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// LT4 / LT8 (profiles half): a local profile's key follows its file's content, also for a same-second,
/// same-length replacement, and a removed file has a key of its own; restoring the file restores the key. The
/// profile is found again from the key, and says why it cannot be used.
#[test]
fn a_local_profile_key_follows_the_file_and_resolves_to_the_profile() {
    let dir = temp("local-key");
    let _dir = use_dir_for_tests(&dir);
    let path = put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
    let stamp = |millis| {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000) + std::time::Duration::from_millis(millis);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
    };
    stamp(100);
    let first = local_key("A7 V Chart");
    assert_eq!(local_key("A7 V Chart"), first, "the same file, the same key");
    assert_ne!(first, local_key("Another"), "the id is part of it");
    assert!(resolve_local(first, "ILCE-7M5").is_ok());
    // a replacement of the same length within the same second
    let mut second = for_model("ILCE-7M5", Some("A7 V Chart"));
    second.files = 13;
    let bytes = serde_json::to_vec(&second).unwrap();
    assert_eq!(bytes.len(), serde_json::to_vec(&for_model("ILCE-7M5", Some("A7 V Chart"))).unwrap().len(), "same length");
    std::fs::write(&path, bytes).unwrap();
    stamp(350);
    let replaced = local_key("A7 V Chart");
    assert_ne!(replaced, first);
    assert_eq!(resolve_local(replaced, "ILCE-7M5").unwrap().1.files, 13);
    // removed: a key of its own, and the reason
    let original = serde_json::to_vec(&for_model("ILCE-7M5", Some("A7 V Chart"))).unwrap();
    std::fs::remove_file(&path).unwrap();
    let missing = local_key("A7 V Chart");
    assert!(missing != first && missing != replaced);
    assert_eq!(resolve_local(missing, "ILCE-7M5").unwrap_err(), ("A7 V Chart".to_owned(), LocalUnavailable::Missing));
    // restored
    std::fs::write(&path, original).unwrap();
    stamp(100);
    assert_eq!(local_key("A7 V Chart"), first, "the file is back: the profile is used again");
    // other cameras, unusable files, hostile ids, unknown keys
    assert_eq!(resolve_local(first, "ILCE-7M4").unwrap_err().1, LocalUnavailable::OtherCamera);
    std::fs::write(dir.join("broken.json"), b"[]").unwrap();
    assert_eq!(resolve_local(local_key("broken"), "ILCE-7M5").unwrap_err().1, LocalUnavailable::Invalid);
    assert_eq!(resolve_local(local_key("../A7 V Chart"), "ILCE-7M5").unwrap_err().1, LocalUnavailable::Missing);
    assert_eq!(resolve_local(0x5eed, "ILCE-7M5").unwrap_err().1, LocalUnavailable::Missing);
    assert!(LocalUnavailable::Missing.note("A7 V Chart").contains("isn't available on this computer"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `calibrate --name`: the file is `<name>.json` with the name inside, path characters in it are made harmless,
/// and it appears as a local profile of that name.
#[test]
fn a_named_profile_is_saved_as_a_local_profile() {
    let dir = temp("named");
    let _dir = use_dir_for_tests(&dir);
    let path = save_named(&for_model("ILCE-7M5", None), &dir, "A7 V Chart").unwrap();
    assert_eq!(path.file_name().unwrap(), "A7 V Chart.json");
    assert_eq!(load(&path).unwrap().name.as_deref(), Some("A7 V Chart"));
    let hostile = save_named(&for_model("ILCE-7M5", None), &dir, "../../evil/name").unwrap();
    assert_eq!(hostile.parent().unwrap(), dir, "stays in the folder");
    assert!(save_named(&for_model("ILCE-7M5", None), &dir, "   ").is_err());
    assert!(save_named(&for_model("ILCE-7M5", None), &dir, "...").is_err());
    let ids: Vec<_> = local_profiles_for("ILCE-7M5").iter().map(|p| p.name.clone()).collect();
    assert!(ids.contains(&"A7 V Chart".to_owned()) && ids.len() == 2, "{ids:?}");
    let plain = save(&for_model("ILCE-7M5", None), &dir).unwrap();
    assert_eq!(plain.file_name().unwrap(), "ILCE-7M5.json", "without a name the file is the model's, as before");
    assert!(!std::fs::read_to_string(&plain).unwrap().contains("\"name\""), "no name field for an unnamed profile");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_names_are_safe() {
    assert_eq!(file_name("ILCE-7M4"), "ILCE-7M4.json");
    assert_eq!(file_name(" DSC-RX100M3 "), "DSC-RX100M3.json");
    assert_eq!(file_name("../../etc/passwd"), "______etc_passwd.json");
    assert_eq!(file_name("Ω 1/2"), "__1_2.json");
}

#[test]
fn pool_skips_files_that_cannot_contribute() {
    let mut pool = Pool::default();
    assert!(pool.add(b"not a raw file").is_err());
    assert!(pool.fit(1).is_empty());
    assert!(pool.files().is_empty());
}

/// A replacement with the same length and the same modification time (a copy that keeps its time, a coarse clock)
/// still gives the profile a new key: the content is read and hashed, not only the stamp.
#[test]
fn a_replacement_with_the_same_length_and_time_is_seen() {
    let dir = temp("same-stamp");
    let _dir = use_dir_for_tests(&dir);
    let path = put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
    let stamp = || {
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(time)).unwrap();
    };
    stamp();
    let first = local_key("A7 V Chart");
    let mut second = for_model("ILCE-7M5", Some("A7 V Chart"));
    second.files = 13;
    let bytes = serde_json::to_vec(&second).unwrap();
    assert_eq!(bytes.len(), serde_json::to_vec(&for_model("ILCE-7M5", Some("A7 V Chart"))).unwrap().len(), "same length");
    std::fs::write(&path, bytes).unwrap();
    stamp();
    let replaced = local_key("A7 V Chart");
    assert_ne!(replaced, first, "a new key for the new content");
    assert_eq!(resolve_local(replaced, "ILCE-7M5").unwrap().1.files, 13, "and the new profile");
    // the picker's listing (no hashing) catches up once the stamp moves
    let _ = std::fs::remove_dir_all(&dir);
}

/// The registry never drops a key that is in use: many profiles registered later leave the first resolvable, and a
/// registry that lost a key (as after a restart of the process) finds the profile again from the folder.
#[test]
fn a_key_is_never_lost_to_other_profiles_and_is_found_again_from_the_folder() {
    let dir = temp("registry");
    let _dir = use_dir_for_tests(&dir);
    put(&dir, "A7 V Chart.json", &for_model("ILCE-7M5", Some("A7 V Chart")));
    let first = local_key("A7 V Chart");
    let gone = local_key("Gone");
    for i in 0..6000 {
        local_key(&format!("junk {i}"));
    }
    assert!(resolve_local(first, "ILCE-7M5").is_ok(), "a later registration did not push it out");
    assert_eq!(resolve_local(gone, "ILCE-7M5").unwrap_err(), ("Gone".to_owned(), LocalUnavailable::Missing), "with its id");
    forget_registry();
    assert_eq!(resolve_local(first, "ILCE-7M5").unwrap().0, "A7 V Chart", "found again from the folder");
    // an id keeps its newest versions; the oldest of many are the ones that go
    let mut keys = Vec::new();
    for i in 0..=KEYS_PER_ID {
        let mut p = for_model("ILCE-7M5", Some("A7 V Chart"));
        p.files = 100 + i;
        put(&dir, "A7 V Chart.json", &p);
        keys.push(local_key("A7 V Chart"));
    }
    assert!(keys.windows(2).all(|w| w[0] != w[1]), "each version has its own key");
    assert!(resolve_local(keys[KEYS_PER_ID], "ILCE-7M5").is_ok(), "the newest is held");
    let _ = std::fs::remove_dir_all(&dir);
}
