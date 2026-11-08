use super::*;

fn limits(lens: &str) -> (Option<(f32, f32)>, Option<f32>) {
    let l = LensLimits::of(lens);
    (l.focal_mm, l.max_aperture)
}

#[test]
fn common_lens_names_give_their_limits() {
    assert_eq!(limits("16-35mm f/4"), (Some((16.0, 35.0)), Some(4.0)));
    assert_eq!(limits("50mm f/1.8"), (Some((50.0, 50.0)), Some(1.8)));
    assert_eq!(limits("FE 50mm F1.8"), (Some((50.0, 50.0)), Some(1.8)));
    assert_eq!(limits("18-55mm F3.5-5.6"), (Some((18.0, 55.0)), Some(3.5)), "a variable aperture is limited by its smallest f-number");
    assert_eq!(limits("70-200mm F2.8 GM"), (Some((70.0, 200.0)), Some(2.8)));
    assert_eq!(limits("FE 24-70mm F2.8 GM II"), (Some((24.0, 70.0)), Some(2.8)));
    assert_eq!(limits("RF24-70mm F2.8 L IS USM"), (Some((24.0, 70.0)), Some(2.8)));
    assert_eq!(limits("Canon EF 50mm 1:1.8 II"), (Some((50.0, 50.0)), Some(1.8)));
    assert_eq!(limits("Sony FE 100-400mm F4.5-5.6 GM OSS"), (Some((100.0, 400.0)), Some(4.5)));
    assert_eq!(limits("24 – 70 mm  f / 2.8"), (Some((24.0, 70.0)), Some(2.8)), "dashes and spaces");
    assert_eq!(limits("85MM F1.4"), (Some((85.0, 85.0)), Some(1.4)), "any case");
}

#[test]
fn a_name_that_says_nothing_sets_no_limit() {
    for lens in ["", "   ", "Mystery lens", "FE", "Zeiss Otus", "f/", "mm", "50", "Lens 5.6"] {
        assert_eq!(limits(lens), (None, None), "{lens:?}");
    }
    // one half can be there without the other
    assert_eq!(limits("35mm"), (Some((35.0, 35.0)), None));
    assert_eq!(limits("Soft Focus f/2.5"), (None, Some(2.5)));
}

#[test]
fn hostile_names_never_panic_and_set_no_limit() {
    for lens in [
        "f/0",
        "F0.0",
        "0mm",
        "0-0mm f/0",
        "f/99999999999999999999",
        "99999999999999999999mm",
        "1e400mm f/1e400",
        "-5mm f/-2",
        "16-mm",
        "mm-35",
        "f/-",
        "f/.",
        "f/2.",
        "1:",
        "ＦＥ ５０ｍｍ Ｆ１．８",
        "レンズ 50mm f/1.8 😀",
        "\u{0}\u{0}f/\u{0}",
        "f/1.8.8",
    ] {
        let l = LensLimits::of(lens);
        if let Some((lo, hi)) = l.focal_mm {
            assert!(lo >= 1.0 && hi <= 5000.0 && lo <= hi, "{lens:?}: {l:?}");
        }
        if let Some(f) = l.max_aperture {
            assert!((0.3..=256.0).contains(&f), "{lens:?}: {l:?}");
        }
    }
    assert_eq!(limits("f/0"), (None, None));
    assert_eq!(limits("0mm"), (None, None));
    assert_eq!(limits("f/99999999999999999999"), (None, None));
    assert_eq!(limits("ＦＥ ５０ｍｍ Ｆ１．８"), (None, None), "full-width digits are not read");
    assert_eq!(limits("レンズ 50mm f/1.8 😀"), (Some((50.0, 50.0)), Some(1.8)), "non-ASCII text around a spec is fine");
    let long = format!("{}50mm f/1.8", "あ".repeat(10_000));
    assert_eq!(LensLimits::of(&long), LensLimits::default(), "only the start of a name is read");
}

#[test]
fn an_aperture_below_the_limit_is_refused_with_a_reason() {
    let l = LensLimits::of("16-35mm f/4");
    assert_eq!(l.aperture_refusal(2.8).map(|r| r.to_string()).as_deref(), Some("This lens opens to f/4 at most"));
    assert_eq!(l.aperture_refusal(4.0), None);
    assert_eq!(l.aperture_refusal(5.6), None);
    assert_eq!(LensLimits::of("18-55mm F3.5-5.6").aperture_refusal(3.0).map(|r| r.to_string()).as_deref(), Some("This lens opens to f/3.5 at most"));
    assert_eq!(LensLimits::of("Mystery").aperture_refusal(0.5), None);
}

#[test]
fn a_focal_length_outside_the_range_is_refused_with_a_reason() {
    let l = LensLimits::of("16-35mm f/4");
    assert_eq!(l.focal_refusal(50.0).map(|r| r.to_string()).as_deref(), Some("This lens covers 16–35 mm"));
    assert_eq!(l.focal_refusal(10.0).map(|r| r.to_string()).as_deref(), Some("This lens covers 16–35 mm"));
    assert_eq!((l.focal_refusal(16.0), l.focal_refusal(35.0), l.focal_refusal(24.0)), (None, None, None));
    assert_eq!(LensLimits::of("50mm f/1.8").focal_refusal(35.0).map(|r| r.to_string()).as_deref(), Some("This lens is 50 mm"));
    assert_eq!(LensLimits::of("50mm f/1.8").focal_refusal(50.0), None);
    assert_eq!(LensLimits::of("").focal_refusal(1234.0), None);
}

