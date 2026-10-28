//! macOS Colour in the loader, with a stand-in for the Mac's decoder (these run on every system).
use std::borrow::Cow;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lightcraft_develop::CameraColour;
use lightcraft_macraw::{Error, Input, LinearImage, Request};
use lightcraft_pipeline::MacosUnavailable;

use super::super::load::load_bytes_now;
use super::Backend;
use crate::media::LoadOptions;

/// A Sony ARW of `model`, 72 × 48, with EXIF `orientation`: an uncompressed 14-bit CFA strip in a SubIFD.
pub(crate) fn arw(model: &str, orientation: u16) -> Vec<u8> {
    use lightcraft_tiff::tags::{self as t, photometric};
    use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};
    let (w, h) = (72u32, 48u32);
    let mut raw = IfdBuilder::new();
    raw.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
    raw.set(t::IMAGE_WIDTH, Value::Long(vec![w]));
    raw.set(t::IMAGE_LENGTH, Value::Long(vec![h]));
    raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![14]));
    raw.set(t::COMPRESSION, Value::Short(vec![1]));
    raw.set(t::PHOTOMETRIC, Value::Short(vec![photometric::CFA]));
    raw.set(t::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
    raw.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 1, 2]));
    raw.set_image(ImageData::Strips { rows_per_strip: h, strips: vec![2000u16.to_le_bytes().repeat((w * h) as usize)] });
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii("SONY".into()));
    ifd0.set(t::MODEL, Value::Ascii(model.into()));
    ifd0.set(t::ORIENTATION, Value::Short(vec![orientation]));
    ifd0.add_sub_ifd(raw);
    TiffWriter::new(ByteOrder::Little, false).write(&[ifd0]).unwrap()
}

/// What the stand-in decoder answers.
#[derive(Clone)]
pub(crate) enum Answer {
    /// A grey card at the requested size (upright: the expected size scaled to the long edge) with a red marker in the
    /// top-left pixel, and this as-shot white (xy).
    Grey([f64; 2]),
    Fail(Error),
    Empty,
    /// This picture (resized to the request), with this as-shot white.
    Picture(lightcraft_raster::Rgb32f, [f64; 2]),
}

pub(crate) struct Fake {
    pub available: bool,
    pub listed: bool,
    pub answer: Answer,
    pub calls: AtomicUsize,
    pub requests: Mutex<Vec<Request>>,
    /// The file the load reads ([`Backend::path`]), and how each render was given the raw ("path" or "bytes").
    pub path: Option<std::path::PathBuf>,
    pub inputs: Mutex<Vec<&'static str>>,
    /// The load's cancel flag ([`Backend::cancel`]).
    pub cancel: AtomicBool,
}

impl Fake {
    pub(crate) fn new(answer: Answer) -> Fake {
        Fake {
            available: true,
            listed: true,
            answer,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            path: None,
            inputs: Mutex::new(Vec::new()),
            cancel: AtomicBool::new(false),
        }
    }
}

/// The grey card's linear value.
pub(crate) const GREY: f32 = 0.18;

impl Backend for Fake {
    fn available(&self) -> bool {
        self.available
    }
    fn supports(&self, _: &str) -> bool {
        self.listed
    }
    fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
    fn cancel(&self) -> &AtomicBool {
        &self.cancel
    }
    fn render(&self, input: Input<'_>, request: &Request, _: &AtomicBool) -> Result<LinearImage, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(*request);
        let how = match input {
            Input::Bytes { uti, .. } => {
                assert_eq!(uti, "com.sony.arw-raw-image", "bytes with the exact type");
                "bytes"
            }
            Input::Path(p) => {
                assert_eq!(Some(p), self.path.as_deref(), "the load's own file");
                "path"
            }
        };
        self.inputs.lock().unwrap().push(how);
        let (ew, eh) = request.expected_size.unwrap();
        let (w, h) = match request.long_edge {
            Some(e) if e < ew.max(eh) => {
                let s = f64::from(e) / f64::from(ew.max(eh));
                ((f64::from(ew) * s).round() as u32, (f64::from(eh) * s).round() as u32)
            }
            _ => (ew, eh),
        };
        match &self.answer {
            Answer::Grey(white) => {
                let mut rgb = vec![[GREY; 3]; (w * h) as usize];
                rgb[0] = [1.0, 0.0, 0.0];
                Ok(LinearImage {
                    width: w,
                    height: h,
                    rgb,
                    white_xy: *white,
                    white_kelvin: 0.0,
                    white_tint: 0.0,
                    native_size: (ew, eh),
                    decoder_version: "8".into(),
                })
            }
            Answer::Picture(img, white) => {
                let img = if (img.width, img.height) == (w as usize, h as usize) {
                    img.clone()
                } else {
                    lightcraft_raster::resample::fit(img, w as usize, h as usize, lightcraft_raster::resample::Filter::Box)
                };
                Ok(LinearImage {
                    width: img.width as u32,
                    height: img.height as u32,
                    rgb: img.data,
                    white_xy: *white,
                    white_kelvin: 0.0,
                    white_tint: 0.0,
                    native_size: (ew, eh),
                    decoder_version: "8".into(),
                })
            }
            Answer::Fail(e) => Err(e.clone()),
            Answer::Empty => Ok(LinearImage {
                width: 0,
                height: 0,
                rgb: Vec::new(),
                white_xy: [0.3457, 0.3585],
                white_kelvin: 0.0,
                white_tint: 0.0,
                native_size: (0, 0),
                decoder_version: "8".into(),
            }),
        }
    }
}

