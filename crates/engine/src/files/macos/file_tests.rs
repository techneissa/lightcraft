//! A macOS Colour load of a file on disk: the decoder reads the file from its path, the job's cancel flag stops it,
//! and its weight at the memory gate.
use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::CameraColour;
use lightcraft_macraw::Error;
use lightcraft_pipeline::MacosUnavailable;

use super::super::load::load_bytes_now;
use super::tests::{Answer, D50, Fake, arw, options};

/// A cancelled render gives up the load (no Camera Matching decode in its place) and is not remembered: the next load
/// of the same file renders again.
#[test]
fn a_cancelled_render_is_tried_again() {
    let bytes = arw("ILCE-CANCEL1", 1);
    let cancelled = Fake::new(Answer::Fail(Error::Cancelled));
    cancelled.cancel.store(true, Ordering::Relaxed);
    let r = load_bytes_now(Cow::Borrowed(&bytes), 512, None, false, options(CameraColour::Macos), &cancelled);
    assert_eq!(r.err().as_deref(), Some("cancelled"));
    let fine = Fake::new(Answer::Grey(D50));
    let (_, _, info, _) = load_bytes_now(Cow::Borrowed(&bytes), 512, None, false, options(CameraColour::Macos), &fine).unwrap();
    assert_eq!((fine.calls.load(Ordering::SeqCst), info.macos_note), (1, None));
}

/// A load with a path hands the decoder the path (its own bytes are let go first); when the decoder fails, the file is
/// read again and decoded as Camera Matching.
#[test]
fn a_file_load_renders_from_its_path() {
    let dir = std::env::temp_dir().join(format!("lc-macos-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.ARW");
    let bytes = arw("ILCE-PATH1", 1);
    std::fs::write(&file, &bytes).unwrap();
    let fake = Fake { path: Some(file.clone()), ..Fake::new(Answer::Grey(D50)) };
    let (_, _, info, _) = load_bytes_now(Cow::Owned(bytes), 512, None, false, options(CameraColour::Macos), &fake).unwrap();
    assert_eq!((fake.inputs.lock().unwrap().clone(), info.macos_note), (vec!["path"], None));

    let bytes = arw("ILCE-PATH2", 1);
    std::fs::write(&file, &bytes).unwrap();
    let failing = Fake { path: Some(file), ..Fake::new(Answer::Fail(Error::Undecodable)) };
    let (img, _, info, _) = load_bytes_now(Cow::Owned(bytes), 512, None, false, options(CameraColour::Macos), &failing).unwrap();
    assert_eq!((failing.inputs.lock().unwrap().clone(), info.macos_note), (vec!["path"], Some(MacosUnavailable::Unreadable)));
    assert!(img.width > 0, "the Camera Matching decode of the file read again");
    std::fs::remove_dir_all(&dir).ok();
}

/// Borrowed bytes (a caller that keeps the file) are handed over as bytes, never the path.
#[test]
fn borrowed_bytes_are_rendered_as_bytes() {
    let bytes = arw("ILCE-PATH3", 1);
    let fake = Fake { path: Some("/nonexistent/a.ARW".into()), ..Fake::new(Answer::Grey(D50)) };
    load_bytes_now(Cow::Borrowed(&bytes), 512, None, false, options(CameraColour::Macos), &fake).unwrap();
    assert_eq!(*fake.inputs.lock().unwrap(), vec!["bytes"]);
}

/// T16d (engine side): an interactive macOS Colour load waits at the memory gate while another macOS render holds it,
/// instead of stacking a second gigabyte on it.
#[test]
fn an_interactive_macos_load_waits_at_the_gate() {
    if !lightcraft_macraw::available() {
        lightcraft_testkit::skip!(System, "no macOS raw decoder");
        return;
    }
    let floor = usize::try_from(lightcraft_macraw::FLOOR_BYTES).unwrap();
    let gate = crate::memory::WorkGate::new(floor + floor / 2);
    let held = gate.acquire_urgent(floor);
    let bytes = arw("ILCE-7M5", 1);
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let own = gate.acquire_urgent(1000);
            let _permit = crate::files::fs::macos_permit(&gate, own, 1000, &bytes, 2560, options(CameraColour::Macos));
            done.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!done.load(Ordering::SeqCst), "the loupe's macOS load stacked on the held render");
        drop(held);
    });
    assert!(done.load(Ordering::SeqCst));
}
