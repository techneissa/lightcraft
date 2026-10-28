//! AI Denoise on macOS Colour with the Mac's own decoder, on a real file (plan T25d). `LIGHTCRAFT_MACRAW_ARW` names an
//! A7 V ARW (as for `lightcraft_macraw`'s Mac-only tests); without it the test is skipped. Nothing of macOS's output is
//! stored: the test measures the transfer against a fresh render.
use std::borrow::Cow;

use lightcraft_develop::CameraColour;
use lightcraft_raster::Rgb32f;

use super::super::load::load_bytes_now;
use super::tests::options;
use super::{GAIN_STOPS, System, base};
use crate::camera_preview::colour_transfer::{apple_blocks, de2000, display_lab};
use crate::media::DenoiseSpec;

/// The block size the pictures are compared on (the plan's score: 24-pixel blocks at 2560 px).
const BLOCK: usize = 24;

fn sample() -> Option<Vec<u8>> {
    let Some(path) = std::env::var_os("LIGHTCRAFT_MACRAW_ARW") else {
        lightcraft_testkit::skip!(Input, "set LIGHTCRAFT_MACRAW_ARW to an A7 V ARW file");
        return None;
    };
    std::fs::read(path).ok()
}

/// A denoise product that does not exist: the pair path runs (the transfer is fitted and applied) without a
/// denoised picture.
fn no_product() -> DenoiseSpec {
    DenoiseSpec { product: std::env::temp_dir().join("lightcraft-no-such-product.lcdn"), key: "none".into(), make: None }
}

/// The transferred plain picture (unscaled: the fixed gain taken off when the tone fit was refused) and macOS's own
/// picture, both at `edge`, upright, and whether the transfer was accepted.
pub(super) fn pair(bytes: &[u8], edge: usize) -> (Rgb32f, Rgb32f, bool) {
    let (img, _, info, _) = load_bytes_now(Cow::Borrowed(bytes), edge, Some(&no_product()), false, options(CameraColour::Macos), &System).unwrap();
    let accepted = info.macos_note.is_none();
    let img = if info.camera_tone.is_none() {
        let gain = 2f32.powf(GAIN_STOPS);
        Rgb32f { data: img.data.iter().map(|p| p.map(|v| v / gain)).collect(), ..img }
    } else {
        img
    };
    let info = lightcraft_raw::probe_info(bytes).unwrap();
    let apple = base(bytes, &info, edge, &System).unwrap().image;
    (img, apple, accepted)
}

/// Per-block ΔE00 of `a` against macOS's `apple` on [`BLOCK`]-pixel blocks, through the plan's display mapping (macOS's
/// median luminance at 0.18), over blocks that are not dark on macOS's side.
pub(super) fn block_errors(a: &Rgb32f, apple: &Rgb32f) -> Vec<f64> {
    let (cols, rows) = (apple.width / BLOCK, apple.height / BLOCK);
    let (pa, pb) = (apple_blocks(a, cols, rows), apple_blocks(apple, cols, rows));
    let luma = |p: &[f32; 3]| f64::from(lightcraft_color::luminance_2020(*p));
    let mut ys: Vec<f64> = pb.data.iter().filter(|p| p[0].is_finite()).map(luma).collect();
    ys.sort_by(f64::total_cmp);
    let k = 0.18 / ys.get(ys.len() / 2).copied().unwrap_or(0.18).max(1e-6);
    pa.data
        .iter()
        .zip(&pb.data)
        .filter(|(x, y)| x[0].is_finite() && y[0].is_finite())
        .map(|(x, y)| de2000(display_lab(x.map(f64::from), k), display_lab(y.map(f64::from), k)))
        .collect()
}