pub(crate) fn options(colour: CameraColour) -> LoadOptions {
    LoadOptions { colour, ..LoadOptions::default() }
}

pub(crate) fn load(bytes: &[u8], max_edge: usize, colour: CameraColour, backend: &dyn Backend) -> super::super::load::Loaded {
    load_bytes_now(Cow::Borrowed(bytes), max_edge, None, false, options(colour), backend).unwrap()
}

/// D50's chromaticity: about 5000 K.
pub(crate) const D50: [f64; 2] = [0.3457, 0.3585];

/// T4: a photo on macOS Colour is the decoder's picture (with the fixed gain: this file has no camera JPEG to fit the
/// tone to, T6), its as-shot white is the
/// decoder's white in Kelvin, it has no camera colour model and is edited on the Kelvin scale.
#[test]
fn a_macos_photo_is_the_decoders_picture() {
    let bytes = arw("ILCE-7M5", 1);
    let fake = Fake::new(Answer::Grey(D50));
    let (img, twin, info, _) = load(&bytes, 4096, CameraColour::Macos, &fake);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
    assert_eq!((img.width, img.height), (72, 48));
    let gain = 2f32.powf(super::GAIN_STOPS);
    assert!((img.data[1][0] - GREY * gain).abs() < 1e-6, "{:?}", img.data[1]);
    assert!(twin.is_none());
    let (t, tint) = lightcraft_color::cct::xy_to_temp_tint(lightcraft_color::Xy::new(D50[0], D50[1]));
    assert_eq!((info.as_shot_temp, info.as_shot_tint), (t.round(), tint.round()));
    assert!((info.as_shot_temp - 5003.0).abs() < 10.0, "{}", info.as_shot_temp);
    assert!(info.raw && !info.relative_wb && info.legacy_relative_wb && info.camera_color.is_none());
    assert!(info.macos_offered && info.macos_note.is_none() && info.colorimetric);
    let request = fake.requests.lock().unwrap()[0];
    assert_eq!(request, Request { long_edge: None, draft: false, expected_size: Some((72, 48)) });
}

/// The same photo on Camera Matching never asks the decoder for a picture, and says macOS Colour is offered.
#[test]
fn camera_matching_never_renders_but_offers_macos() {
    let bytes = arw("ILCE-7M5", 1);
    let fake = Fake::new(Answer::Grey(D50));
    let (_, _, info, _) = load(&bytes, 4096, CameraColour::Auto, &fake);
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert!(info.macos_offered && info.macos_note.is_none());
    let unlisted = Fake { listed: false, ..Fake::new(Answer::Grey(D50)) };
    assert!(!load(&bytes, 4096, CameraColour::Auto, &unlisted).2.macos_offered);
}

/// A thumbnail asks for the decoder's downsample at its size, a preview for its long edge.
#[test]
fn smaller_sources_ask_for_their_long_edge() {
    let bytes = arw("ILCE-7M5", 1);
    let fake = Fake::new(Answer::Grey(D50));
    let (thumb, ..) = load(&bytes, 24, CameraColour::Macos, &fake);
    let (preview, ..) = load(&bytes, 60, CameraColour::Macos, &fake);
    assert_eq!(((thumb.width, thumb.height), (preview.width, preview.height)), ((24, 16), (60, 40)));
    let requests = fake.requests.lock().unwrap().clone();
    assert_eq!(requests[0], Request { long_edge: Some(24), draft: true, expected_size: Some((72, 48)) });
    assert_eq!(requests[1].long_edge, Some(60));
}

/// T7: a portrait raw asks for its upright size, and the decoder's upright picture is not turned again.
#[test]
fn a_portrait_is_not_rotated_twice() {
    let bytes = arw("ILCE-7M5", 6);
    let fake = Fake::new(Answer::Grey(D50));
    let (img, ..) = load(&bytes, 4096, CameraColour::Macos, &fake);
    assert_eq!(fake.requests.lock().unwrap()[0].expected_size, Some((48, 72)));
    assert_eq!((img.width, img.height), (48, 72));
    let gain = 2f32.powf(super::GAIN_STOPS);
    assert_eq!(img.data[0], [gain, 0.0, 0.0], "the marker stays in the top-left corner");
}

