use crate::layout::{Elem, check_shape, contiguous, f16_to_f32, gather, replace_dir};
use crate::{ComputeUnits, Error, Model, compile};

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

#[test]
fn half_floats_read_right() {
    assert_eq!(f16_to_f32(0x3c00), 1.0);
    assert_eq!(f16_to_f32(0xc000), -2.0);
    assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
    assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
    assert_eq!(f16_to_f32(0x7c00), f32::INFINITY);
    assert!(f16_to_f32(0x7e00).is_nan());
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
