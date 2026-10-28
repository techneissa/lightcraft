//! LT9: the smart preview of a photo on a local camera profile carries the profile in its header, and goes stale only
//! when the profile file changed (or the photo left the profile) while the original can be read; a missing or unusable
//! file never makes one stale.

use lightcraft_catalog::{Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;

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
