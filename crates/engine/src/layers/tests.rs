use super::*;

/// The demo library, its first `n` photos by id (`LC01200.JPG`…), and a selection of the photos at
/// `selected` (positions in that list) with the one at `active` active.
fn session(n: usize, selected: &[usize], active: usize) -> (Session, Vec<PhotoId>) {
    let mut s = Session::with_demo();
    let mut ids: Vec<PhotoId> = s.catalog.photos().map(|p| p.id).collect();
    ids.sort();
    ids.truncate(n);
    assert_eq!(ids.len(), n, "the demo has {n} photos");
    s.selection = crate::Selection { active: ids.get(active).copied(), ids: selected.iter().map(|i| ids[*i]).collect() };
    (s, ids)
}

/// A photo's name without its extension.
fn stem(s: &Session, id: PhotoId) -> String {
    let name = s.catalog.photo(id).map(|p| p.file_name.clone()).unwrap_or_default();
    Path::new(&name).file_stem().unwrap().to_string_lossy().to_string()
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-layers-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn pcstack_json_is_the_version_1_layout() {
    let files = vec!["/a/top.psd".to_string(), "/a/middle.psd".to_string(), "/a/bottom.psd".to_string()];
    let v: Value = serde_json::from_str(&pcstack_json("DSC01234 + 2", &files).unwrap()).unwrap();
    assert_eq!(v, json!({"kind": "photocraft-open-as-layers", "version": 1, "name": "DSC01234 + 2", "files": files}));
    assert_eq!(v["files"][0], "/a/top.psd", "top layer first");
}

#[test]
fn pcstack_json_refuses_what_photocraft_would() {
    assert!(pcstack_json("x", &[]).is_err());
    assert!(pcstack_json("x", &["relative/a.tif".into()]).unwrap_err().contains("absolute"));
    let many: Vec<String> = (0..=MAX_LAYERS).map(|i| format!("/a/{i}.tif")).collect();
    assert!(pcstack_json("x", &many).unwrap_err().contains("256"));
    assert!(pcstack_json("x", &many[..MAX_LAYERS]).is_ok(), "256 is the most");
}

#[test]
fn the_document_is_named_after_the_top_photo() {
    assert_eq!(doc_name("DSC01234.ARW", 3), "DSC01234 + 2");
    assert_eq!(doc_name("DSC01234.ARW", 2), "DSC01234 + 1");
    assert_eq!(doc_name("DSC01234.ARW", 1), "DSC01234");
    assert_eq!(doc_name("", 2), "Layers + 1");
}

#[test]
fn the_plist_check_wants_pcstack_in_the_document_types() {
    let plist = |types: &str, other: &str| {
        format!(
            "<?xml version=\"1.0\"?>\n<plist version=\"1.0\"><dict>\n<key>CFBundleName</key><string>PhotoCraft</string>\n\
             <key>CFBundleDocumentTypes</key>\n<array>{types}</array>\n{other}</dict></plist>"
        )
    };
    let dict = |exts: &[&str]| {
        let strings: String = exts.iter().map(|e| format!("<string>{e}</string>")).collect();
        format!("<dict><key>CFBundleTypeName</key><string>Image</string><key>CFBundleTypeExtensions</key><array>{strings}</array></dict>")
    };
    assert!(plist_lists_pcstack(&plist(&dict(&["psd", "tif"]).repeat(2).replace("tif", "pcstack"), "")), "nested array found");
    assert!(plist_lists_pcstack(&plist(&format!("{}{}", dict(&["psd"]), dict(&["PCStack"])), "")), "any case, any entry");
    assert!(!plist_lists_pcstack(&plist(&dict(&["psd", "tif", "jpg"]), "")), "the official PhotoCraft");
    assert!(!plist_lists_pcstack(&plist("", "<key>Other</key><array><string>pcstack</string></array>")), "only the document types count");
    assert!(!plist_lists_pcstack("<plist><dict><key>CFBundleName</key><string>pcstack</string></dict></plist>"), "no document types at all");
    assert!(!plist_lists_pcstack(""));
}

#[test]
fn the_active_photo_goes_on_top_and_the_rest_keep_their_selection_order() {
    let (s, id) = session(5, &[3, 1, 4, 2], 4);
    assert_eq!(ordered_targets(&s, &json!({})).unwrap(), [id[4], id[3], id[1], id[2]]);
    // explicit ids are taken as given (first = top), repeats dropped
    let given = json!({"ids": [id[1].0, id[2].0, id[1].0, id[0].0]});
    assert_eq!(ordered_targets(&s, &given).unwrap(), [id[1], id[2], id[0]]);
    // an active photo outside the selection doesn't join it
    let (s, id) = session(3, &[0, 1], 2);
    assert_eq!(ordered_targets(&s, &json!({})).unwrap(), [id[0], id[1]]);
}

#[test]
fn two_to_256_photos_are_enough_and_fewer_or_more_are_refused_before_rendering() {
    assert!(check_count(0).is_err() && check_count(1).unwrap_err().contains("at least two"));
    assert!(check_count(2).is_ok() && check_count(256).is_ok());
    let e = check_count(257).unwrap_err();
    assert!(e.contains("257") && e.contains("256"), "{e}");
    let (mut s, id) = session(2, &[0, 1], 0);
    let many: Vec<u64> = (1..=300).collect();
    let e = prepare(&mut s, &json!({"ids": many, "dir": "/tmp"})).err().unwrap();
    assert!(e.contains("300"), "{e}");
    let e = prepare(&mut s, &json!({"ids": [id[0].0], "dir": "/tmp"})).err().unwrap();
    assert!(e.contains("at least two"), "{e}");
    let e = prepare(&mut s, &json!({})).err().unwrap();
    assert!(e.contains("give `dir`"), "a generated photo has no folder: {e}");
}

#[test]
fn layers_are_rendered_named_after_their_photos_and_listed_top_first() {
    let dir = temp_dir("run");
    let (mut s, id) = session(3, &[0, 1, 2], 1);
    let library = s.catalog.len();
    let doc = format!("{} + 2", stem(&s, id[1]));
    let plan = prepare(&mut s, &json!({"dir": dir.to_string_lossy(), "format": "tiff", "bitDepth": 8})).unwrap();
    assert_eq!((plan.name.as_str(), plan.total()), (doc.as_str(), 3));
    assert_eq!(plan.ids, [id[1], id[0], id[2]]);
    assert!(plan.dir.ends_with(&format!("{doc} Layers")), "{}", plan.dir);
    let mut seen = Vec::new();
    let layers = plan
        .run(true, &mut crate::export::write_file_durable, &mut |done, name| {
            seen.push((done, name.to_string()));
            true
        })
        .unwrap();
    assert_eq!(seen.iter().map(|s| s.0).collect::<Vec<_>>(), [0, 1, 2]);
    let names: Vec<String> = layers.files.iter().map(|f| Path::new(f).file_name().unwrap().to_string_lossy().to_string()).collect();
    let want: Vec<String> = [id[1], id[0], id[2]].iter().map(|i| format!("{}.tif", stem(&s, *i))).collect();
    assert_eq!(names, want, "named after the originals: PhotoCraft's layer names");
    assert!(layers.files.iter().all(|f| Path::new(f).is_absolute() && Path::new(f).is_file()));
    let stack = layers.stack.unwrap();
    assert!(stack.ends_with(&format!("{doc}.pcstack")) && Path::new(&stack).parent() == Path::new(&layers.dir).into());
    let v: Value = serde_json::from_slice(&std::fs::read(&stack).unwrap()).unwrap();
    assert_eq!((v["kind"].clone(), v["version"].clone(), v["name"].clone()), (json!(KIND), json!(1), json!(doc)));
    assert_eq!(v["files"], json!(layers.files));
    assert_eq!(s.catalog.len(), library, "the layer files are not added to the library");
    // a second run beside the first takes a new folder and never touches the first
    let again = prepare(&mut s, &json!({"dir": dir.to_string_lossy()})).unwrap();
    assert!(again.dir.ends_with(&format!("{doc} Layers 2")), "{}", again.dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_stack_only_the_layer_files_are_written() {
    let dir = temp_dir("nostack");
    let (mut s, _) = session(2, &[0, 1], 0);
    let layers = prepare(&mut s, &json!({"dir": dir.to_string_lossy()}))
        .unwrap()
        .run(false, &mut crate::export::write_file_durable, &mut |_, _| true)
        .unwrap();
    assert!(layers.stack.is_none());
    assert_eq!(std::fs::read_dir(&layers.dir).unwrap().count(), 2, "no .pcstack");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cancel or a write that fails leaves nothing behind: a document missing a layer would go
/// unnoticed.
#[test]
fn a_cancel_or_a_write_error_removes_what_was_written() {
    let dir = temp_dir("fail");
    let (mut s, _) = session(3, &[0, 1, 2], 0);
    let plan = prepare(&mut s, &json!({"dir": dir.to_string_lossy()})).unwrap();
    let layer_dir = PathBuf::from(&plan.dir);
    let e = plan.run(true, &mut crate::export::write_file_durable, &mut |done, _| done < 1).unwrap_err();
    assert!(e.contains("cancelled"), "{e}");
    assert!(!layer_dir.exists(), "the folder goes with its files");
    let plan = prepare(&mut s, &json!({"dir": dir.to_string_lossy()})).unwrap();
    let mut n = 0;
    let mut flaky = |path: &str, bytes: &[u8]| {
        n += 1;
        if n == 2 { Err("the disk is full".to_string()) } else { crate::export::write_file_durable(path, bytes) }
    };
    let e = plan.run(true, &mut flaky, &mut |_, _| true).unwrap_err();
    assert!(e.contains("the disk is full"), "{e}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the first layer file and its folder are removed");
    let _ = std::fs::remove_dir_all(&dir);
}
