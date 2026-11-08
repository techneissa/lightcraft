use super::*;

#[test]
fn rows_show_values_the_way_they_are_typed() {
    let mut m = Meta {
        camera: "SONY ILCE-7M5".into(),
        focal_mm: Some(35.0),
        aperture: Some(2.8),
        shutter: "1/250".into(),
        iso: Some(400),
        ..Default::default()
    };
    assert_eq!(display(ExifField::Camera, &m, false), "SONY ILCE-7M5");
    assert_eq!(display(ExifField::Focal, &m, false), "35 mm");
    assert_eq!(display(ExifField::Aperture, &m, false), "f/2.8");
    assert_eq!(display(ExifField::Shutter, &m, false), "1/250 s");
    assert_eq!(display(ExifField::Iso, &m, false), "400");
    assert_eq!(display(ExifField::Lens, &m, false), "", "no value, no text");
    // what a row shows reads back through the same parsers the command uses
    m.set_exif(ExifField::Focal, Some("70.5mm")).unwrap();
    for f in ExifField::ALL {
        let shown = display(f, &m, false);
        let mut copy = m.clone();
        assert!(copy.set_exif(f, Some(&shown)).is_ok(), "{f:?}: `{shown}`");
        assert_eq!(copy.exif, m.exif, "{f:?}: showing and retyping a value changes nothing");
    }
    assert_eq!(display(ExifField::Focal, &m, true), "35 mm", "the file's value stays available");
}

#[test]
fn a_prime_lens_name_gives_its_focal_length() {
    for (name, mm) in [
        ("Sirui Aurora 35mm f/1.4", 35.0),
        ("Laowa 100mm f/2.8 2x Ultra Macro APO", 100.0),
        ("85mm", 85.0),
        ("7Artisans 50mm f/1.8", 50.0),
        ("35mm f/1.4 (manual)", 35.0),
        ("Laowa 60mm f/2.8 2X Ultra-Macro", 60.0),
        ("RF50mm F1.2 L USM", 50.0),
        ("Zeiss 50 mm T1.5 1.33x", 50.0),
        ("Samyang 12mm f/2 NCS CS", 12.0),
    ] {
        assert_eq!(prime_focal_mm(name), Some(mm), "{name}");
    }
}

#[test]
fn a_zoom_or_a_name_without_a_focal_length_gives_none() {
    for name in [
        "FE 24-70mm F2.8 GM",
        "70-200mm f/2.8",
        "24mm-70mm",
        "24 – 105mm",
        "Sigma 18-35mm F1.8",
        "",
        "Kenko 1.4x Teleplus",
        "Lens f/2.8 T1.5",
        "mm",
        "0mm",
    ] {
        assert_eq!(prime_focal_mm(name), None, "{name}");
    }
}
