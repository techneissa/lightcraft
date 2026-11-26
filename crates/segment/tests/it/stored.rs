//! Stored text features of fixed phrases (issue #437): a Sky mask from the stored features is the mask the text
//! encoder gives, bit for bit, on candle and on Core ML's split prompt model, and a new load reads them without
//! loading the encoder. With the real model from the shared models folder (or `LIGHTCRAFT_SAM3_DIR`), linked into a
//! temporary folder; skipped without it.
//!
//! Bit for bit because the stored features are the numbers the same engine's encoder gave, read back exactly, and
//! the detector runs on the same photo features: nothing else differs between the two paths.

use std::path::{Path, PathBuf};

use lightcraft_segment::remote::CONCEPT_THRESHOLD;
use lightcraft_segment::{Device, Runtime, Sam3, coreml, stored};

use crate::coreml::{linked_without, shared_dir};

/// A made-up landscape: a blue sky with a white cloud over green hills.
fn landscape() -> (Vec<u8>, usize, usize) {
    let (w, h) = (1200, 800);
    let rgb = (0..w * h)
        .flat_map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            let ridge = 0.55 + 0.08 * (x * 7.0).sin();
            if y > ridge {
                [(60.0 + 50.0 * y) as u8, (120.0 + 40.0 * x) as u8, 50]
            } else if (x - 0.3).powi(2) * 0.4 + (y - 0.2).powi(2) < 0.004 {
                [245, 245, 248]
            } else {
                [(70.0 + 80.0 * y) as u8, (130.0 + 70.0 * y) as u8, 230]
            }
        })
        .collect();
    (rgb, w, h)
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

fn modified(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap()
}

/// The Sky mask three ways on one load and photo: a fixed phrase's first run (the encoder's features, stored), its
/// second (the stored ones) and Describe (the encoder's), then a fixed phrase on a new load. Returns the mask.
fn sky_three_ways(dir: &Path, runtime: Runtime, device: Device, store: &str) -> Vec<f32> {
    let (rgb, w, h) = landscape();
    let mut m = Sam3::load_with(dir, device.clone(), runtime).unwrap();
    let mut enc = m.encode(&rgb, w, h).unwrap();
    let first = m.segment_concept(&mut enc, "sky", CONCEPT_THRESHOLD, false).unwrap().expect("the sky is found").logits;
    assert!(!m.text_encoder_loaded(), "the encoder loaded for a fixed phrase is let go");
    let file = stored::path(dir, store).unwrap();
    let written = modified(&file);
    let second = m.segment_concept(&mut enc, "sky", CONCEPT_THRESHOLD, false).unwrap().unwrap().logits;
    assert!(!m.text_encoder_loaded());
    let describe = m.segment_text_with(&mut enc, "sky", CONCEPT_THRESHOLD, false).unwrap().unwrap().logits;
    assert!(m.text_encoder_loaded(), "Describe loads the encoder");
    assert_eq!(bits(&first), bits(&describe), "the first fixed run is the encoder's mask");
    assert_eq!(bits(&second), bits(&describe), "the stored features give the encoder's mask");
    drop(m);

    let mut m = Sam3::load_with(dir, device, runtime).unwrap();
    let mut enc = m.encode(&rgb, w, h).unwrap();
    let again = m.segment_concept(&mut enc, "sky", CONCEPT_THRESHOLD, false).unwrap().unwrap().logits;
    assert!(!m.text_encoder_loaded());
    assert_eq!(modified(&file), written, "a new load reads the stored features (a miss would write the file again)");
    let flipped = again.iter().zip(&describe).filter(|(a, b)| (**a > 0.0) != (**b > 0.0)).count();
    eprintln!("{store}: a new load's Sky mask has {flipped} cells flipped against the first load's");
    assert_eq!(flipped, 0);
    describe
}

