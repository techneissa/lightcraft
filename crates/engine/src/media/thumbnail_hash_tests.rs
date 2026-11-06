use super::*;

fn session() -> crate::Session {
    let mut s = crate::Session::new();
    let p = Photo::new(PhotoId(1), Source::File { path: "hash-fixture.jpg".into() }, "fixture.jpg", "JPEG", 6000, 4000, "2026-01-01");
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    s
}
/// Settings saved by a newer LightCraft (a process number this build doesn't know) render with
/// this build's latest process, and a build that resolved the number to an older process
/// cached other pixels under the same settings hash: the persistent keys (thumbnails, the
/// loupe's view render, variants) name the process the settings render with, so those are
/// never served (review of 5a7016c8: a catalog saved with a newer process kept main's V2 thumbnails).
#[test]
fn persistent_keys_name_the_process_the_settings_render_with() {
    use lightcraft_develop::ProcessVersion;
    let mut s = session();
    let newer = DevelopSettings { process: ProcessVersion(ProcessVersion::LATEST.0 + 1), ..DevelopSettings::default() };
    s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(newer.clone()), label: "newer".into(), edited: None }).unwrap();
    let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
    let unnamed = |kind: Option<&str>, size: u64| {
        let mut h = Hasher128::new();
        h.str(&content_key(&p));
        if let Some(k) = kind {
            h.str(k);
        }
        h.u64(newer.hash64()).u64(size).u64(RENDER_CACHE_VERSION).u64(crate::camera_profiles::cache_key()).finish()
    };
    let named = |kind: Option<&str>, size: u64, process: u64| {
        let mut h = Hasher128::new();
        h.str(&content_key(&p));
        if let Some(k) = kind {
            h.str(k);
        }
        h.u64(newer.hash64()).u64(size).u64(RENDER_CACHE_VERSION).u64(process).u64(crate::camera_profiles::cache_key()).finish()
    };
    let resolved = u64::from(lightcraft_develop::Process::LATEST.version().0);
    let keys = [
        (s.thumb_job(PhotoId(1), 256).unwrap().cache.unwrap().1, None, 256),
        (crate::Session::view_key(&p, p.develop.hash64(), true, 0), Some("view"), 1),
        (crate::Session::variant_key(&p, &newer, 256, 0), Some("variant"), 256),
    ];
    for (key, kind, size) in keys {
        assert_ne!(key, unnamed(kind, size), "old builds did not name their resolved process");
        assert_eq!(key, named(kind, size, resolved), "the key names the process used for rendering");
        assert_ne!(key, named(kind, size, u64::from(newer.process.0)), "the saved future number is not the resolved process");
    }
}

/// Photos on V1 and V2 keep the persistent keys builds before process V3 gave them (no process
/// in them), so their cached thumbnails, view renders and variants stay valid after upgrading
/// (review of 26cd2df1: every V1/V2 photo rendered again, and offline ones without smart
/// previews lost their thumbnails); V3 and later name their process.
#[test]
fn v1_and_v2_photos_keep_the_render_cache_keys_they_had() {
    use lightcraft_develop::Process;
    let mut s = session();
    for process in [Process::V1, Process::V2, Process::V3] {
        let settings = DevelopSettings { process: process.version(), ..DevelopSettings::default() };
        s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(settings.clone()), label: "p".into(), edited: None }).unwrap();
        let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
        let before_v3 = |kind: Option<&str>, size: u64| {
            let mut h = Hasher128::new();
            h.str(&content_key(&p));
            if let Some(k) = kind {
                h.str(k);
            }
            h.u64(settings.hash64()).u64(size).u64(RENDER_CACHE_VERSION).u64(crate::camera_profiles::cache_key()).finish()
        };
        let keys = [
            s.thumb_job(PhotoId(1), 256).unwrap().cache.unwrap().1 == before_v3(None, 256),
            crate::Session::view_key(&p, p.develop.hash64(), true, 0) == before_v3(Some("view"), 1),
            crate::Session::variant_key(&p, &settings, 256, 0) == before_v3(Some("variant"), 256),
        ];
        let old = process <= Process::V2;
        assert_eq!(keys, [old; 3], "{process:?}: thumbnail, view and variant keys");
    }
}