/// `blocks` as `MBS1`: u32 width, u32 height, 4 f32 unused, then width × height × 3 f32, little endian.
fn write_blocks(path: &std::path::Path, blocks: &Rgb32f) {
    let mut b = b"MBS1".to_vec();
    b.extend_from_slice(&(blocks.width as u32).to_le_bytes());
    b.extend_from_slice(&(blocks.height as u32).to_le_bytes());
    b.extend_from_slice(&[0u8; 16]);
    for p in &blocks.data {
        for v in p {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    std::fs::write(path, b).unwrap();
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

/// T25d: on a real A7 V file, LightCraft's develop carried over by the transfer is within a median 1 ΔE00 of macOS's
/// own picture.
#[test]
fn a_real_transfer_reproduces_macos_colours() {
    let Some(bytes) = sample() else { return };
    let (img, apple, accepted) = pair(&bytes, 1600);
    assert!(accepted, "the transfer was refused");
    assert_eq!((img.width, img.height), (apple.width, apple.height));
    let errors = block_errors(&img, &apple);
    let med = median(errors);
    eprintln!("transfer against macOS's picture: median ΔE00 {med:.2}");
    assert!(med <= 1.0, "median ΔE00 {med:.2}");
}

/// The measuring kit for the plan's 68-photo score (run on request, `--ignored`): for every ARW listed in the file
/// `LIGHTCRAFT_TRANSFER_LIST`, writes into `LIGHTCRAFT_TRANSFER_OUT` the transferred picture and macOS's, averaged to
/// [`BLOCK`]-pixel blocks at 2560 px (`MBS1`, u32 width, u32 height, 4 f32 unused, then width × height × 3 f32, little
/// endian), plus `<name>.clip`, one byte per block, 1 where LightCraft's camera RGB holds a clipped sample.
#[test]
#[ignore = "measuring kit: needs LIGHTCRAFT_TRANSFER_LIST and LIGHTCRAFT_TRANSFER_OUT"]
fn transfer_score_kit() {
    let (Some(list), Some(out)) = (std::env::var_os("LIGHTCRAFT_TRANSFER_LIST"), std::env::var_os("LIGHTCRAFT_TRANSFER_OUT")) else { return };
    let out = std::path::PathBuf::from(out);
    for path in std::fs::read_to_string(list).unwrap().lines().filter(|l| !l.trim().is_empty()) {
        let name = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::read(path).unwrap();
        let (img, apple, accepted) = pair(&bytes, 2560);
        let (cols, rows) = (apple.width / BLOCK, apple.height / BLOCK);
        let write = |suffix: &str, blocks: &Rgb32f| write_blocks(&out.join(format!("{name}.{suffix}.mbs")), blocks);
        let mean = |img: &Rgb32f| super::colour_transfer::means(img, cols, rows);
        write("tr", &mean(&img));
        write("ap", &mean(&apple));
        // which blocks hold a clipped camera sample (as the fit leaves them out)
        let raw = lightcraft_raw::decode(&bytes).unwrap();
        let k = crate::files::colour::bin_factor(&raw, 2560).unwrap_or(1);
        let clipped = match raw.develop_binned_masked(k, 0.95).unwrap() {
            Some((img, mask)) => Rgb32f { data: mask.iter().map(|&m| [if m != 0 { 1.0 } else { 0.0 }; 3]).collect(), ..img },
            None => Rgb32f { data: vec![], width: 0, height: 0 },
        }
        .into_oriented(raw.orientation);
        let clipped = lightcraft_raster::resample::fit(&clipped, apple.width, apple.height, lightcraft_raster::resample::Filter::Box);
        let clip = mean(&clipped).data.iter().map(|p| u8::from(p[0] != 0.0)).collect::<Vec<u8>>();
        std::fs::write(out.join(format!("{name}.clip")), clip).unwrap();
        let errors = block_errors(&img, &apple);
        eprintln!("{name}: accepted {accepted}, median ΔE00 {:.2}", median(errors));
    }
}

/// The plan's score on a real AI-denoised picture: `LIGHTCRAFT_DENOISE_PRODUCT` names the `.lcdn` product AI Denoise
/// made for the file in `LIGHTCRAFT_MACRAW_ARW` (its name is its key). The denoised picture, carried over by the
/// transfer fitted on the plain one, is within a median 1 ΔE00 of macOS's own picture at 2560 px. With
/// `LIGHTCRAFT_TRANSFER_OUT` the three pictures' blocks are written there (`dn`, `tr`, `ap`).
#[test]
fn a_real_denoised_picture_keeps_macos_colours() {
    let Some(product) = std::env::var_os("LIGHTCRAFT_DENOISE_PRODUCT").map(std::path::PathBuf::from) else {
        lightcraft_testkit::skip!(Input, "set LIGHTCRAFT_DENOISE_PRODUCT to an AI Denoise product of LIGHTCRAFT_MACRAW_ARW");
        return;
    };
    let Some(bytes) = sample() else { return };
    let key = product.file_stem().unwrap().to_string_lossy().into_owned();
    let spec = DenoiseSpec { product, key, make: None };
    let (plain, twin, info, _) = load_bytes_now(Cow::Borrowed(&bytes), 2560, Some(&spec), false, options(CameraColour::Macos), &System).unwrap();
    assert!(info.macos_note.is_none(), "{:?}", info.macos_note);
    let twin = twin.expect("the product fits this raw");
    let unscale = |img: Rgb32f| match info.camera_tone {
        Some(_) => img,
        None => Rgb32f { data: img.data.iter().map(|p| p.map(|v| v / 2f32.powf(GAIN_STOPS))).collect(), ..img },
    };
    let (plain, twin) = (unscale(plain), unscale(twin));
    let probe = lightcraft_raw::probe_info(&bytes).unwrap();
    let apple = base(&bytes, &probe, 2560, &System).unwrap().image;
    let p95 = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() * 95 / 100).copied().unwrap_or(f64::NAN)
    };
    let (dn, tr) = (block_errors(&twin, &apple), block_errors(&plain, &apple));
    eprintln!(
        "denoised vs macOS: median {:.2} p95 {:.2}; plain vs macOS: median {:.2} p95 {:.2}; tone fit {}",
        median(dn.clone()),
        p95(dn.clone()),
        median(tr.clone()),
        p95(tr),
        info.camera_tone.is_some()
    );
    if let Some(out) = std::env::var_os("LIGHTCRAFT_TRANSFER_OUT").map(std::path::PathBuf::from) {
        let (cols, rows) = (apple.width / BLOCK, apple.height / BLOCK);
        let name = std::env::var("LIGHTCRAFT_TRANSFER_NAME").unwrap_or_else(|_| "denoised".into());
        for (suffix, img) in [("dn", &twin), ("tr", &plain), ("ap", &apple)] {
            write_blocks(&out.join(format!("{name}.{suffix}.mbs")), &super::colour_transfer::means(img, cols, rows));
        }
    }
    assert!(median(dn) <= 1.0);
}

/// Step 8: on a real A7 V file's macOS Colour source, the white balance the picker reads off a tinted near-grey patch
/// (`pipeline::auto::auto_wb`), set as a Custom Kelvin white, renders that patch neutral within ±1 a* / b* (Bradford on
/// the linear base from macOS's as-shot white, `pipeline::local::wb_matrix_for`).
#[test]
fn the_white_balance_slider_neutralises_a_grey_patch() {
    use lightcraft_develop::{DevelopSettings, WbMode, WbScale};
    let Some(bytes) = sample() else { return };
    let (img, _, info, _) = load_bytes_now(Cow::Borrowed(&bytes), 1600, None, false, options(CameraColour::Macos), &System).unwrap();
    assert!(info.macos_note.is_none() && !info.relative_wb && info.legacy_relative_wb);
    let (cols, rows) = (img.width / 32, img.height / 32);
    let blocks = super::colour_transfer::means(&img, cols, rows);
    let luma = |p: &[f32; 3]| f64::from(lightcraft_color::luminance_2020(*p));
    let mut ys: Vec<f64> = blocks.data.iter().map(luma).filter(|y| y.is_finite()).collect();
    ys.sort_by(f64::total_cmp);
    let k = 0.18 / ys.get(ys.len() / 2).copied().unwrap_or(0.18).max(1e-6);
    let lab = |p: [f64; 3]| display_lab(p, k);
    // the least saturated mid-tone block that still has a visible tint (chroma 3–20)
    let chroma = |p: &[f32; 3]| {
        let l = lab(p.map(f64::from));
        (l[0], l[1].hypot(l[2]))
    };
    let (i, _) = blocks
        .data
        .iter()
        .enumerate()
        .filter(|(_, p)| p[0].is_finite() && (25.0..=85.0).contains(&chroma(p).0) && (3.0..=20.0).contains(&chroma(p).1))
        .min_by(|a, b| chroma(a.1).1.total_cmp(&chroma(b.1).1))
        .expect("a tinted near-grey patch");
    let patch = blocks.data[i];
    let before = lab(patch.map(f64::from));
    let (temp, tint) = lightcraft_pipeline::auto::auto_wb(&Rgb32f { width: 1, height: 1, data: vec![patch] }, &info);
    let mut d = DevelopSettings::default();
    (d.wb.mode, d.wb.temp, d.wb.tint, d.wb.scale) = (WbMode::Custom, temp, tint, WbScale::Kelvin);
    let m = lightcraft_pipeline::local::wb_matrix_for(&info, &d).expect("a white away from as shot");
    let p = patch.map(f64::from);
    let out: [f64; 3] = std::array::from_fn(|r| (0..3).map(|c| f64::from(m[r][c]) * p[c]).sum());
    let after = lab(out);
    eprintln!(
        "patch {i}: as shot {}K/{} → {temp}K/{tint}; a*b* {:.2},{:.2} → {:.2},{:.2}",
        info.as_shot_temp, info.as_shot_tint, before[1], before[2], after[1], after[2]
    );
    assert!(after[1].abs() <= 1.0 && after[2].abs() <= 1.0, "a*b* after: {:.2} {:.2}", after[1], after[2]);
}

/// The filesystem loader renders a macOS Colour photo from its path (its own copy of the file let go first) to the same
/// picture as from the file's bytes.
#[test]
fn the_file_loader_renders_from_the_path_like_the_bytes() {
    let Some(path) = std::env::var_os("LIGHTCRAFT_MACRAW_ARW") else {
        lightcraft_testkit::skip!(Input, "set LIGHTCRAFT_MACRAW_ARW to an A7 V ARW file");
        return;
    };
    let path = std::path::PathBuf::from(path);
    let bytes = std::fs::read(&path).unwrap();
    let (from_bytes, _, a, _) = load_bytes_now(Cow::Borrowed(&bytes), 1600, None, false, options(CameraColour::Macos), &System).unwrap();
    let (from_path, _, b, _) = super::super::load::load_file(&path, bytes, 1600, None, false, options(CameraColour::Macos)).unwrap();
    assert!(a.macos_note.is_none() && b.macos_note.is_none());
    assert_eq!((from_path.width, from_path.height), (from_bytes.width, from_bytes.height));
    let worst = from_bytes.data.iter().flatten().zip(from_path.data.iter().flatten()).map(|(x, y)| (x - y).abs()).fold(0f32, f32::max);
    assert!(worst < 1e-3, "path and bytes differ by {worst}");
}
