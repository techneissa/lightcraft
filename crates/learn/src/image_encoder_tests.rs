//! Core ML against candle (issue #105). The model tests read the models where the app keeps them, the folder shared
//! with PhotoCraft (`~/Library/Application Support/ArtCraft/models/dinov2-small`: `model.safetensors`, and
//! `coreml/dinov2-small.mlpackage` from `tools/convert-coreml-dinov2.py`), only through links in a temporary
//! folder, and skip without them. Nothing is downloaded and nothing is written there.

use std::path::{Path, PathBuf};

use super::{Backend, COREML_COMPILED, COREML_DIR, COREML_PACKAGE, ImageEncoder, Runtime};
use crate::{Error, ImageFeatures};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightcraft-learn-coreml-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(COREML_DIR)).unwrap();
    dir
}

/// `name` inside the shared DINOv2-small folder, when it is there.
fn shared(name: &str) -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(home.join("Library/Application Support/ArtCraft/models").join(crate::ENCODER_ID).join(name)).filter(|p| p.exists())
}

/// A model folder in `dir` holding a link to the real weights.
fn with_weights(dir: &Path, weights: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(weights, dir.join(crate::ENCODER_FILE)).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(weights, dir.join(crate::ENCODER_FILE)).unwrap();
}

fn junk_compiled(dir: &Path) {
    let c = dir.join(COREML_DIR).join(COREML_COMPILED);
    std::fs::create_dir_all(c.join("weights")).unwrap();
    std::fs::write(c.join("model.mil"), b"not a model").unwrap();
    std::fs::write(c.join("weights/weight.bin"), [3u8; 64]).unwrap();
}

/// The fixture: a 320 × 240 sky-over-field picture with a dark disc, made here.
fn fixture() -> (usize, usize, Vec<u8>) {
    let (w, h) = (320usize, 240usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let disc = (fx - 0.62).powi(2) + (fy - 0.4).powi(2) < 0.012;
            let px = if disc {
                [40.0, 30.0, 35.0]
            } else if fy < 0.55 {
                [90.0 + 80.0 * fy, 140.0 + 60.0 * fy, 230.0 - 30.0 * fx]
            } else {
                [70.0 + 60.0 * fx, 150.0 - 40.0 * fy, 50.0 + 20.0 * ((x / 7 + y / 5) % 2) as f32]
            };
            rgb.extend(px.map(|v: f32| v.clamp(0.0, 255.0) as u8));
        }
    }
    (w, h, rgb)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm(a) * norm(b))
}

#[test]
fn without_any_model_it_is_an_error_not_a_panic() {
    let dir = scratch("none");
    junk_compiled(&dir);
    std::fs::write(dir.join(crate::ENCODER_FILE), b"junk").unwrap();
    for runtime in Runtime::ALL {
        assert!(matches!(ImageEncoder::load(&dir, runtime), Err(Error::Model(_))), "{runtime:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_or_damaged_core_ml_model_falls_back_to_candle() {
    let Some(weights) = shared(crate::ENCODER_FILE) else {
        lightcraft_testkit::skip!(Model, "DINOv2-small's model.safetensors is not in the shared ArtCraft models folder");
        return;
    };
    let (w, h, rgb) = fixture();
    let dir = scratch("fallback");
    with_weights(&dir, &weights);
    let reference = ImageEncoder::load(&dir, Runtime::Candle).unwrap().features(w, h, &rgb).unwrap();

    let missing = ImageEncoder::load(&dir, Runtime::Automatic).unwrap();
    assert_eq!(missing.backend(), Backend::Candle);
    assert_eq!(missing.features(w, h, &rgb).unwrap(), reference);
    assert!(matches!(ImageEncoder::load(&dir, Runtime::CoreMl), Err(Error::Model(_))));

    junk_compiled(&dir);
    let damaged = ImageEncoder::load(&dir, Runtime::Automatic).unwrap();
    assert_eq!(damaged.backend(), Backend::Candle);
    assert_eq!(damaged.features(w, h, &rgb).unwrap(), reference);
    assert!(matches!(ImageEncoder::load(&dir, Runtime::CoreMl), Err(Error::Model(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "macos")]
#[test]
fn core_ml_gives_the_same_features_as_candle() {
    let package = format!("{COREML_DIR}/{COREML_PACKAGE}");
    let (Some(weights), Some(package)) = (shared(crate::ENCODER_FILE), shared(&package)) else {
        lightcraft_testkit::skip!(Model, "DINOv2-small and its Core ML package are not in the shared ArtCraft models folder");
        return;
    };
    let dir = scratch("same");
    with_weights(&dir, &weights);
    std::os::unix::fs::symlink(&package, dir.join(COREML_DIR).join(COREML_PACKAGE)).unwrap();
    let (w, h, rgb) = fixture();
    let candle = ImageEncoder::load(&dir, Runtime::Candle).unwrap().features(w, h, &rgb).unwrap();
    let coreml = ImageEncoder::load(&dir, Runtime::CoreMl).unwrap();
    assert_eq!(coreml.backend(), Backend::CoreMl);
    let got = coreml.features(w, h, &rgb).unwrap();
    let worst = candle.iter().zip(&got).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    let scale = candle.iter().map(|v| v.abs()).fold(0f32, f32::max);
    eprintln!("Core ML vs candle: cosine {:.6}, max |diff| {worst:.4} of max |value| {scale:.3}", cosine(&candle, &got));
    // fp16 on the Neural Engine against fp32 on candle
    assert!(cosine(&candle, &got) > 0.999, "cosine {}", cosine(&candle, &got));
    assert!(worst < 0.05 * scale, "max |diff| {worst} of {scale}");
    // the compiled model was kept for the next start
    assert!(dir.join(COREML_DIR).join(COREML_COMPILED).is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}
