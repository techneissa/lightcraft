use crate::cache::{key_path, package_key};
use crate::layout::{Elem, check_io, check_shape, contiguous, f16_to_f32, gather, replace_dir};
use crate::{ComputeUnits, Error, Model, Tensor, compile, load_package};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lightcraft-coreml-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shapes_are_checked_before_any_call() {
    assert_eq!(check_shape(&[1, 3, 2, 2], 12), Ok(12));
    assert!(matches!(check_shape(&[1, 3, 2, 2], 11), Err(Error::Input(_))));
    assert!(matches!(check_shape(&[], 0), Err(Error::Input(_))));
    assert!(matches!(check_shape(&[1, 0, 4], 0), Err(Error::Input(_))));
    assert!(matches!(check_shape(&[1, 1, 1, 1, 1, 1], 1), Err(Error::Input(_))));
    assert!(matches!(check_shape(&[usize::MAX, 2], 0), Err(Error::Input(_))));
    assert!(matches!(check_shape(&[crate::MAX_ELEMENTS + 1], crate::MAX_ELEMENTS + 1), Err(Error::Input(_))));
    assert_eq!(contiguous(&[2, 3, 4]), vec![12, 4, 1]);
}

/// A call with several tensors is checked before Core ML sees it: each input of the model given once with a shape that
/// fits, each output known and asked for once.
#[test]
fn several_tensors_are_checked_before_any_call() {
    let have_in: Vec<String> = ["image", "labels"].map(String::from).to_vec();
    let have_out: Vec<String> = ["masks", "iou"].map(String::from).to_vec();
    let (image, labels) = ([0f32; 6], [1i32, -1]);
    let ok = [Tensor::f32("image", &[1, 2, 3], &image), Tensor::i32("labels", &[1, 2], &labels)];
    assert_eq!(check_io(&have_in, &have_out, &ok, &["iou", "masks"]), Ok(()));
    let input_err = |inputs: &[Tensor<'_>]| matches!(check_io(&have_in, &have_out, inputs, &["masks"]), Err(Error::Input(_)));
    assert!(input_err(&ok[..1]), "a model input left out");
    assert!(input_err(&[ok[0], ok[1], ok[1]]), "an input given twice");
    assert!(input_err(&[ok[0], Tensor::i32("labels", &[1, 3], &labels)]), "a shape that doesn't fit its numbers");
    assert!(input_err(&[ok[0], ok[1], Tensor::f32("other", &[1], &[0.0])]), "a name the model lacks");
    assert!(input_err(&[]));
    let output_err = |outputs: &[&str]| matches!(check_io(&have_in, &have_out, &ok, outputs), Err(Error::Output(_)));
    assert!(output_err(&["masks", "score"]) && output_err(&["iou", "iou"]));
    assert!(matches!(check_io(&have_in, &have_out, &ok, &[]), Err(Error::Input(_))));
}

#[test]
fn half_floats_read_right() {
    assert_eq!(f16_to_f32(0x3c00), 1.0);
    assert_eq!(f16_to_f32(0xc000), -2.0);
    assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
    assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
    assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
    assert!(f16_to_f32(0x7e00).is_nan());
    assert_eq!(f16_to_f32(0x8001), -(2f32.powi(-24)));
    assert_eq!(f16_to_f32(0x8000).to_bits(), (-0.0f32).to_bits());
    assert_eq!(f16_to_f32(0x7bff), 65504.0);
    assert_eq!(f16_to_f32(0x0400), 2f32.powi(-14));
}

/// Rows read whole equal the numbers read one by one, padded or not, in every rank.
#[test]
fn rows_read_whole_equal_numbers_read_one_by_one() {
    // 2 x 2 x 3, rows padded to 4: the last dimension dense
    let values: Vec<f32> = (0..16).map(|v| v as f32).collect();
    let stored: Vec<u8> = values.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let got = gather(&[2, 2, 3], &[8, 4, 1], Elem::F32, &stored).unwrap();
    assert_eq!(got, vec![0., 1., 2., 4., 5., 6., 8., 9., 10., 12., 13., 14.]);
    // the same tensor stored transposed (last stride not 1) is read number by number
    let got = gather(&[3, 2], &[1, 3], Elem::F32, &stored).unwrap();
    assert_eq!(got, vec![0., 3., 1., 4., 2., 5.]);
    // a padded last row that ends past the buffer is an error
    assert!(matches!(gather(&[2, 3], &[7, 1], Elem::F32, &stored[..36]), Err(Error::Output(_))));
}

#[test]
fn padded_rows_are_read_without_the_padding() {
    // a 2 x 3 tensor whose rows Core ML padded to 4 numbers
    let stored: Vec<u8> = [1f32, 2., 3., 99., 4., 5., 6., 99.].iter().flat_map(|v| v.to_ne_bytes()).collect();
    assert_eq!(gather(&[2, 3], &[4, 1], Elem::F32, &stored).unwrap(), vec![1., 2., 3., 4., 5., 6.]);
    let halves: Vec<u8> = [0x3c00u16, 0x4000].iter().flat_map(|v| v.to_ne_bytes()).collect();
    assert_eq!(gather(&[2], &[1], Elem::F16, &halves).unwrap(), vec![1., 2.]);
}

#[test]
fn strides_past_the_buffer_are_an_error() {
    let stored = vec![0u8; 16];
    assert!(matches!(gather(&[2, 3], &[4, 1], Elem::F32, &stored), Err(Error::Output(_))));
    assert!(matches!(gather(&[2, 2], &[usize::MAX, 1], Elem::F32, &stored), Err(Error::Output(_))));
    assert!(matches!(gather(&[2, 2], &[2], Elem::F32, &stored), Err(Error::Output(_))));
}

/// A shape whose element count overflows `usize` is an error, not an overflow panic inside Core ML's byte handler.
#[test]
fn a_huge_output_shape_is_an_error() {
    let stored = vec![0u8; 16];
    let r = gather(&[usize::MAX, usize::MAX, 2], &[1, 1, 1], Elem::F32, &stored);
    assert!(matches!(r, Err(Error::Output(_))), "{r:?}");
    let r = gather(&[1 << 40, 1 << 30], &[1 << 30, 1], Elem::F16, &stored);
    assert!(matches!(r, Err(Error::Output(_))), "{r:?}");
}

#[test]
fn a_compiled_folder_replaces_the_old_one_whole() {
    let dir = scratch("replace");
    let (new, dest) = (dir.join("built"), dir.join("model.mlmodelc"));
    std::fs::create_dir_all(new.join("weights")).unwrap();
    std::fs::write(new.join("weights/weight.bin"), b"new").unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.bin"), b"old").unwrap();
    replace_dir(&new, &dest).unwrap();
    assert_eq!(std::fs::read(dest.join("weights/weight.bin")).unwrap(), b"new");
    assert!(!dest.join("stale.bin").exists() && !new.exists() && !dir.join("model.mlmodelc.part").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_model_is_an_error() {
    let dir = scratch("missing");
    assert!(matches!(Model::load(&dir.join("none.mlmodelc"), ComputeUnits::All), Err(Error::Missing(_))));
    assert!(matches!(compile(&dir.join("none.mlpackage"), &dir.join("out.mlmodelc")), Err(Error::Missing(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A damaged compiled model or package is an error from Core ML (on the Mac) or "unavailable" (elsewhere), never a
/// panic or a crash in Apple's code.
#[test]
fn a_damaged_model_is_an_error_not_a_crash() {
    let dir = scratch("damaged");
    let compiled = dir.join("junk.mlmodelc");
    std::fs::create_dir_all(compiled.join("weights")).unwrap();
    std::fs::write(compiled.join("model.mil"), b"not a model").unwrap();
    std::fs::write(compiled.join("coremldata.bin"), [0xffu8; 64]).unwrap();
    std::fs::write(compiled.join("weights/weight.bin"), [7u8; 100]).unwrap();
    let loaded = Model::load(&compiled, ComputeUnits::All);
    assert!(matches!(loaded, Err(Error::Load(_) | Error::Unavailable)), "{loaded:?}");
    let package = dir.join("junk.mlpackage");
    std::fs::create_dir_all(package.join("Data/com.apple.CoreML")).unwrap();
    std::fs::write(package.join("Manifest.json"), b"{ not json").unwrap();
    let compiled_junk = compile(&package, &dir.join("out.mlmodelc"));
    assert!(matches!(compiled_junk, Err(Error::Compile(_) | Error::Unavailable)), "{compiled_junk:?}");
    assert!(!dir.join("out.mlmodelc").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "macos")]
#[test]
fn core_ml_is_available_on_this_mac() {
    assert!(crate::available());
}

#[test]
fn a_new_manifest_is_a_new_cache_key() {
    let dir = scratch("key");
    let package = dir.join("m.mlpackage");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("Manifest.json"), br#"{"itemInfoEntries": {"A": {}}}"#).unwrap();
    let first = package_key(&package).unwrap();
    assert_eq!(package_key(&package).unwrap(), first);
    std::fs::write(package.join("Manifest.json"), br#"{"itemInfoEntries": {"B": {}}}"#).unwrap();
    assert_ne!(package_key(&package).unwrap(), first);
    assert!(matches!(package_key(&dir.join("none.mlpackage")), Err(Error::Io(_))));
    assert_eq!(key_path(&dir.join("m.mlmodelc")), dir.join("m.mlmodelc.key"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A model or weights file replaced inside a package (the manifest unchanged) is a new key: it compiled from the old
/// copy before.
#[test]
fn a_new_model_or_weights_file_is_a_new_cache_key() {
    let dir = scratch("key-files");
    let package = dir.join("m.mlpackage");
    let data = package.join("Data/com.apple.CoreML");
    std::fs::create_dir_all(data.join("weights")).unwrap();
    std::fs::write(package.join("Manifest.json"), br#"{"itemInfoEntries": {"A": {}}}"#).unwrap();
    std::fs::write(data.join("model.mlmodel"), b"model one").unwrap();
    std::fs::write(data.join("weights/weight.bin"), b"weights one").unwrap();
    let first = package_key(&package).unwrap();
    assert_eq!(package_key(&package).unwrap(), first);
    std::fs::write(data.join("weights/weight.bin"), b"weights two, longer").unwrap();
    let second = package_key(&package).unwrap();
    assert_ne!(second, first);
    std::fs::write(data.join("model.mlmodel"), b"model two, longer").unwrap();
    assert_ne!(package_key(&package).unwrap(), second);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A damaged package next to a stale compiled copy: compiling it fails, nothing is loaded, no key is left behind.
#[test]
fn a_damaged_package_is_not_loaded_from_a_stale_copy() {
    let dir = scratch("stale");
    let (package, compiled) = (dir.join("m.mlpackage"), dir.join("m.mlmodelc"));
    std::fs::create_dir_all(package.join("Data/com.apple.CoreML")).unwrap();
    std::fs::write(package.join("Manifest.json"), b"{ damaged").unwrap();
    std::fs::create_dir_all(&compiled).unwrap();
    std::fs::write(key_path(&compiled), "an older model").unwrap();
    let r = load_package(&package, &compiled, ComputeUnits::All);
    assert!(matches!(r, Err(Error::Compile(_) | Error::Unavailable)), "{r:?}");
    if crate::available() {
        assert!(!key_path(&compiled).exists());
    }
    assert!(matches!(
        load_package(&dir.join("none.mlpackage"), &dir.join("none.mlmodelc"), ComputeUnits::All),
        Err(Error::Missing(_) | Error::Unavailable)
    ));
    let _ = std::fs::remove_dir_all(&dir);
}