#[test]
fn hashes_match_uncached_keys_and_are_not_recomputed_for_sizes_metadata_or_before() {
    let mut s = session();
    for edge in [256, 384, 512, 256] {
        let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
        let b = thumb_bucket(edge);
        // Independent original formula: never asks the new hash cache for expected values.
        let settings = p.develop.hash64();
        let content = Hasher128::new().str(&content_key(&p)).finish().0 as u64;
        let expected = settings ^ ((b as u64) << 40) ^ ((b as u64) << 20) ^ 1 ^ p.id.0.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ content.rotate_left(17);
        let disk = Hasher128::new()
            .str(&content_key(&p))
            .u64(settings)
            .u64(b as u64)
            .u64(RENDER_CACHE_VERSION)
            .u64(u64::from(p.develop.process.process().version().0))
            .u64(crate::camera_profiles::cache_key())
            .finish();
        let job = s.thumb_job(p.id, edge).unwrap();
        assert_eq!(job.key, expected);
        assert_eq!(job.cache.unwrap().1, disk);
    }
    s.catalog.apply(Op::SetRating { id: PhotoId(1), rating: 4 }).unwrap();
    s.thumb_job(PhotoId(1), 256).unwrap();
    s.render_job(PhotoId(1), 256, 256, true, true).unwrap();
    s.thumb_job(PhotoId(1), 256).unwrap();
    assert_eq!(s.media.settings_hashes.computations, 1);
    let mut edit = DevelopSettings::default();
    edit.light.exposure = 1.0;
    let undo = s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(edit), label: "edit".into(), edited: None }).unwrap();
    s.thumb_job(PhotoId(1), 256).unwrap();
    s.thumb_job(PhotoId(1), 384).unwrap();
    assert_eq!(s.media.settings_hashes.computations, 2);
    s.catalog.apply(undo).unwrap();
    s.thumb_job(PhotoId(1), 256).unwrap();
    assert_eq!(s.media.settings_hashes.computations, 3);
}
#[test]
fn hash_cache_is_bounded_weak_and_shared_settings_remain_independent() {
    let mut hashes = SettingsHashes::default();
    let shared = Arc::new(DevelopSettings::default());
    for id in 1..=SETTINGS_HASH_ENTRIES as u64 * 3 {
        assert_eq!(hashes.get(PhotoId(id), &shared), shared.hash64());
    }
    assert_eq!(hashes.entries.len(), SETTINGS_HASH_ENTRIES);
    assert_eq!(hashes.order.len(), SETTINGS_HASH_ENTRIES);
    assert_eq!(Arc::strong_count(&shared), 1);
    let mut edit = (*shared).clone();
    edit.light.exposure = 1.0;
    let edit = Arc::new(edit);
    let a = PhotoId(10000);
    let b = PhotoId(10001);
    let initial = hashes.get(a, &shared);
    assert_eq!(hashes.get(b, &shared), initial);
    assert_ne!(hashes.get(a, &edit), initial);
    assert_eq!(hashes.get(b, &shared), initial);
    let mut s = session();
    s.thumb_job(PhotoId(1), 256).unwrap();
    s.media.clear_sources();
    assert!(s.media.settings_hashes.entries.is_empty());
    assert!(s.media.settings_hashes.order.is_empty());
}
#[test]
fn source_from_before_content_reload_is_not_accepted() {
    let mut s = session();
    s.media.file_loader = Some(Arc::new(|_, _, _, _| Ok((Rgb32f::new(8, 8), SourceInfo::default()))));
    let old = s.thumb_job(PhotoId(1), 256).unwrap().run();
    assert!(old.loaded.is_some());
    s.catalog
        .apply(Op::SetContent {
            id: PhotoId(1),
            width: 6000,
            height: 4000,
            file_size: 99,
            content_hash: Some("replacement".into()),
            preview_only: None,
            preview_checked: String::new(),
        })
        .unwrap();
    s.media.forget(PhotoId(1));
    s.accept(&old);
    assert_eq!(s.media.source_usage().0, 0);
}
/// The photo's Camera Matching choice is part of the decode: a source decoded with another choice is decoded again
/// (and a late render of the old one is not taken back into the cache); one the choice doesn't change is reused.
#[test]
fn camera_colour_decodes_again_only_when_it_changes_the_source() {
    use lightcraft_develop::CameraColour;
    let decodes = |colorimetric: bool| {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<LoadOptions>::new()));
        let log = seen.clone();
        let loader: FileLoader = Arc::new(move |_, _, options, _| {
            log.lock().unwrap().push(options);
            Ok((Rgb32f::new(8, 8), SourceInfo { raw: true, colorimetric, ..SourceInfo::default() }))
        });
        (loader, seen)
    };
    let set = |s: &mut crate::Session, colour: CameraColour| {
        let mut d = (*s.catalog.photo(PhotoId(1)).unwrap().develop).clone();
        d.profile.camera_colour = colour;
        s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(d), label: "edit".into(), edited: None }).unwrap();
    };
    let render = |s: &mut crate::Session| {
        let r = s.render_job(PhotoId(1), 64, 64, false, true).unwrap().run();
        s.accept(&r);
    };

    let mut s = session();
    let (loader, seen) = decodes(true);
    s.media.file_loader = Some(loader);
    render(&mut s);
    // a job made before the choice changes, finishing after it
    let late = s.render_job(PhotoId(1), 64, 64, false, true).unwrap();
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), 1, "the decoded source is reused while the choice stays");
    set(&mut s, CameraColour::Colorimetric);
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), 2, "a new choice decodes again");
    assert_eq!(seen.lock().unwrap()[1].colour, CameraColour::Colorimetric, "with the choice");
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), 2, "and the new source is reused");
    // the late job decodes with the old choice from the file
    let mut late = late;
    late.source = s.media.origin_ref(&Source::File { path: "hash-fixture.jpg".into() }, 64);
    let late = late.run();
    assert_eq!(late.loaded.as_ref().map(|l| l.options.colour), Some(CameraColour::Auto));
    s.accept(&late);
    let n = seen.lock().unwrap().len();
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), n, "the No Camera Matching source stayed in the cache");

    // a variant (the profile browser's thumbnail of the other choice) is decoded for that choice, not the photo's
    let mut other = (*s.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    other.profile.camera_colour = CameraColour::Auto;
    let n = seen.lock().unwrap().len();
    let r = s.variant_job(PhotoId(1), &other, 128).unwrap().run();
    assert_eq!(r.loaded.as_ref().map(|l| l.options.colour), Some(CameraColour::Auto));
    assert_eq!(seen.lock().unwrap().len(), n + 1);

    // a grid thumbnail goes through the thumbnail ("reduced") loader, which decodes for the photo's choice too
    let thumb_seen = Arc::new(std::sync::Mutex::new(Vec::<LoadOptions>::new()));
    let log = thumb_seen.clone();
    s.media.reduced_loader = Some(Arc::new(move |_, _, options, _| {
        log.lock().unwrap().push(options);
        Ok((Rgb32f::new(8, 8), SourceInfo { raw: true, colorimetric: true, ..SourceInfo::default() }, false))
    }));
    s.media.forget(PhotoId(1));
    let r = s.thumb_job(PhotoId(1), 256).unwrap().run();
    assert_eq!(
        thumb_seen.lock().unwrap().as_slice(),
        [LoadOptions { colour: CameraColour::Colorimetric, ..LoadOptions::default() }],
        "the thumbnail decodes without the match"
    );
    assert_eq!(r.loaded.as_ref().map(|l| l.options.colour), Some(CameraColour::Colorimetric));
    set(&mut s, CameraColour::Auto);
    s.media.forget(PhotoId(1));
    s.thumb_job(PhotoId(1), 256).unwrap().run();
    assert_eq!(thumb_seen.lock().unwrap().last().map(|o| o.colour), Some(CameraColour::Auto));

    // a camera whose starting colour isn't fitted to its JPEG: the choice changes nothing, so the source is kept
    let mut s = session();
    let (loader, seen) = decodes(false);
    s.media.file_loader = Some(loader);
    render(&mut s);
    set(&mut s, CameraColour::Colorimetric);
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), 1, "no second decode where the choice has no effect");
}