#[test]
fn only_an_edited_value_is_checked_against_the_lens_shown() {
    let mut m = Meta { lens: "16-35mm f/4".into(), focal_mm: Some(50.0), aperture: Some(2.8), ..Default::default() };
    // the file's own values are shown as they are
    assert_eq!((refusal(&m, ExifField::Focal), refusal(&m, ExifField::Aperture)), (None, None));
    // an edit outside the lens is refused; one inside is not
    m.exif.aperture = Some(3.5);
    m.exif.focal_mm = Some(24.0);
    assert!(refusal(&m, ExifField::Aperture).is_some());
    assert_eq!(refusal(&m, ExifField::Focal), None);
    // the lens it is checked against is the edited lens
    m.set_exif(ExifField::Lens, Some("24-70mm f/2.8")).unwrap();
    assert_eq!((refusal(&m, ExifField::Aperture), refusal(&m, ExifField::Focal)), (None, None));
    m.set_exif(ExifField::Lens, Some("70-200mm F2.8")).unwrap();
    assert!(refusal(&m, ExifField::Focal).is_some(), "a lens change leaves the edit, and now it is out of range");
    assert_eq!(refusal(&m, ExifField::Iso), None);
}

#[test]
fn a_teleconverter_scales_the_limits() {
    let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 0.01);
    let l = LensLimits::of("100-400mm f/4.5-5.6 + 1.4x");
    assert!(l.focal_mm.is_some_and(|(lo, hi)| (lo - 140.0).abs() < 0.1 && (hi - 560.0).abs() < 0.1), "{l:?}");
    assert!(near(l.max_aperture, 6.3), "{l:?}");
    assert_eq!(l.focal_refusal(560.0), None, "560 mm is what the lens gives with its converter");
    assert_eq!(l.aperture_refusal(6.3), None);
    assert!(l.aperture_refusal(4.5).is_some(), "f/4.5 is gone with the converter");
    let l = LensLimits::of("300mm f/2.8 + EF1.4x");
    assert_eq!(l.focal_refusal(420.0), None);
    assert!(near(l.max_aperture, 3.92), "{l:?}");
    for (name, focal, aperture) in [
        ("70-200mm f/2.8 + 2x", (140.0, 400.0), 5.6),
        ("70-200mm F2.8 GM OSS with TC-20E", (140.0, 400.0), 5.6),
        ("Nikkor 200-500mm f/5.6E + TC-14E III", (280.0, 700.0), 7.84),
        ("Canon EF 100-400mm f/4.5-5.6L IS II USM + Extender EF 1.4x III", (140.0, 560.0), 6.3),
        ("400mm f/2.8 + x1.4", (560.0, 560.0), 3.92),
        ("400mm f/2.8 + 1.4X", (560.0, 560.0), 3.92),
        ("400mm f/2.8 + 1,4x", (560.0, 560.0), 3.92),
    ] {
        let l = LensLimits::of(name);
        assert!(l.focal_mm.is_some_and(|(lo, hi)| (lo - focal.0).abs() < 0.2 && (hi - focal.1).abs() < 0.2), "{name}: {l:?}");
        assert!(near(l.max_aperture, aperture), "{name}: {l:?}");
    }
}

#[test]
fn a_teleconverter_with_no_readable_factor_sets_no_limit() {
    for name in [
        "70-200mm f/2.8 + Extender",
        "70-200mm f/2.8 with teleconverter",
        "300mm f/2.8 + TC",
        "300mm f/2.8 + 0x",
        "300mm f/2.8 + 99x",
        "300mm f/2.8 + ",
        "70-200mm f/2.8 + 1.4",
    ] {
        assert_eq!(LensLimits::of(name), LensLimits::default(), "{name}");
    }
    // a "+" in a name with no converter mentioned is covered by the same rule: better no limit than a wrong one
    assert_eq!(limits("FE 24-70mm F2.8 GM"), (Some((24.0, 70.0)), Some(2.8)), "no converter, no scaling");
}

#[test]
fn a_decimal_comma_reads_as_a_point() {
    assert_eq!(limits("Summilux 35 mm f/1,4"), (Some((35.0, 35.0)), Some(1.4)));
    assert_eq!(limits("Summicron 50mm F2,0"), (Some((50.0, 50.0)), Some(2.0)));
    assert_eq!(limits("Zoom 18-55mm F3,5-5,6"), (Some((18.0, 55.0)), Some(3.5)));
    assert_eq!(limits("Lens 8,5mm f/1,8"), (Some((8.5, 8.5)), Some(1.8)));
    // a comma between two things is not a decimal point
    assert_eq!(limits("Lens 50mm,f/1.8"), (Some((50.0, 50.0)), Some(1.8)));
}
