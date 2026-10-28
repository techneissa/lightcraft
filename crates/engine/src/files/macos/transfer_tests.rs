//! AI Denoise on macOS Colour in the loader (plan §6.6, T22–T25b), with a stand-in for the Mac's decoder.
use std::borrow::Cow;

use lightcraft_develop::CameraColour;
use lightcraft_macraw::Request;
use lightcraft_pipeline::MacosUnavailable;
use lightcraft_raster::Rgb32f;

use super::super::load::{Loaded, load_bytes_now};
use super::tests::{Answer, D50, Fake, arw, load, options};
use super::{GAIN_STOPS, TRANSFER_EDGE, request};
use crate::media::DenoiseSpec;

/// A Sony ARW of `model`, 72 × 48, whose RGGB mosaic holds 6 × 6 tiles of different colours.
fn arw_tiles(model: &str) -> Vec<u8> {
    use lightcraft_tiff::tags::{self as t, photometric};
    use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};
    let (w, h) = (72usize, 48usize);
    let mut rng = crate::camera_preview::colour_transfer::tests::Lcg(7);
    let tiles: Vec<[f64; 3]> = (0..12 * 8).map(|_| [rng.unit(), rng.unit(), rng.unit()]).collect();
    let mut data = Vec::with_capacity(w * h * 2);
    for y in 0..h {
        for x in 0..w {
            let colour = [0, 1, 1, 2][(y % 2) * 2 + x % 2];
            let v = 1500.0 + 7000.0 * tiles[(y / 6) * 12 + x / 6][colour];
            data.extend_from_slice(&(v as u16).to_le_bytes());
        }
    }
    let mut raw = IfdBuilder::new();
    raw.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
    raw.set(t::IMAGE_WIDTH, Value::Long(vec![w as u32]));
    raw.set(t::IMAGE_LENGTH, Value::Long(vec![h as u32]));
    raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![14]));
    raw.set(t::COMPRESSION, Value::Short(vec![1]));
    raw.set(t::PHOTOMETRIC, Value::Short(vec![photometric::CFA]));
    raw.set(t::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
    raw.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 1, 2]));
    raw.set_image(ImageData::Strips { rows_per_strip: h as u32, strips: vec![data] });
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii("SONY".into()));
    ifd0.set(t::MODEL, Value::Ascii(model.into()));
    ifd0.set(t::ORIENTATION, Value::Short(vec![1]));
    ifd0.add_sub_ifd(raw);
    TiffWriter::new(ByteOrder::Little, false).write(&[ifd0]).unwrap()
}

/// A denoise product of `bytes` that is its plain develop (a "denoised" picture equal to the plain one).
fn product_of(bytes: &[u8], name: &str) -> (DenoiseSpec, std::path::PathBuf) {
    let dir = crate::tests_xmp::temp_dir(name);
    let raw = lightcraft_raw::decode(bytes).unwrap();
    let camera = raw.develop(lightcraft_raw::Method::Ahd).unwrap();
    let path = dir.join("plain.lcdn");
    lightcraft_denoise::product::write(&path, &camera, "k").unwrap();
    (DenoiseSpec { product: path, key: "k".into(), make: None }, dir)
}

fn load_pair(bytes: &[u8], spec: &DenoiseSpec, colour: CameraColour, backend: &dyn super::Backend) -> Loaded {
    load_bytes_now(Cow::Borrowed(bytes), 4096, Some(spec), false, options(colour), backend).unwrap()
}

fn worst(a: &Rgb32f, b: &Rgb32f) -> f32 {
    a.data.iter().zip(&b.data).flat_map(|(p, q)| p.iter().zip(q).map(|(x, y)| (x - y).abs())).fold(0.0, f32::max)
}