/// A decode that started before the photo's database lens profile changed (a lens database arriving, looked up again
/// by `cmd::lens_refresh`, which drops the photo's sources) never puts its source back in the cache: the source's
/// colour was fitted through the old profile.
#[test]
fn source_from_before_a_lens_profile_change_is_not_accepted() {
    let mut s = session();
    s.media.file_loader = Some(Arc::new(|_, _, _, _| Ok((Rgb32f::new(8, 8), SourceInfo::default()))));
    let old = s.thumb_job(PhotoId(1), 256).unwrap().run();
    assert!(old.loaded.is_some());
    let lens = lightcraft_develop::EmbeddedLens { source: lightcraft_develop::LensSource::Database, ..Default::default() };
    s.catalog.apply(Op::SetEmbeddedLens { id: PhotoId(1), lens: Some(Box::new(lens)) }).unwrap();
    s.media.forget(PhotoId(1));
    s.accept(&old);
    assert_eq!(s.media.source_usage().0, 0, "the old decode is dropped");
    // a decode made with the new profile is kept
    let new = s.thumb_job(PhotoId(1), 256).unwrap().run();
    s.accept(&new);
    assert_ne!(s.media.source_usage().0, 0);
}
/// The loupe's hover preview (a preset or profile under the pointer) is cached by its key, so a database lens
/// profile looked up again gives it a new key and the hover renders afresh instead of showing the old correction.
#[test]
fn a_hover_preview_key_follows_the_lens_profile() {
    let mut s = session();
    let settings = s.catalog.photo(PhotoId(1)).unwrap().develop.clone();
    let before = s.preview_job(PhotoId(1), 640, 480, true, &settings).unwrap().key;
    let lens = lightcraft_develop::EmbeddedLens { source: lightcraft_develop::LensSource::Database, ..Default::default() };
    s.catalog.apply(Op::SetEmbeddedLens { id: PhotoId(1), lens: Some(Box::new(lens)) }).unwrap();
    let after = s.preview_job(PhotoId(1), 640, 480, true, &settings).unwrap().key;
    assert_ne!(before, after);
    // one database profile looked up again as another (a newer database): the key follows the profile's contents,
    // not only whether there is one
    let vignette = lightcraft_develop::EmbeddedVignette { k: [-0.3, 0.0, 0.0, 0.0, 0.0], radius: 1.0, ..Default::default() };
    let other = lightcraft_develop::EmbeddedLens { source: lightcraft_develop::LensSource::Database, vignette: Some(vignette), ..Default::default() };
    s.catalog.apply(Op::SetEmbeddedLens { id: PhotoId(1), lens: Some(Box::new(other)) }).unwrap();
    let again = s.preview_job(PhotoId(1), 640, 480, true, &settings).unwrap().key;
    assert_ne!(after, again, "a different database profile");
}
/// A panic while rendering (here in the decoder) used to end the pool's worker thread without
/// an answer, so the view that asked showed "Rendering…" forever. It is the job's failure now,
/// for renders and for stand-ins, answering the request it came from.
#[test]
fn a_render_that_panics_fails_instead_of_never_answering() {
    let mut s = session();
    s.media.file_loader = Some(Arc::new(|_, _, _, _| panic!("decoder bug")));
    let mut job = s.thumb_job(PhotoId(1), 256).unwrap();
    job.request_id = 42;
    let (key, level, source_key) = (job.key, job.level, job.source_key);
    let r = job.run();
    assert_eq!((r.request_id, r.photo, r.key, r.level, r.source_key), (42, PhotoId(1), key, level, source_key));
    assert_eq!(r.rendered.err().as_deref(), Some("the render failed unexpectedly: decoder bug"));
    let quick = QuickJob {
        request_id: 7,
        photo: PhotoId(1),
        key: 9,
        cached: Vec::new(),
        embedded: Some(("x.arw".into(), Arc::new(|_: &str, _: usize| -> Option<Rgba8> { panic!("preview reader bug") }), 256)),
        minimum_embedded_edge: None,
        small: None,
    };
    let r = quick.run();
    assert_eq!((r.request_id, r.key), (7, 9));
    assert_eq!(r.rendered.err().as_deref(), Some("the render failed unexpectedly: preview reader bug"));
}

