//! The kept camera-look fit (issue #220): a second decode of a raw reuses its fit and gives the same source as a
//! decode with nothing kept; what the fit reads besides the file (the Profile choice, outlier removal, the file's
//! bytes) gives another fit.

use super::{fitted, forget, kept, made};
use crate::files::load_bytes_decoded;
use crate::media::{LoadOptions, RawDecode};

/// A 384 × 256 DNG with colour matrices but no tone curve, and a camera JPEG showing its scene through a bright camera
/// curve: its decode fits the tone to the JPEG (`camera_preview::dng_tone_eligible`). `seed` varies the scene.
fn dng_with_camera_jpeg(seed: f32) -> Vec<u8> {
    use lightcraft_color::{REC2020, SRGB, luminance_2020, transfer::linear_to_srgb};
    let (w, h) = (384usize, 256usize);
    let data = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let v = 0.02 + 0.2 * (0.5 + 0.5 * (x * 0.045 + seed).sin() * (y * 0.06).cos()) + 0.1 * x / w as f32;
            let colour = match (i / w % 2, i % w % 2) {
                (0, 0) => 1.2,
                (1, 1) => 0.8,
                _ => 1.0,
            };
            256 + (v * colour * 15744.0) as u16
        })
        .collect();
    let bytes = crate::tests_xmp::synthetic_dng_of(w, h, "RGGB", data, None, Default::default());
    let mut raw = lightcraft_raw::decode(&bytes).unwrap();
    raw.color = lightcraft_raw::ColorData {
        illuminant: [21, 0],
        color_matrix: [Some(REC2020.from_xyz()), None],
        as_shot_neutral: Some([1.0; 3]),
        ..Default::default()
    };
    let t = lightcraft_raw::color::camera_transform(&raw, lightcraft_raw::color::as_shot_white_xy(&raw));
    let developed = raw.develop(lightcraft_raw::Method::Bilinear).unwrap();
    let to_srgb = REC2020.to_space(&SRGB);
    let pixels: Vec<u8> = developed
        .data
        .iter()
        .flat_map(|p| {
            let p = t.matrix.apply_f32(std::array::from_fn(|i| p[i] * t.wb[i]));
            let y = luminance_2020(p).max(1e-6);
            let gain = (1.0 - (-3.0 * y).exp()) / y;
            to_srgb.apply_f32(p.map(|v| v * gain)).map(|v| (255.0 * linear_to_srgb(v).clamp(0.0, 1.0)).round() as u8)
        })
        .collect();
    let image = lightcraft_codecs::EncodeImage::new(w as u32, h as u32, 3, lightcraft_codecs::Samples::U8(&pixels));
    let jpeg = lightcraft_codecs::encode_jpeg(&image, 98, lightcraft_codecs::ChromaSubsampling::S444, &Default::default()).unwrap();
    let mut file = lightcraft_raw::write_dng(&raw, &Default::default()).unwrap();
    // a preview IFD after IFD0 of the little-endian DNG written above
    let root = u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(file[root..root + 2].try_into().unwrap()) as usize;
    let next = root + 2 + 12 * count;
    let start = file.len() as u32;
    file[next..next + 4].copy_from_slice(&start.to_le_bytes());
    let jpeg_at = start + 2 + 4 * 12 + 4;
    file.extend(4u16.to_le_bytes());
    for (tag, value) in [(256u16, w as u32), (257, h as u32), (0x201, jpeg_at), (0x202, jpeg.len() as u32)] {
        file.extend(tag.to_le_bytes());
        file.extend(4u16.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        file.extend(value.to_le_bytes());
    }
    file.extend(0u32.to_le_bytes());
    file.extend(&jpeg);
    file
}

/// The key the decode of `file` keeps its fit under, as `load_bytes_now` makes it.
fn key_of(file: &[u8], options: LoadOptions, outliers: bool) -> u128 {
    let raw = lightcraft_raw::decode(file).unwrap();
    let lens = crate::lens_profiles::for_raw(&raw.info(), || lightcraft_meta::extract(file));
    super::key(file, options, outliers.then(crate::files::hot_pixel_limit), false, lens.as_ref())
}

fn decode(file: &[u8], edge: usize, options: LoadOptions) -> (lightcraft_raster::Rgb32f, lightcraft_pipeline::SourceInfo) {
    load_bytes_decoded(file, edge, options, RawDecode::default()).unwrap()
}

