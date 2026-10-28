//! Mac-only tests against real raws. They assert properties (size, finiteness, ranges,
//! determinism), never stored Apple output. Files: `LIGHTCRAFT_MACRAW_ARW` (a bright landscape
//! ARW), `LIGHTCRAFT_MACRAW_PORTRAIT` (a portrait ARW), else the first `.arw` in the corpus.

use super::{available, os_build, render, render_bands};
use crate::{Error, Input, Request};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn corpus_arw() -> Option<PathBuf> {
    let root = std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"));
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("raw"))
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("arw")))
        .collect();
    files.sort();
    files.into_iter().next()
}

/// The file a test needs, or `None` after recording why it skipped.
fn sample(var: &str, corpus_fallback: bool) -> Option<PathBuf> {
    if !available() {
        lightcraft_testkit::skip!(System, "no macOS raw decoder");
        return None;
    }
    let file = std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_file());
    let file = file.or_else(|| corpus_fallback.then(corpus_arw).flatten());
    if file.is_none() {
        lightcraft_testkit::skip!(Input, "set {var} to an ARW file (or download the corpus)");
    }
    file
}

fn at(edge: u32) -> Request {
    Request { long_edge: Some(edge), ..Request::default() }
}

/// T15: a 1600 px render is the right size, finite, non-negative, keeps values above 1 on a bright
/// scene, has a daylight-to-tungsten white and is the same twice.
#[test]
fn renders_a_raw_at_1600_px() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_ARW", true) else { return };
    eprintln!("macOS: {}", os_build());
    let cancel = AtomicBool::new(false);
    let t = std::time::Instant::now();
    let a = render(Input::Path(&file), &at(1600), &cancel).unwrap();
    eprintln!(
        "{}: {}×{} of {:?} in {:.2} s, white {:?} {} K / {}, decoder {}",
        file.display(),
        a.width,
        a.height,
        a.native_size,
        t.elapsed().as_secs_f64(),
        a.white_xy,
        a.white_kelvin,
        a.white_tint,
        a.decoder_version
    );
    assert_eq!(a.width.max(a.height), 1600);
    let (nw, nh) = a.native_size;
    let aspect = |w: u32, h: u32| f64::from(w) / f64::from(h);
    assert!((aspect(a.width, a.height) - aspect(nw, nh)).abs() < 2.0 / 1600.0, "aspect kept");
    assert_eq!(a.rgb.len(), a.width as usize * a.height as usize * 3);
    assert!(a.rgb.iter().all(|v| v.is_finite() && *v >= 0.0));
    let max = a.rgb.iter().copied().fold(0f32, f32::max);
    eprintln!("max linear value {max}");
    let [x, y] = a.white_xy;
    assert!((0.30..=0.46).contains(&x) && (0.30..=0.42).contains(&y), "as-shot white xy {x}, {y}");
    let b = render(Input::Path(&file), &at(1600), &cancel).unwrap();
    assert!(a == b, "two renders of one file are identical");
    if std::env::var_os("LIGHTCRAFT_MACRAW_ARW").is_some() {
        assert!(max > 1.0, "a bright scene keeps values above 1 (extended range)");
    }
}

/// T16 (portrait): Core Image applies the EXIF orientation, so a portrait file comes out upright.
#[test]
fn portrait_comes_out_upright() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_PORTRAIT", false) else { return };
    let img = render(Input::Path(&file), &at(800), &AtomicBool::new(false)).unwrap();
    assert!(img.height > img.width, "{}×{}", img.width, img.height);
    assert!(img.native_size.1 > img.native_size.0);
}

/// T16: bad input is an error, never a panic or an empty picture.
#[test]
fn bad_input_is_an_error() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_ARW", true) else { return };
    let cancel = AtomicBool::new(false);
    let uti = "com.sony.arw-raw-image";
    assert!(render(Input::Bytes { data: &[], uti }, &at(512), &cancel).is_err());
    let bytes = std::fs::read(&file).unwrap();
    let head = &bytes[..bytes.len().min(1 << 20)];
    assert!(render(Input::Bytes { data: head, uti }, &at(512), &cancel).is_err(), "truncated file");
    let garbage: Vec<u8> = (0..1 << 16).map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    assert!(render(Input::Bytes { data: &garbage, uti }, &at(512), &cancel).is_err());
    assert!(render(Input::Path(Path::new("/nonexistent/x.ARW")), &at(512), &cancel).is_err());
    assert_eq!(render(Input::Path(&file), &at(512), &AtomicBool::new(true)), Err(Error::Cancelled));
}