/// LT8 / LT4 (photo half): a photo on a local camera profile decodes again when its profile file changes, and its
/// three render keys follow the file; a photo on Camera Matching is untouched by the file (same keys, same decode).
#[test]
fn a_local_profile_file_decodes_and_renders_the_photo_afresh_when_it_changes() {
    use lightcraft_develop::CameraColour;
    let dir = std::env::temp_dir().join(format!("lc-local-source-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _dir = crate::camera_profiles::use_dir_for_tests(&dir);
    let write = |files: usize| {
        let table = lightcraft_raw::profile::HsvTable {
            hue_divisions: 4,
            sat_divisions: 2,
            val_divisions: 1,
            data: vec![[5.0, 1.2, 1.0]; 8],
            srgb_value: false,
        };
        let p = crate::camera_profiles::CameraProfile::new("ILCE-7M5", files, 100, lightcraft_color::Mat3::IDENTITY, Some(table));
        std::fs::write(dir.join("A7 V Chart.json"), serde_json::to_vec(&p).unwrap()).unwrap();
    };
    write(10);
    let mut s = session();
    let seen = Arc::new(std::sync::Mutex::new(Vec::<LoadOptions>::new()));
    let log = seen.clone();
    s.media.file_loader = Some(Arc::new(move |_, _, options, _| {
        log.lock().unwrap().push(options);
        Ok((Rgb32f::new(8, 8), SourceInfo { raw: true, colorimetric: true, ..SourceInfo::default() }))
    }));
    let keys = |s: &mut crate::Session| {
        let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
        let job = s.thumb_job(PhotoId(1), 256).unwrap();
        (
            job.key,
            job.cache.unwrap().1,
            crate::Session::variant_key(&p, &p.develop, 256, 0),
            crate::Session::view_key(&p, p.develop.hash64(), true, 0),
        )
    };
    let render = |s: &mut crate::Session| {
        let r = s.render_job(PhotoId(1), 64, 64, false, true).unwrap().run();
        s.accept(&r);
    };
    let auto_keys = keys(&mut s);
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), 1);
    // the photo chooses the profile
    let mut d = (*s.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    d.profile.camera_colour = CameraColour::Local;
    d.profile.camera_profile = Some("A7 V Chart".into());
    s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(d), label: "edit".into(), edited: None }).unwrap();
    render(&mut s);
    let (n, first) = (seen.lock().unwrap().len(), keys(&mut s));
    assert_eq!(n, 2, "a new choice decodes again");
    let used = *seen.lock().unwrap().last().unwrap();
    assert_eq!(used.colour, CameraColour::Local);
    assert_eq!(used.profile_key, Some(crate::camera_profiles::local_key("A7 V Chart")));
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), n, "and the source is reused while the file stays");
    // the file changes: decoded again, new keys
    write(11);
    render(&mut s);
    assert_eq!(seen.lock().unwrap().len(), n + 1, "a changed profile file decodes the photo again");
    let second = keys(&mut s);
    assert!(first.0 != second.0 && first.1 != second.1 && first.2 != second.2 && first.3 != second.3, "every render key follows the file");
    // back to Camera Matching: the keys are what they were before the profile existed, whatever the folder holds
    let mut d = (*s.catalog.photo(PhotoId(1)).unwrap().develop).clone();
    d.profile.camera_colour = CameraColour::Auto;
    d.profile.camera_profile = None;
    s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(d), label: "edit".into(), edited: None }).unwrap();
    assert_eq!(keys(&mut s), auto_keys);
    let _ = std::fs::remove_dir_all(&dir);
}

