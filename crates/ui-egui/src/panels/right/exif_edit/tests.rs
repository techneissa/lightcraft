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