/// #220: a raw decoded for its thumbnail and again for its loupe preview is fitted to its camera JPEG once, and the
/// second source is the one a decode with nothing kept gives, bit for bit (pixels and the fitted tone).
#[test]
fn a_second_decode_at_another_size_reuses_the_fit_and_gives_the_same_source() {
    let file = dng_with_camera_jpeg(0.3);
    let options = LoadOptions::default();
    let key = key_of(&file, options, false);
    let (_, small) = decode(&file, 128, options);
    assert!(small.camera_tone.is_some(), "the DNG's tone is fitted to its camera JPEG");
    assert!(kept(key), "the fit is kept under the file's key");
    let (kept_pixels, kept_info) = decode(&file, 300, options);
    assert_eq!(made(key), 1, "the second decode runs no fit");
    forget(key);
    let (fresh_pixels, fresh_info) = decode(&file, 300, options);
    assert_eq!(made(key), 2, "with nothing kept the decode fits again");
    assert_eq!(kept_pixels.data, fresh_pixels.data, "the same pixels with and without the kept fit");
    assert_eq!(kept_info.camera_tone, fresh_info.camera_tone, "the same tone");
    assert_eq!(kept_info.as_shot_temp.to_bits(), fresh_info.as_shot_temp.to_bits());
    assert_eq!(kept_info.as_shot_tint.to_bits(), fresh_info.as_shot_tint.to_bits());
}

/// What the fit reads besides the file is in its key: another Profile choice, another local profile file, outlier
/// removal and another file each fit again instead of reusing a fit made for something else.
#[test]
fn another_profile_choice_outlier_removal_or_file_is_fitted_again() {
    use lightcraft_develop::CameraColour;
    let file = dng_with_camera_jpeg(1.1);
    let auto = LoadOptions::default();
    let base = key_of(&file, auto, false);
    let others = [
        key_of(&file, LoadOptions { colour: CameraColour::Colorimetric, profile_key: None }, false),
        key_of(&file, LoadOptions { colour: CameraColour::Local, profile_key: Some(7) }, false),
        key_of(&file, LoadOptions { colour: CameraColour::Local, profile_key: Some(8) }, false),
        key_of(&file, auto, true),
        key_of(&dng_with_camera_jpeg(1.2), auto, false),
    ];
    for (i, k) in others.iter().enumerate() {
        assert_ne!(*k, base, "input {i} changes the key");
        assert!(others.iter().skip(i + 1).all(|o| o != k), "input {i} has a key of its own");
    }
    let colorimetric = LoadOptions { colour: CameraColour::Colorimetric, profile_key: None };
    decode(&file, 128, auto);
    decode(&file, 128, colorimetric);
    assert_eq!((made(base), made(others[0])), (1, 1), "the Profile choice changed: fitted again");
}

/// A kept fit puts back the colour tags the fit wrote into the raw (a camera without a fitted look gets its measured
/// spectral matrices), so the rest of the decode reads the same colour as after a fit; a fit that wrote none leaves
/// them alone.
#[test]
fn a_kept_fit_puts_back_the_colour_the_fit_wrote() {
    let file = dng_with_camera_jpeg(2.0);
    let fresh = || lightcraft_raw::decode(&file).unwrap();
    let written = lightcraft_raw::ColorData { as_shot_neutral: Some([0.5, 1.0, 0.7]), ..Default::default() };
    let fit = |raw: &lightcraft_raw::RawImage| {
        let xy = lightcraft_raw::color::as_shot_white_xy(raw);
        (xy, lightcraft_raw::color::camera_transform(raw, xy), None, Default::default())
    };
    let key = u128::from_le_bytes(*b"look-cache-test1");
    let mut first = fresh();
    let made_once = fitted(&mut first, Some(key), |raw| {
        raw.color = written.clone();
        fit(raw)
    });
    let mut second = fresh();
    let reused = fitted(&mut second, Some(key), |_| panic!("kept: no second fit"));
    assert_eq!(second.color, written, "the colour the fit wrote is put back");
    assert_eq!((reused.0, reused.1), (made_once.0, made_once.1));
    let untouched = u128::from_le_bytes(*b"look-cache-test2");
    let mut third = fresh();
    let own = third.color.clone();
    fitted(&mut third, Some(untouched), |raw| fit(raw));
    let mut fourth = fresh();
    fitted(&mut fourth, Some(untouched), |_| panic!("kept: no second fit"));
    assert_eq!(fourth.color, own, "a fit that wrote nothing leaves the colour as decoded");
}