/// T11: a photo on macOS Colour has the Mac decoder's key (OS build and recipe) in its thumbnail, variant, view and job
/// keys, so a render made by another macOS build is made again; a Camera Matching photo's keys do not move with it and
/// stay what they were before macOS Colour existed (the formula of the test above, which predates it).
#[test]
fn macos_colour_renders_follow_the_mac_decoder_and_camera_matching_keys_stay() {
    use lightcraft_develop::CameraColour;
    let set_key = |k: u64| crate::files::MACOS_KEY_FOR_TESTS.with(|c| c.set(Some(k)));
    let mut s = session();
    let keys = |s: &mut crate::Session| {
        let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
        let job = s.thumb_job(PhotoId(1), 256).unwrap();
        (
            job.key,
            job.cache.unwrap().1,
            crate::Session::variant_key(&p, &p.develop, 256, 0),
            crate::Session::view_key(&p, p.develop.hash64(), true, 0),
        )
    };
    // Camera Matching: the same keys whatever the decoder's key, equal to the formula from before
    set_key(11);
    let auto = keys(&mut s);
    set_key(22);
    assert_eq!(keys(&mut s), auto, "a Camera Matching photo's keys never carry the macOS decoder");
    let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
    let b = thumb_bucket(256);
    let disk = Hasher128::new()
        .str(&content_key(&p))
        .u64(p.develop.hash64())
        .u64(b as u64)
        .u64(RENDER_CACHE_VERSION)
        .u64(u64::from(p.develop.process.process().version().0))
        .u64(crate::camera_profiles::cache_key())
        .finish();
    assert_eq!(auto.1, disk);
    // macOS Colour: every key follows the decoder's
    let mut d = (*p.develop).clone();
    d.profile.camera_colour = CameraColour::Macos;
    s.catalog.apply(Op::SetDevelop { id: PhotoId(1), settings: Arc::new(d), label: "edit".into(), edited: None }).unwrap();
    set_key(11);
    let first = keys(&mut s);
    assert_eq!(keys(&mut s), first, "stable while the OS build stays");
    set_key(22);
    let second = keys(&mut s);
    assert!(first.0 != second.0 && first.1 != second.1 && first.2 != second.2 && first.3 != second.3, "every render key follows the macOS decoder");
    // the decoded source is cached with the key too (another OS build decodes again)
    let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
    assert_eq!(LoadOptions::of(&p.develop).profile_key, Some(22));
    crate::files::MACOS_KEY_FOR_TESTS.with(|c| c.set(None));
}
/// The loupe asks for its render job, its stand-in key and the settings hash on every frame: the settings are hashed once,
/// when an edit makes them new, not once per frame (each hash writes every stroke of every mask to JSON).
#[test]
fn a_frame_of_loupe_keys_hashes_the_settings_once_per_edit() {
    let mut s = session();
    let id = PhotoId(1);
    let frame = |s: &mut crate::Session| {
        s.loupe_job(id, 256, 256, true).unwrap();
        s.quick_view_key(id, true, false).unwrap();
        s.quick_view_job(id, 256, true, false).unwrap();
        s.settings_hash(id).unwrap();
    };
    let start = lightcraft_develop::hash64_calls();
    for _ in 0..5 {
        frame(&mut s);
    }
    assert_eq!(s.media.settings_hashes.computations, 1);
    assert_eq!(lightcraft_develop::hash64_calls() - start, 1, "five frames of loupe keys hashed the settings more than once");
    let mut edit = DevelopSettings::default();
    edit.light.exposure = 1.0;
    s.catalog.apply(Op::SetDevelop { id, settings: Arc::new(edit), label: "edit".into(), edited: None }).unwrap();
    for _ in 0..5 {
        frame(&mut s);
    }
    assert_eq!(s.media.settings_hashes.computations, 2);
    assert_eq!(lightcraft_develop::hash64_calls() - start, 2);
}

/// A preview of settings the caller keeps (the brush tool's, asked for on every frame) costs no hash or copy of its own, and
/// renders under the key the plain preview job gives them.
#[test]
fn a_shared_preview_job_keeps_the_key_of_the_plain_one() {
    let mut s = session();
    let mut settings = DevelopSettings::default();
    settings.light.exposure = 0.5;
    let plain = s.preview_job(PhotoId(1), 256, 256, true, &settings).unwrap();
    let shared = Arc::new(settings.clone());
    let before = s.media.settings_hashes.computations;
    let kept = s.preview_job_shared(PhotoId(1), 256, 256, true, &shared, settings.hash64()).unwrap();
    assert_eq!(kept.key, plain.key);
    assert!(Arc::ptr_eq(&kept.settings, &shared), "the job renders the caller's own settings, not a copy");
    // (only the photo's own settings, for the job's other keys: the preview's are not hashed again)
    assert!(s.media.settings_hashes.computations <= before + 1);
}