/// Banding does not change the picture: 64-row bands equal one band.
#[test]
fn bands_join_seamlessly() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_ARW", true) else { return };
    let cancel = AtomicBool::new(false);
    let one = render_bands(Input::Path(&file), &at(600), &cancel, u32::MAX).unwrap();
    let many = render_bands(Input::Path(&file), &at(600), &cancel, 64).unwrap();
    assert_eq!((one.width, one.height), (many.width, many.height));
    // Core Image tiles each band on its own, so values agree to float noise, not bit for bit
    // (measured: largest difference 0.0015 on values up to 3). A one-row shift would be far larger.
    let diffs = || one.rgb.iter().zip(&many.rgb).map(|(a, b)| f64::from((a - b).abs()));
    let worst = diffs().fold(0f64, f64::max);
    let mean = diffs().sum::<f64>() / one.rgb.len() as f64;
    eprintln!("bands: largest difference {worst}, mean {mean}");
    assert!(worst < 0.01 && mean < 1e-4, "largest difference {worst}, mean {mean}");
}

fn resident_bytes() -> Option<u64> {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok().map(|kb| kb * 1024)
}

/// T16b: 100 thumbnail renders on one thread leave the resident size within 50 MB (one
/// autorelease pool per render; without it every render's Objective-C objects stay until exit).
#[test]
fn thumbnails_do_not_leak() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_ARW", true) else { return };
    let cancel = AtomicBool::new(false);
    std::thread::spawn(move || {
        for _ in 0..5 {
            render(Input::Path(&file), &at(512), &cancel).unwrap();
        }
        let before = resident_bytes().unwrap();
        for _ in 0..100 {
            render(Input::Path(&file), &at(512), &cancel).unwrap();
        }
        let after = resident_bytes().unwrap();
        let grew = after.saturating_sub(before) as f64 / 1e6;
        eprintln!("resident {:.0} MB → {:.0} MB (+{grew:.1} MB) over 100 renders", before as f64 / 1e6, after as f64 / 1e6);
        assert!(grew < 50.0, "grew {grew:.1} MB");
    })
    .join()
    .unwrap();
}

/// T16: the exact type hint decodes the bytes like the path; a wrong or generic one is
/// `Undecodable`, never an empty picture.
#[test]
fn bytes_need_the_exact_type_hint() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_ARW", true) else { return };
    let cancel = AtomicBool::new(false);
    let bytes = std::fs::read(&file).unwrap();
    let uti = crate::uti_for_extension("arw").unwrap();
    let t = std::time::Instant::now();
    let from_bytes = render(Input::Bytes { data: &bytes, uti }, &at(1600), &cancel).unwrap();
    eprintln!("bytes path at 1600 px: {:.2} s", t.elapsed().as_secs_f64());
    let from_path = render(Input::Path(&file), &at(1600), &cancel).unwrap();
    assert_eq!((from_bytes.width, from_bytes.height, from_bytes.white_xy), (from_path.width, from_path.height, from_path.white_xy));
    let worst = from_bytes.rgb.iter().zip(&from_path.rgb).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    assert!(worst < 1e-3, "bytes and path differ by {worst}");
    for hint in ["ARW", ".arw", "", "public.camera-raw-image"] {
        assert_eq!(render(Input::Bytes { data: &bytes, uti: hint }, &at(512), &cancel), Err(Error::Undecodable), "hint {hint:?}");
    }
}

/// T5b on Apple's real list, `probe` against `render`, and the size gate on a real file (T5c).
#[test]
fn gates_and_probe_on_a_real_file() {
    let Some(file) = sample("LIGHTCRAFT_MACRAW_ARW", true) else { return };
    assert!(super::supports("ILCE-7M5"));
    assert!(!super::supports("SONY ILCE-7M5"));
    assert!(!super::supports("NO-SUCH-CAMERA-1"));
    assert_ne!(super::cache_key(), 0);
    let t = std::time::Instant::now();
    let probe = super::probe(Input::Path(&file)).unwrap();
    eprintln!("probe {:?} in {:.2} s", probe.native_size, t.elapsed().as_secs_f64());
    let cancel = AtomicBool::new(false);
    let (w, h) = probe.native_size;
    // 1 % smaller than Apple's: inside the gate, resampled to exactly that size (scaled to 800).
    let expected = (w - w / 100, h - h / 100);
    let req = Request { long_edge: Some(800), expected_size: Some(expected), ..Request::default() };
    let img = render(Input::Path(&file), &req, &cancel).unwrap();
    assert_eq!(img.native_size, probe.native_size);
    let s = 800.0 / f64::from(expected.0.max(expected.1));
    let want = ((f64::from(expected.0) * s).round() as u32, (f64::from(expected.1) * s).round() as u32);
    assert_eq!((img.width, img.height), want);
    let off = (w + w * 3 / 100, h);
    let req = Request { long_edge: Some(800), expected_size: Some(off), ..Request::default() };
    assert_eq!(render(Input::Path(&file), &req, &cancel), Err(Error::SizeMismatch { apple: (w, h), expected: off }));
}