/// T23: with AI Denoise on, a macOS Colour photo's plain and denoised pictures are LightCraft's develop carried over
/// to macOS's colours by one transfer (equal pictures stay equal, so the Amount slider blends one colour), close to
/// macOS's own picture but not it (T25: under 1 ΔE00), with the facts of a macOS base. T25b: without the product the photo is macOS's
/// picture again.
#[test]
fn a_denoised_macos_photo_is_carried_over_to_macos_colours() {
    let bytes = arw_tiles("ILCE-7M5");
    // macOS's picture: the Camera Matching picture, darker (any colours a white-keeping matrix and a gain reach)
    let (camera_matching, ..) = load(&bytes, 4096, CameraColour::Auto, &Fake::new(Answer::Grey(D50)));
    let apple = Rgb32f { data: camera_matching.data.iter().map(|p| p.map(|v| v * 0.8)).collect(), ..camera_matching.clone() };
    let fake = Fake::new(Answer::Picture(apple.clone(), D50));
    let (spec, dir) = product_of(&bytes, "macos-transfer");
    let (plain, twin, info, _) = load_pair(&bytes, &spec, CameraColour::Macos, &fake);
    let twin = twin.expect("the denoised picture");
    assert_eq!(fake.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(info.macos_note.is_none() && info.macos_offered && info.camera_color.is_none() && !info.relative_wb, "{info:?}");
    let (t, _) = lightcraft_color::cct::xy_to_temp_tint(lightcraft_color::Xy::new(D50[0], D50[1]));
    assert_eq!(info.as_shot_temp, t.round(), "Kelvin from macOS's white");
    // no camera JPEG: the tone fit is refused and the fixed gain is on both pictures, once
    assert!(info.camera_tone.is_none());
    let gain = 2f32.powf(GAIN_STOPS);
    let unscaled = Rgb32f { data: plain.data.iter().map(|p| p.map(|v| v / gain)).collect(), ..plain.clone() };
    assert!(worst(&unscaled, &apple) < 0.01, "macOS's colours: {}", worst(&unscaled, &apple));
    // T25: the colour step from macOS's picture to the carried-over one, as a display shows it
    use crate::camera_preview::colour_transfer::{de2000, display_lab};
    let lab = |p: &[f32; 3]| display_lab(p.map(f64::from), 1.0);
    let step = unscaled.data.iter().zip(&apple.data).map(|(p, q)| de2000(lab(p), lab(q))).fold(0.0, f64::max);
    assert!(step < 1.0, "T25: colour step {step:.3} ΔE00");
    assert!(worst(&plain, &twin) < 0.002, "one transfer for both: {}", worst(&plain, &twin));
    let (direct, ..) = load(&bytes, 4096, CameraColour::Macos, &fake);
    assert_ne!(plain, direct, "LightCraft's develop, not macOS's picture");
    assert_eq!(direct.data, apple.data.iter().map(|p| p.map(|v| v * gain)).collect::<Vec<_>>(), "T25b: without AI Denoise, macOS's picture");
    let _ = std::fs::remove_dir_all(dir);
}

/// T24: the transfer's picture of macOS's is at most [`TRANSFER_EDGE`] long, whatever size the photo is loaded at.
#[test]
fn the_transfer_asks_macos_for_a_small_picture() {
    assert_eq!(request((7008, 4672), TRANSFER_EDGE), Request { long_edge: Some(1024), draft: false, expected_size: Some((7008, 4672)) });
    let bytes = arw_tiles("ILCE-7M5");
    let fake = Fake::new(Answer::Grey(D50));
    let (spec, dir) = product_of(&bytes, "macos-transfer-small");
    let _ = load_pair(&bytes, &spec, CameraColour::Macos, &fake);
    let requests = fake.requests.lock().unwrap().clone();
    assert_eq!(requests, vec![request((72, 48), TRANSFER_EDGE)], "one small render, no full-size one");
    let _ = std::fs::remove_dir_all(dir);
}

/// T22 (loader side): a refused transfer (one colour: nothing tells the matrix apart), or macOS failing, leaves both
/// pictures on Camera Matching, and the photo says why.
#[test]
fn a_refused_transfer_keeps_both_pictures_on_camera_matching() {
    let flat = arw("ILCE-7M5", 1);
    let (spec, dir) = product_of(&flat, "macos-transfer-refused");
    let fake = Fake::new(Answer::Grey(D50));
    let (plain, twin, info, _) = load_pair(&flat, &spec, CameraColour::Macos, &fake);
    let (cm_plain, cm_twin, cm_info, _) = load_pair(&flat, &spec, CameraColour::Auto, &fake);
    assert_eq!(info.macos_note, Some(MacosUnavailable::Transfer));
    assert_eq!((plain, twin), (cm_plain, cm_twin), "Camera Matching for both");
    assert_eq!((info.as_shot_temp, info.relative_wb), (cm_info.as_shot_temp, cm_info.relative_wb));

    let tiles = arw_tiles("ILCE-T22F");
    let (spec2, dir2) = product_of(&tiles, "macos-transfer-failed");
    let failing = Fake::new(Answer::Fail(lightcraft_macraw::Error::Undecodable));
    let (plain, twin, info, _) = load_pair(&tiles, &spec2, CameraColour::Macos, &failing);
    let (cm_plain, cm_twin, ..) = load_pair(&tiles, &spec2, CameraColour::Auto, &failing);
    assert_eq!(info.macos_note, Some(MacosUnavailable::Unreadable));
    assert_eq!((plain, twin), (cm_plain, cm_twin));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(dir2);
}
