//! SAM 3 on Core ML against candle (issue #268), with the real model and its conversion from the shared models folder
//! (`~/Library/Application Support/ArtCraft/models/sam3` and `sam3-coreml`, or `LIGHTCRAFT_SAM3_DIR` and its sibling
//! `sam3-coreml`). They are linked into a temporary folder: a missing compiled copy is made there, nothing is written
//! in the shared folder. Skipped without them or off the Mac.
//!
//! Clicks (1, 2, 3 and 5, with excludes, and a click that removes an object) and prompts (two that match, one that
//! matches nothing) give candle's masks; a Core ML output that is not finite is an error on Core ML only and hands
//! the run to candle on Automatic.

use std::path::{Path, PathBuf};

use lightcraft_segment::{Click, Encoded, Error, Runtime, Sam3, best_device, coreml};

fn shared_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("LIGHTCRAFT_SAM3_DIR") {
        return Some(PathBuf::from(d));
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/ArtCraft/models/sam3"))
}

#[cfg(unix)]
fn link(from: &Path, to: &Path) {
    std::os::unix::fs::symlink(from, to).unwrap();
}

#[cfg(not(unix))]
fn link(_: &Path, _: &Path) {}

/// The model and its packages linked into a temporary models folder (and the compiled copies, when made already).
fn linked(from: &Path) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lc-sam3-coreml-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (dir, cm) = (root.join("sam3"), root.join(coreml::FOLDER));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&cm).unwrap();
    for f in [lightcraft_segment::WEIGHTS_FILE, "vocab.json", "merges.txt"] {
        link(&from.join(f), &dir.join(f));
    }
    let shared = coreml::folder(from).unwrap();
    for part in coreml::Part::ALL {
        let ((package, compiled), (to_package, to_compiled)) = (coreml::paths(&shared, part), coreml::paths(&cm, part));
        link(&package, &to_package);
        let key = PathBuf::from(format!("{}.key", compiled.display()));
        if compiled.is_dir() && key.is_file() {
            link(&compiled, &to_compiled);
            std::fs::copy(&key, format!("{}.key", to_compiled.display())).unwrap();
        }
    }
    dir
}

/// A made-up photo: a red disc and a blue square on a grey-green gradient.
fn photo() -> (Vec<u8>, usize, usize) {
    let (w, h) = (1200, 900);
    let rgb = (0..w * h)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            if (x - 0.33).powi(2) + ((y - 0.5) * 0.75).powi(2) < 0.018 {
                [210, 30, 35]
            } else if (0.6..0.82).contains(&x) && (0.3..0.62).contains(&y) {
                [30, 60, 200]
            } else {
                [(110.0 + 40.0 * x) as u8, (130.0 + 30.0 * y) as u8, 100]
            }
        })
        .collect();
    (rgb, w, h)
}

fn click(x: f32, y: f32, positive: bool) -> Click {
    Click { x, y, positive }
}

/// Every selection the test compares, in order: (what, logits or nothing selected).
fn selections(model: &mut Sam3, rgb: &[u8], w: usize, h: usize) -> Vec<(String, Option<Vec<f32>>)> {
    let mut enc: Encoded = model.encode(rgb, w, h).unwrap();
    model.prepare_clicks(&mut enc).unwrap();
    let sets = [
        vec![click(0.33, 0.5, true)],
        vec![click(0.33, 0.5, true), click(0.7, 0.45, false)],
        vec![click(0.33, 0.5, true), click(0.7, 0.45, true), click(0.1, 0.1, false)],
        vec![click(0.33, 0.5, true), click(0.25, 0.45, true), click(0.7, 0.45, false), click(0.9, 0.9, false), click(0.75, 0.55, true)],
    ];
    let mut out = Vec::new();
    for set in &sets {
        out.push((format!("{} clicks", set.len()), Some(model.segment_clicks(&mut enc, set).unwrap().logits)));
    }
    out.push(("remove click".into(), Some(model.segment_object(&mut enc, &sets[0]).unwrap().logits)));
    for prompt in ["red circle", "blue square", "giraffe"] {
        out.push((format!("prompt {prompt:?}"), model.segment_text(&mut enc, prompt, 0.5).unwrap().map(|p| p.logits)));
    }
    out
}

#[test]
fn core_ml_gives_candles_masks() {
    let Some(from) = shared_dir().filter(|d| lightcraft_segment::is_model_dir(d)) else {
        lightcraft_testkit::skip!(Model, "SAM 3 is not in the shared models folder (or LIGHTCRAFT_SAM3_DIR)");
        return;
    };
    if !lightcraft_coreml::available() {
        lightcraft_testkit::skip!(Model, "Core ML runs only on a Mac");
        return;
    }
    if !coreml::installed(&from) {
        lightcraft_testkit::skip!(Model, "SAM 3's Core ML conversion is not beside it (tools/convert-coreml-sam3.py)");
        return;
    }
    let dir = linked(&from);
    let (rgb, w, h) = photo();

    let mut cm = Sam3::load_with(&dir, best_device(), Runtime::CoreMl).unwrap();
    assert_eq!(cm.backend(), "coreml");
    let got = selections(&mut cm, &rgb, w, h);
    // a non-finite output is an error on Core ML only
    let e = cm.encode_values(vec![f32::NAN; 3 * 1008 * 1008]).err().unwrap();
    assert!(matches!(e, Error::CoreMl(_)), "{e}");
    drop(cm);

    // on Automatic it hands this and every later run to candle
    let mut auto = Sam3::load_with(&dir, best_device(), Runtime::Automatic).unwrap();
    assert_eq!(auto.backend(), "coreml");
    assert!(auto.encode_values(vec![f32::NAN; 3 * 1008 * 1008]).is_ok());
    assert_eq!(auto.backend(), "candle");
    let want = selections(&mut auto, &rgb, w, h);

    assert_eq!(got.len(), want.len());
    let mut matched = 0;
    for ((what, g), (_, c)) in got.iter().zip(&want) {
        match (g, c) {
            (None, None) => eprintln!("{what}: nothing selected by either"),
            (Some(g), Some(c)) => {
                matched += 1;
                let (a, b): (Vec<bool>, Vec<bool>) = (g.iter().map(|v| *v > 0.0).collect(), c.iter().map(|v| *v > 0.0).collect());
                let union = a.iter().zip(&b).filter(|(x, y)| **x || **y).count();
                let both = a.iter().zip(&b).filter(|(x, y)| **x && **y).count();
                let flipped = a.iter().zip(&b).filter(|(x, y)| x != y).count();
                let iou = if union == 0 { 1.0 } else { both as f64 / union as f64 };
                eprintln!("{what}: IoU {iou:.5}, {flipped} of {} cells flipped, {} selected", a.len(), b.iter().filter(|v| **v).count());
                assert!(iou >= 0.99, "{what}: IoU {iou}");
            }
            _ => panic!("{what}: Core ML {} but candle {}", g.is_some(), c.is_some()),
        }
    }
    assert!(matched >= 6, "the clicks and at least one prompt selected something");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
}
