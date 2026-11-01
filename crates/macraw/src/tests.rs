use super::{Error, Input, Request, stub};
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// T1: the backend used off the Mac offers nothing and fails every call with `Unsupported`. Run on
/// every system (the Mac too), so the Linux CI and a Mac both cover it.
#[test]
fn stub_is_unavailable_and_unsupported() {
    assert!(!stub::available());
    assert!(!stub::supports("ILCE-7M5"));
    assert_eq!(stub::cache_key(), 0);
    let cancel = AtomicBool::new(false);
    let input = Input::Path(Path::new("/nonexistent.ARW"));
    assert_eq!(stub::probe(input), Err(Error::Unsupported));
    assert_eq!(stub::render(input, &Request::default(), &cancel), Err(Error::Unsupported));
    let bytes = Input::Bytes { data: &[0, 1, 2], uti: "com.sony.arw-raw-image" };
    assert_eq!(stub::render(bytes, &Request::default(), &cancel), Err(Error::Unsupported));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn off_the_mac_the_public_api_is_the_stub() {
    let cancel = AtomicBool::new(false);
    assert!(!super::available());
    assert_eq!(super::render(Input::Path(Path::new("x.ARW")), &Request::default(), &cancel), Err(Error::Unsupported));
}

#[test]
fn every_error_has_a_message() {
    let all = [
        Error::Unsupported,
        Error::Undecodable,
        Error::SizeMismatch { apple: (7008, 4672), expected: (6000, 4000) },
        Error::TooLarge { width: 30000, height: 20000 },
        Error::NeedsDownload { camera: "ILCE-7M5".into() },
        Error::Cancelled,
        Error::Os("x".into()),
    ];
    for e in all {
        assert!(!e.to_string().is_empty(), "{e:?}");
    }
}

#[test]
fn cache_key_follows_os_build_and_recipe() {
    let a = super::key_of("Version 27.0.1 (Build 26A434)", 1);
    assert_eq!(a, super::key_of("Version 27.0.1 (Build 26A434)", 1));
    assert_ne!(a, super::key_of("Version 27.0.2 (Build 26A500)", 1));
    assert_ne!(a, super::key_of("Version 27.0.1 (Build 26A434)", 2));
    assert_ne!(super::key_of("", 0), 0, "0 means unavailable");
}
