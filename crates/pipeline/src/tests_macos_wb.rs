//! White balance of macOS Colour photos (plan §4, T8, T10, T10b): the Kelvin scale from macOS's as-shot white, values
//! still on the relative scale carried over exactly, and back again when the photo leaves macOS Colour.
use lightcraft_develop::{CameraColour, DevelopSettings, WbMode, WbScale};

use crate::SourceInfo;
use crate::local::{effective_wb, kelvin_to_legacy, leave_kelvin, legacy_to_kelvin, wb_matrix_for};

/// macOS's as-shot white of an A7 V photo under tungsten (`plan/colour/edit/aswhite.txt`: 3970 K).
const APPLE: (f64, f64) = (3970.0, 4.0);

/// A macOS Colour source (`files::macos::source_info`).
fn macos() -> SourceInfo {
    SourceInfo { raw: true, as_shot_temp: APPLE.0, as_shot_tint: APPLE.1, relative_wb: false, legacy_relative_wb: true, ..SourceInfo::default() }
}

/// The same photo on Camera Matching: the relative scale (6500 K / 0 = as shot).
fn relative() -> SourceInfo {
    SourceInfo { raw: true, as_shot_temp: 6500.0, as_shot_tint: 0.0, relative_wb: true, ..SourceInfo::default() }
}

fn wb(mode: WbMode, temp: f64, tint: f64, scale: WbScale, colour: CameraColour) -> DevelopSettings {
    let mut d = DevelopSettings::default();
    (d.wb.mode, d.wb.temp, d.wb.tint, d.wb.scale) = (mode, temp, tint, scale);
    d.profile.camera_colour = colour;
    d
}

/// `kelvin_to_legacy` undoes `legacy_to_kelvin` (within 1 K and 0.1 tint), and As Shot is 6500 K / 0 exactly.
#[test]
fn kelvin_to_legacy_inverts_legacy_to_kelvin() {
    assert_eq!(kelvin_to_legacy(APPLE.0, APPLE.1, APPLE), (6500.0, 0.0));
    for (t, tint) in [(5000.0, 0.0), (7000.0, 5.0), (9000.0, -20.0), (3500.0, 30.0), (12000.0, 10.0)] {
        let k = legacy_to_kelvin(t, tint, APPLE);
        let (back_t, back_tint) = kelvin_to_legacy(k.0, k.1, APPLE);
        assert!((back_t - t).abs() < 1.0 && (back_tint - tint).abs() < 0.1, "{t} {tint} → {k:?} → {back_t} {back_tint}");
    }
    assert_eq!(kelvin_to_legacy(f64::NAN, 0.0, APPLE).0.to_bits(), f64::NAN.to_bits());
}

/// T8 / T10b: on macOS Colour, As Shot is macOS's as-shot white whatever numbers are stored (6500 K / 0 from Camera
/// Matching is never read as Kelvin), and a Custom white still on the relative scale renders as its Kelvin
/// composition; neither is rewritten.
#[test]
fn relative_values_render_as_their_kelvin_on_macos_colour() {
    let as_shot = wb(WbMode::AsShot, 6500.0, 0.0, WbScale::Legacy, CameraColour::Macos);
    assert_eq!(effective_wb(&macos(), &as_shot), APPLE);
    assert!(wb_matrix_for(&macos(), &as_shot).is_none(), "as shot: no adaptation");
    let custom = wb(WbMode::Custom, 7000.0, 5.0, WbScale::Legacy, CameraColour::Macos);
    assert_eq!(effective_wb(&macos(), &custom), legacy_to_kelvin(7000.0, 5.0, APPLE));
    // a relative Custom 6500 K / 0 (As Shot left as a number) stays as shot
    let left = wb(WbMode::Custom, 6500.0, 0.0, WbScale::Legacy, CameraColour::Macos);
    assert_eq!(effective_wb(&macos(), &left), APPLE);
    // on the way back to Camera Matching the stored values are what they were
    let back = DevelopSettings { profile: Default::default(), ..custom.clone() };
    assert_eq!(effective_wb(&relative(), &back), (7000.0, 5.0));
}

/// T10: Custom white balance round trip. Set on Camera Matching (relative), shown on macOS Colour as its Kelvin, edited
/// there (now Kelvin), and carried back by `leave_kelvin`: the same relative white within 1 K, and it renders as the
/// Kelvin did.
#[test]
fn a_custom_white_round_trips_through_macos_colour() {
    let k = legacy_to_kelvin(7000.0, 5.0, APPLE);
    let mut d = wb(WbMode::Custom, k.0, k.1, WbScale::Kelvin, CameraColour::Macos);
    leave_kelvin(&mut d, APPLE);
    assert_eq!((d.wb.mode, d.wb.scale), (WbMode::Custom, WbScale::Legacy));
    assert!((d.wb.temp - 7000.0).abs() < 1.0 && (d.wb.tint - 5.0).abs() < 0.1, "{:?}", d.wb);
    // a preset set on macOS Colour becomes the Custom value of its Kelvin; As Shot and relative values stay
    let mut daylight = wb(WbMode::Daylight, 0.0, 0.0, WbScale::Kelvin, CameraColour::Macos);
    leave_kelvin(&mut daylight, APPLE);
    let preset = WbMode::Daylight.preset().unwrap();
    assert_eq!((daylight.wb.mode, daylight.wb.scale), (WbMode::Custom, WbScale::Legacy));
    assert_eq!((daylight.wb.temp, daylight.wb.tint), kelvin_to_legacy(preset.0, preset.1, APPLE));
    for keep in
        [wb(WbMode::AsShot, 3970.0, 4.0, WbScale::Kelvin, CameraColour::Macos), wb(WbMode::Custom, 7000.0, 5.0, WbScale::Legacy, CameraColour::Macos)]
    {
        let mut d = keep.clone();
        leave_kelvin(&mut d, APPLE);
        assert_eq!(d, keep);
    }
}

/// Off the Mac (or a file macOS cannot read) a macOS Colour photo renders as Camera Matching on the relative scale: a
/// white set there in Kelvin renders as shot, not as a relative 4 000 K, and is kept.
#[test]
fn kelvin_set_on_a_mac_renders_as_shot_elsewhere() {
    let d = wb(WbMode::Custom, 4200.0, 3.0, WbScale::Kelvin, CameraColour::Macos);
    assert_eq!(effective_wb(&relative(), &d), (6500.0, 0.0));
    assert!(wb_matrix_for(&relative(), &d).is_none());
    // the same values on a photo not set to macOS Colour are read as before
    let other = wb(WbMode::Custom, 4200.0, 3.0, WbScale::Kelvin, CameraColour::Auto);
    assert_eq!(effective_wb(&relative(), &other), (4200.0, 3.0));
}
