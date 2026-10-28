//! LT9: the smart preview of a photo on a local camera profile carries the profile in its header, and goes stale only
//! when the profile file changed (or the photo left the profile) while the original can be read; a missing or unusable
//! file never makes one stale.

use lightcraft_catalog::{Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;

use super::import;
use crate::camera_profiles::{self, CameraProfile};
use crate::cmd::previews::smart_local;
use crate::smart::{LocalWant, encode, encode_stamped, is_local_stale, local_stamp};

fn dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lc-smart-local-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn raw_photo(camera: &str, local: Option<&str>) -> Photo {
    let mut p = Photo::new(PhotoId(1), Source::File { path: "/p/a.arw".into() }, "a.arw", "ARW", 6000, 4000, "2026-10-28");
    p.kind = lightcraft_catalog::MediaKind::Raw;
    p.meta.camera = camera.into();
    if let Some(id) = local {
        let mut d = (*p.develop).clone();
        d.profile.camera_colour = CameraColour::Local;
        d.profile.camera_profile = Some(id.into());
        p.develop = std::sync::Arc::new(d);
    }
    p
}

fn profile(model: &str) -> CameraProfile {
    CameraProfile::new(model, 40, 9000, lightcraft_color::Mat3::IDENTITY, None)
}

#[test]
fn a_smart_preview_follows_the_local_profile_of_its_photo() {
    let d = dir("header");
    let img = lightcraft_scenes::demo_library()[0].render(32, 20);
    let plain = d.join("plain.lcsp");
    let stamped = d.join("stamped.lcsp");
    std::fs::write(&plain, encode(&img, None).unwrap()).unwrap();
    std::fs::write(&stamped, encode_stamped(&img, None, Some("A7 V Chart:00000000000000aa")).unwrap()).unwrap();
    let is = |s: &str| LocalWant::Is(s.to_owned());
    // a proxy without a profile: stale for a photo on one, fine for a photo on none, kept for a photo whose file is gone
    assert!(is_local_stale(&plain, &is("A7 V Chart:00000000000000aa")));
    assert!(!is_local_stale(&plain, &LocalWant::No));
    assert!(!is_local_stale(&plain, &LocalWant::Keep));
    // a proxy with this profile as the file is now: current; with another content or another profile: stale
    assert!(!is_local_stale(&stamped, &is("A7 V Chart:00000000000000aa")));
    assert!(is_local_stale(&stamped, &is("A7 V Chart:00000000000000bb")), "the file changed");
    assert!(is_local_stale(&stamped, &is("Other:00000000000000aa")), "another profile");
    assert!(is_local_stale(&stamped, &LocalWant::No), "the photo left the profile");
    assert!(!is_local_stale(&stamped, &LocalWant::Keep), "the file is missing or unusable: the proxy is kept");
    // nothing readable is never stale for this (it is `is_valid`'s business)
    assert!(!is_local_stale(&d.join("missing.lcsp"), &is("x:1")));
    std::fs::write(d.join("cut.lcsp"), b"not a smart preview").unwrap();
    assert!(!is_local_stale(&d.join("cut.lcsp"), &is("x:1")));
    // the stamp does not make a proxy stale for the fit version
    assert!(!crate::smart::is_stale_for(&stamped, true));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_photo_is_built_with_its_local_profile_only_while_the_file_is_usable_for_its_camera() {
    let d = dir("jobs");
    camera_profiles::save_named(&profile("ILCE-7M5"), &d, "A7 V Chart").unwrap();
    std::fs::write(d.join("broken.json"), b"{}").unwrap();
    let _d = camera_profiles::use_dir_for_tests(&d);
    let (options, want) = smart_local(&raw_photo("SONY ILCE-7M5", Some("A7 V Chart")));
    let key = camera_profiles::local_key("A7 V Chart");
    assert_eq!(options.map(|o| (o.colour, o.profile_key)), Some((CameraColour::Local, Some(key))));
    assert_eq!(want, LocalWant::Is(local_stamp("A7 V Chart", key)));
    // Camera Matching, whatever the photo is set to otherwise
    assert_eq!(smart_local(&raw_photo("SONY ILCE-7M5", None)), (None, LocalWant::No));
    // a file that is gone, unusable or for another camera: whatever the proxy has is kept
    for (camera, id) in [("SONY ILCE-7M5", "Gone"), ("SONY ILCE-7M5", "broken"), ("SONY ILCE-7M4", "A7 V Chart")] {
        assert_eq!(smart_local(&raw_photo(camera, Some(id))), (None, LocalWant::Keep), "{camera} {id}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The colour base a proxy names: `auto` for Camera Matching, `local` for a local profile.
#[test]
fn a_smart_preview_names_its_colour_base_and_one_without_is_stale_for_the_fitted_look() {
    let d = dir("base");
    let img = lightcraft_scenes::demo_library()[0].render(32, 20);
    let plain = d.join("plain.lcsp");
    let stamped = d.join("stamped.lcsp");
    std::fs::write(&plain, encode(&img, None).unwrap()).unwrap();
    std::fs::write(&stamped, encode_stamped(&img, None, Some("A7 V Chart:00000000000000aa")).unwrap()).unwrap();
    assert_eq!(import::smart_header(&plain)["base"], "auto");
    assert_eq!(import::smart_header(&stamped)["base"], "local");
    assert!(!crate::smart::is_stale_for(&plain, true) && !crate::smart::is_stale_for(&stamped, true));
    // current in every other way, but written before the base was named: stale for the fitted look only
    for f in [&plain, &stamped] {
        import::edit_smart_header(f, |h| {
            h.as_object_mut().unwrap().remove("base");
        });
        assert!(crate::smart::is_stale_for(f, true), "no base: out of date for a photo with the fitted look");
        assert!(!crate::smart::is_stale_for(f, false), "a JPEG's or PNG's proxy is not about the fit");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The proxy of a Camera Matching photo written while a local profile's colour was applied silently (an old
/// `.lcsp`: no `base`, the previous fit version, and the local profile's pixels) is rebuilt from the original.
#[test]
fn an_old_style_smart_preview_of_a_camera_matching_photo_is_rebuilt() {
    use serde_json::json;
    let src = import::temp_dir("oldbase-src");
    let lib = import::temp_dir("oldbase-lib");
    import::write_png(&src.join("a.png"), 3);
    let mut session = crate::Session::new().with_fs();
    session.open_library(&lib, false).unwrap();
    session.execute("library.import", &json!({"paths": [src.join("a.png").to_string_lossy()]})).unwrap();
    let id = session.catalog.photos().next().unwrap().id;
    // stands for a raw on the fitted look (the test loader decodes its pixels)
    let mut raw = (**session.catalog.photo(id).unwrap()).clone();
    raw.format = "ARW".into();
    raw.kind = lightcraft_catalog::MediaKind::Raw;
    session.catalog.apply(lightcraft_catalog::Op::RemovePhoto { id }).unwrap();
    session.catalog.apply(lightcraft_catalog::Op::AddPhoto { photo: Box::new(raw) }).unwrap();
    let photo = session.catalog.photo(id).unwrap().clone();
    assert!(photo.fitted_look() && photo.develop.profile.camera_colour != CameraColour::Local);
    let r = session.execute("library.smartPreviews", &json!({})).unwrap();
    assert_eq!(r["built"].as_u64(), Some(1), "{r}");
    let dir = session.media.smart_dir.clone().unwrap();
    let file = crate::smart::sharded_file_path(&dir, &photo);
    assert_eq!(import::smart_header(&file)["base"], "auto");
    // make it the old style: the previous fit version, no base, and a marker that said it was checked
    import::edit_smart_header(&file, |h| {
        let o = h.as_object_mut().unwrap();
        o.remove("base");
        o.insert("look_version".into(), json!(crate::camera_preview::LOOK_VERSION - 1));
    });
    let old = std::fs::read(&file).unwrap();
    assert!(crate::smart::is_stale(&file));
    let r = session.execute("library.smartPreviews", &json!({})).unwrap();
    assert_eq!(r["refreshed"].as_u64(), Some(1), "{r}");
    assert_ne!(std::fs::read(&file).unwrap(), old, "rebuilt");
    let head = import::smart_header(&file);
    assert_eq!((head["base"].as_str(), head["look_version"].as_u64()), (Some("auto"), Some(u64::from(crate::camera_preview::LOOK_VERSION))));
    assert!(head.get("local_profile").is_none());
    assert!(!crate::smart::is_stale(&file));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// T12: the smart preview of a photo on macOS Colour is made from the macOS base where this computer can make it and
/// says so (`base: macos` and the base's version). It goes stale for an older base version, never for a macOS update
/// (the OS build is not recorded); a computer that can't make it keeps whatever proxy there is and builds none.
#[test]
fn a_macos_colour_photo_keeps_macos_pixels_in_its_smart_preview() {
    use crate::cmd::previews::smart_colour;
    use crate::smart::{MACOS_BASE_VERSION, encode_for};
    let d = dir("macos");
    let mut photo = raw_photo("SONY ILCE-7M5", None);
    let mut dev = (*photo.develop).clone();
    dev.profile.camera_colour = CameraColour::Macos;
    photo.develop = std::sync::Arc::new(dev);
    // which proxy the photo wants, and how its original loads
    let (options, want) = smart_colour(&photo, true);
    assert_eq!(want, LocalWant::Macos);
    assert_eq!(options.map(|o| o.colour), Some(CameraColour::Macos), "built from the macOS base");
    assert_eq!(smart_colour(&photo, false), (None, LocalWant::MacosElsewhere));
    assert_eq!(smart_colour(&raw_photo("SONY ILCE-7M5", None), true), (None, LocalWant::No), "Camera Matching photos are unchanged");
    // the header
    let img = lightcraft_scenes::demo_library()[0].render(32, 20);
    let macos = d.join("macos.lcsp");
    let plain = d.join("plain.lcsp");
    std::fs::write(&macos, encode_for(&img, None, &LocalWant::Macos).unwrap()).unwrap();
    std::fs::write(&plain, encode(&img, None).unwrap()).unwrap();
    let head = import::smart_header(&macos);
    assert_eq!(head["base"], "macos");
    assert_eq!(head["macos_base_version"], MACOS_BASE_VERSION);
    assert!(head.get("os_build").is_none() && head.get("local_profile").is_none());
    // staleness
    assert!(!crate::smart::is_stale_for(&macos, true), "a current macOS proxy is current for the fit");
    assert!(!is_local_stale(&macos, &LocalWant::Macos));
    assert!(is_local_stale(&plain, &LocalWant::Macos), "a Camera Matching proxy is rebuilt on the Mac");
    assert!(is_local_stale(&macos, &LocalWant::No), "the photo left macOS Colour");
    assert!(!is_local_stale(&macos, &LocalWant::MacosElsewhere) && !is_local_stale(&plain, &LocalWant::MacosElsewhere), "kept off the Mac");
    import::edit_smart_header(&macos, |h| h["macos_base_version"] = serde_json::Value::from(MACOS_BASE_VERSION - 1));
    assert!(is_local_stale(&macos, &LocalWant::Macos), "an older macOS base is rebuilt");
    import::edit_smart_header(&macos, |h| h["macos_base_version"] = serde_json::Value::from(MACOS_BASE_VERSION + 1));
    assert!(!is_local_stale(&macos, &LocalWant::Macos), "a newer build's proxy is left alone");
    let _ = std::fs::remove_dir_all(&d);
}