/// T5: every way the decoder can fail gives the Camera Matching source, says why, and is not retried for that file.
#[test]
fn a_failed_decode_falls_back_once_and_says_why() {
    let cases = [
        ("ILCE-T5A", Answer::Fail(Error::Undecodable), MacosUnavailable::Unreadable),
        ("ILCE-T5B", Answer::Fail(Error::SizeMismatch { apple: (74, 48), expected: (72, 48) }), MacosUnavailable::Size),
        ("ILCE-T5C", Answer::Fail(Error::Os("no context".into())), MacosUnavailable::Unreadable),
        ("ILCE-T5D", Answer::Empty, MacosUnavailable::Unreadable),
        ("ILCE-T5E", Answer::Grey([f64::NAN, 0.33]), MacosUnavailable::Unreadable),
    ];
    for (model, answer, why) in cases {
        let bytes = arw(model, 1);
        let fake = Fake::new(answer);
        let (img, _, info, _) = load(&bytes, 4096, CameraColour::Macos, &fake);
        let (auto_img, _, auto_info, _) = load(&bytes, 4096, CameraColour::Auto, &Fake::new(Answer::Grey(D50)));
        assert_eq!(img, auto_img, "{model}: the Camera Matching picture");
        assert_eq!((info.as_shot_temp, info.as_shot_tint, info.relative_wb), (auto_info.as_shot_temp, auto_info.as_shot_tint, auto_info.relative_wb));
        assert_eq!(info.macos_note, Some(why), "{model}");
        assert!(!info.macos_offered, "{model}: not offered once it failed");
        let (_, _, again, _) = load(&bytes, 512, CameraColour::Macos, &fake);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1, "{model}: remembered, not rendered again");
        assert_eq!(again.macos_note, Some(why));
    }
}

/// Not a Mac (or no decoder), or a camera macOS does not list: Camera Matching with the reason, nothing rendered.
#[test]
fn no_decoder_or_unlisted_camera_falls_back_without_rendering() {
    let bytes = arw("ILCE-7M5", 1);
    let off = Fake { available: false, ..Fake::new(Answer::Grey(D50)) };
    let unlisted = Fake { listed: false, ..Fake::new(Answer::Grey(D50)) };
    for (fake, why) in [(&off, MacosUnavailable::System), (&unlisted, MacosUnavailable::Camera)] {
        let (img, _, info, _) = load(&bytes, 4096, CameraColour::Macos, fake);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        assert_eq!(info.macos_note, Some(why));
        assert_eq!(img, load(&bytes, 4096, CameraColour::Auto, fake).0);
    }
}

/// The real decoder off the Mac: a macOS Colour photo renders as Camera Matching and says the system cannot.
#[cfg(not(target_os = "macos"))]
#[test]
fn off_the_mac_the_system_decoder_falls_back() {
    let bytes = arw("ILCE-7M5", 1);
    let (_, info) = crate::files::load_bytes_with(&bytes, 4096, options(CameraColour::Macos)).unwrap();
    assert_eq!(info.macos_note, Some(MacosUnavailable::System));
}

/// T16d (engine side): a macOS Colour load weighs Core Image's peak footprint at the memory gate, only where the
/// decoder exists; other loads keep their own weight.
#[test]
fn a_macos_load_weighs_the_decoders_footprint() {
    use crate::files::fs::load_weight;
    let floor = lightcraft_macraw::FLOOR_BYTES as usize;
    let a7v = arw("ILCE-7M5", 1);
    assert_eq!(load_weight(250_000_000, &a7v, 1600, options(CameraColour::Macos), true), Some(floor));
    assert_eq!(load_weight(2_000_000_000, &a7v, 1600, options(CameraColour::Macos), true), Some(2_000_000_000));
    assert_eq!(load_weight(250_000_000, b"not a raw", usize::MAX, options(CameraColour::Macos), true), Some(floor), "the floor without a header");
    assert_eq!(load_weight(250_000_000, &a7v, 1600, options(CameraColour::Macos), false), None);
    assert_eq!(load_weight(250_000_000, &a7v, 1600, options(CameraColour::Auto), true), None);
}

/// T6: an accepted tone fit keeps the decoder's values (its curve maps them); a refused one scales them by the fixed
/// gain, once.
#[test]
fn the_gain_is_applied_only_when_the_tone_fit_is_refused() {
    let base = lightcraft_raster::Rgb32f { width: 2, height: 1, data: vec![[GREY; 3], [1.0, 0.5, 0.0]] };
    let tone = lightcraft_pipeline::tone::CameraTone::new(std::array::from_fn(|i| {
        let x = 2f32.powf(-12.0 + 12.0 * i as f32 / 31.0);
        [x, x.sqrt().min(0.999)]
    }))
    .unwrap();
    let look = crate::camera_preview::CameraLook { matrix: lightcraft_color::Mat3::IDENTITY, tone, hue_sat: None };
    let (kept, fitted) = super::toned(base.clone(), Some(look));
    assert_eq!((kept, fitted), (base.clone(), Some(tone)));
    let (scaled, none) = super::toned(base, None);
    let gain = 2f32.powf(super::GAIN_STOPS);
    assert!(none.is_none());
    assert_eq!(scaled.data, vec![[GREY * gain; 3], [gain, 0.5 * gain, 0.0]]);
}