/// Removes a temporary models folder when the test ends, passed or failed.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(root) = self.0.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// The phrases a stored-features file holds (none when there is no file).
fn phrases(dir: &Path, store: &str) -> Vec<String> {
    let Ok(bytes) = std::fs::read(stored::path(dir, store).unwrap()) else { return Vec::new() };
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["phrases"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

#[test]
fn stored_text_features_give_the_encoders_mask_on_candle() {
    let _sam3 = crate::one_sam3_at_a_time();
    let Some(from) = shared_dir().filter(|d| lightcraft_segment::is_model_dir(d)) else {
        lightcraft_testkit::skip!(Model, "SAM 3 is not in the shared models folder (or LIGHTCRAFT_SAM3_DIR)");
        return;
    };
    let dir = Cleanup(linked_without(&from, "stored-candle", &coreml::Part::ALL));
    let device = lightcraft_segment::best_device();
    let store = if device.is_metal() { "candle-metal" } else { "candle-cpu" };
    sky_three_ways(&dir.0, Runtime::Candle, device, store);
}

/// Core ML's split prompt model; Find Subject (which runs candle's detector) keeps its features in candle's file,
/// never in Core ML's; and against the old single `textscore` part when that is there too (the masks before #437):
/// the same logits.
#[test]
fn stored_text_features_give_the_encoders_mask_on_core_ml() {
    let _sam3 = crate::one_sam3_at_a_time();
    let Some(from) = shared_dir().filter(|d| lightcraft_segment::is_model_dir(d)) else {
        lightcraft_testkit::skip!(Model, "SAM 3 is not in the shared models folder (or LIGHTCRAFT_SAM3_DIR)");
        return;
    };
    if !lightcraft_coreml::available() {
        lightcraft_testkit::skip!(Model, "Core ML runs only on a Mac");
        return;
    }
    let shared = coreml::folder(&from).unwrap();
    let has = |p: coreml::Part| coreml::paths(&shared, p).0.is_dir() || coreml::paths(&shared, p).1.is_dir();
    if !(coreml::installed(&from) && has(coreml::Part::TextEnc) && has(coreml::Part::TextDet)) {
        lightcraft_testkit::skip!(
            Model,
            "SAM 3's split Core ML prompt model is not beside it (tools/convert-coreml-sam3.py --parts textenc,textdet)"
        );
        return;
    }
    let dir = Cleanup(linked_without(&from, "stored-coreml", &[coreml::Part::TextScore]));
    let store = format!("coreml-{:?}", coreml::DEFAULT_UNITS).to_lowercase();
    let new = sky_three_ways(&dir.0, Runtime::CoreMl, Device::Cpu, &store);

    let (rgb, w, h) = landscape();
    let mut m = Sam3::load_with(&dir.0, lightcraft_segment::best_device(), Runtime::CoreMl).unwrap();
    let mut enc = m.encode(&rgb, w, h).unwrap();
    m.segment_subject(&mut enc, None, 0.5).unwrap();
    assert!(!m.text_encoder_loaded(), "the encoder loaded for Find Subject is let go");
    let candle = if m.device().is_metal() { "candle-metal" } else { "candle-cpu" };
    let (cm, cd) = (phrases(&dir.0, &store), phrases(&dir.0, candle));
    assert!(!cm.iter().any(|p| p == "person" || p == "animal"), "candle's features never go in Core ML's file: {cm:?}");
    assert!(cd.iter().any(|p| p == "person") && cd.iter().any(|p| p == "animal"), "they go in candle's: {cd:?}");
    drop(m);

    if !has(coreml::Part::TextScore) {
        return;
    }
    let old_dir = Cleanup(linked_without(&from, "stored-coreml-old", &[coreml::Part::TextEnc, coreml::Part::TextDet]));
    let mut m = Sam3::load_with(&old_dir.0, Device::Cpu, Runtime::CoreMl).unwrap();
    let mut enc = m.encode(&rgb, w, h).unwrap();
    let old = m.segment_concept(&mut enc, "sky", CONCEPT_THRESHOLD, false).unwrap().expect("the sky is found").logits;
    let flipped = old.iter().zip(&new).filter(|(a, b)| (**a > 0.0) != (**b > 0.0)).count();
    let most = old.iter().zip(&new).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    eprintln!("old textscore against textenc + textdet: {flipped} cells flipped, logits differ by at most {most}");
    assert_eq!(bits(&old), bits(&new), "the split prompt model gives the old one's logits");
}
