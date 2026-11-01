//! Edited camera and exposure values in sidecars: the packet carries only overridden fields,
//! reads them back as overrides, a reset removes them from an existing sidecar, and hostile
//! values are skipped.

use super::*;
use crate::sidecar::{merge_into, parse_sidecar, sidecar_contents, sidecar_packet};
use lightcraft_catalog::{Catalog, Photo, PhotoId, Source};

fn photo(file: impl FnOnce(&mut Meta)) -> Photo {
    let mut p = Photo::new(PhotoId(1), Source::File { path: "/x/IMG_1.jpg".into() }, "IMG_1.jpg", "JPEG", 40, 30, "2026-01-01T00:00:00");
    file(&mut p.meta);
    p
}

/// The file's own values: what the camera recorded.
fn sony(m: &mut Meta) {
    m.camera = "SONY ILCE-7M5".into();
    m.lens = "FE 24-70mm F2.8 GM".into();
    m.focal_mm = Some(24.0);
    m.aperture = Some(2.8);
    m.shutter = "1/250".into();
    m.iso = Some(100);
}

fn packet(p: &Photo) -> String {
    sidecar_packet(p, &Catalog::new())
}

fn read_back(file_values: &Photo, xmp: &str) -> Photo {
    let sc = parse_sidecar(xmp, crate::crs::Target::Rendered).unwrap();
    let mut q = file_values.clone();
    q.meta.exif = ExifOverrides::default();
    merge_into(&mut q, &sc, "now");
    q
}

fn edit_all(p: &mut Photo) {
    for (f, v) in [
        (ExifField::Camera, "Canon EOS R5"),
        (ExifField::Lens, "RF24-70mm F2.8 L IS USM"),
        (ExifField::Focal, "35"),
        (ExifField::Aperture, "f/4"),
        (ExifField::Shutter, "2.5\""),
        (ExifField::Iso, "1600"),
    ] {
        p.meta.set_exif(f, Some(v)).unwrap();
    }
}

#[test]
fn camera_names_split_into_make_and_model_and_join_back() {
    assert_eq!(split_camera("Sony ILCE-7M5"), (Some("Sony".into()), "ILCE-7M5".into()));
    assert_eq!(split_camera("Canon EOS R5"), (Some("Canon".into()), "EOS R5".into()));
    assert_eq!(split_camera("Phase One IQ4 150MP"), (Some("Phase One".into()), "IQ4 150MP".into()));
    assert_eq!(split_camera("phase one IQ4"), (Some("phase one".into()), "IQ4".into()));
    assert_eq!(split_camera("X100V"), (None, "X100V".into()));
    assert_eq!(split_camera("Ünï Cam 1"), (Some("Ünï".into()), "Cam 1".into()));
}

#[test]
fn only_overridden_fields_are_written() {
    let mut p = photo(sony);
    let x = packet(&p);
    for tag in
        ["tiff:Make", "tiff:Model", "aux:Lens", "exif:FocalLength", "exif:FNumber", "exif:ExposureTime", "exif:ISOSpeedRatings", "exifOverrides"]
    {
        assert!(!x.contains(tag), "no override, no {tag}:\n{x}");
    }
    p.meta.set_exif(ExifField::Iso, Some("800")).unwrap();
    p.meta.set_exif(ExifField::Aperture, Some("2")).unwrap();
    let x = packet(&p);
    assert!(x.contains("<exif:FNumber>2/1</exif:FNumber>"), "{x}");
    assert!(x.contains("<rdf:li>800</rdf:li>") && x.contains("<exifEX:PhotographicSensitivity>800<"), "{x}");
    assert!(x.contains("<lc:exifOverrides>aperture,iso</lc:exifOverrides>"), "{x}");
    for tag in ["tiff:Make", "tiff:Model", "aux:Lens", "exifEX:LensModel", "exif:FocalLength", "exif:ExposureTime"] {
        assert!(!x.contains(tag), "{tag} was not edited:\n{x}");
    }
}

#[test]
fn all_six_fields_write_the_standard_tags() {
    let mut p = photo(sony);
    edit_all(&mut p);
    let x = packet(&p);
    for want in [
        "<tiff:Make>Canon</tiff:Make>",
        "<tiff:Model>EOS R5</tiff:Model>",
        "<aux:Lens>RF24-70mm F2.8 L IS USM</aux:Lens>",
        "<exifEX:LensModel>RF24-70mm F2.8 L IS USM</exifEX:LensModel>",
        "<exif:FocalLength>35/1</exif:FocalLength>",
        "<exif:FNumber>4/1</exif:FNumber>",
        "<exif:ExposureTime>25/10</exif:ExposureTime>",
        "<exif:ISOSpeedRatings>",
        "<exifEX:PhotographicSensitivity>1600</exifEX:PhotographicSensitivity>",
        "<lc:exifOverrides>camera,lens,focal,aperture,shutter,iso</lc:exifOverrides>",
    ] {
        assert!(x.contains(want), "missing {want}:\n{x}");
    }
}

#[test]
fn write_then_read_in_a_fresh_catalog_gives_the_same_overrides() {
    let mut p = photo(sony);
    edit_all(&mut p);
    // a catalog that has the file's values and no edits reads the sidecar
    let q = read_back(&photo(sony), &packet(&p));
    assert_eq!(q.meta.exif, p.meta.exif);
    assert_eq!(q.meta.camera, "SONY ILCE-7M5", "the file's values stay");
}

#[test]
fn each_field_alone_round_trips() {
    for (field, text) in [
        (ExifField::Camera, "Nikon Z 8"),
        (ExifField::Camera, "OneWord"),
        (ExifField::Lens, "Laowa 15mm f/2 Zero-D"),
        (ExifField::Focal, "85.5"),
        (ExifField::Aperture, "1.4"),
        (ExifField::Aperture, "5.66"),
        (ExifField::Shutter, "1/8000"),
        (ExifField::Shutter, "1/3"),
        (ExifField::Shutter, "30"),
        (ExifField::Iso, "64"),
        (ExifField::Iso, "102400"),
    ] {
        let mut p = photo(sony);
        p.meta.set_exif(field, Some(text)).unwrap();
        assert!(p.meta.exif.has(field), "{field:?} {text}");
        let q = read_back(&photo(sony), &packet(&p));
        assert_eq!(q.meta.exif, p.meta.exif, "{field:?} {text}");
    }
}

#[test]
fn a_file_with_no_values_takes_the_sidecars_as_overrides() {
    let mut p = photo(sony);
    edit_all(&mut p);
    let q = read_back(&photo(|m| m.lens = "----".into()), &packet(&p));
    assert_eq!(q.meta.exif, p.meta.exif);
}

#[test]
fn values_equal_to_the_files_are_no_override() {
    // another app's sidecar that merely repeats the file's EXIF
    let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:tiff="http://ns.adobe.com/tiff/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/"
        xmlns:aux="http://ns.adobe.com/exif/1.0/aux/" tiff:Make="SONY" tiff:Model="ILCE-7M5" aux:Lens="FE 24-70mm F2.8 GM">
        <exif:FocalLength>240/10</exif:FocalLength><exif:FNumber>28/10</exif:FNumber><exif:ExposureTime>1/250</exif:ExposureTime>
        <exif:ISOSpeedRatings><rdf:Seq><rdf:li>100</rdf:li></rdf:Seq></exif:ISOSpeedRatings>
      </rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let q = read_back(&photo(sony), x);
    assert!(q.meta.exif.is_empty(), "{:?}", q.meta.exif);
    // the file's 24.04 mm and f/2.7999 count as 24 and 2.8
    let q = read_back(
        &photo(|m| {
            sony(m);
            m.focal_mm = Some(24.04);
            m.aperture = Some(2.7999);
        }),
        x,
    );
    assert!(q.meta.exif.is_empty(), "{:?}", q.meta.exif);
}

#[test]
fn a_sidecar_stating_the_files_value_clears_an_override() {
    let mut p = photo(sony);
    p.meta.set_exif(ExifField::Iso, Some("800")).unwrap();
    p.meta.set_exif(ExifField::Lens, Some("Mine")).unwrap();
    let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/"><exif:ISOSpeedRatings><rdf:Seq><rdf:li>100</rdf:li></rdf:Seq></exif:ISOSpeedRatings></rdf:Description>
      </rdf:RDF></x:xmpmeta>"#;
    let sc = parse_sidecar(x, crate::crs::Target::Rendered).unwrap();
    merge_into(&mut p, &sc, "now");
    assert_eq!((p.meta.exif.iso, p.meta.exif.lens.as_deref()), (None, Some("Mine")), "ISO is stated and equal; the lens is not stated");
}

#[test]
fn another_apps_sidecar_with_the_tags_reads_without_error() {
    let x = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:tiff="http://ns.adobe.com/tiff/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:exifEX="http://cipa.jp/exif/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    tiff:Make="Leica Camera AG" tiff:Model="LEICA Q3" exifEX:LensModel="Summilux 28 f/1.7" crs:Exposure2012="+0.50">
   <exif:FocalLength>28/1</exif:FocalLength>
   <exif:ExposureTime>1/2000</exif:ExposureTime>
   <exif:FNumber>17/10</exif:FNumber>
   <exif:ISOSpeedRatings><rdf:Seq><rdf:li>400</rdf:li></rdf:Seq></exif:ISOSpeedRatings>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;
    let q = read_back(&photo(sony), x);
    assert_eq!(
        q.meta.exif,
        ExifOverrides {
            camera: Some("Leica Camera AG LEICA Q3".into()),
            lens: Some("Summilux 28 f/1.7".into()),
            focal_mm: Some(28.0),
            aperture: Some(1.7),
            shutter: Some("1/2000".into()),
            iso: Some(400),
        }
    );
}

/// An existing sidecar a save is merged into.
fn save_over(path: &std::path::Path, p: &Photo) -> String {
    let (data, merged, _) = sidecar_contents(path, packet(p), "t", false).unwrap();
    assert!(merged || !path.exists());
    std::fs::write(path, &data).unwrap();
    data
}

fn temp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-exifxmp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("IMG_1.xmp")
}

#[test]
fn a_reset_field_is_removed_from_the_sidecar_and_others_stay() {
    let path = temp("reset");
    let mut p = photo(sony);
    edit_all(&mut p);
    save_over(&path, &p);
    p.meta.reset_exif(ExifField::Lens);
    p.meta.reset_exif(ExifField::Camera);
    let x = save_over(&path, &p);
    for gone in ["tiff:Make", "tiff:Model", "aux:Lens", "exifEX:LensModel", "RF24-70mm"] {
        assert!(!x.contains(gone), "{gone} should be gone:\n{x}");
    }
    assert!(x.contains("<lc:exifOverrides>focal,aperture,shutter,iso</lc:exifOverrides>"), "{x}");
    assert_eq!(x.matches("<exif:FocalLength>").count(), 1, "{x}");
    // the sidecar now reads back with just the four
    let q = read_back(&photo(sony), &x);
    assert_eq!((q.meta.exif.camera.clone(), q.meta.exif.lens.clone()), (None, None));
    assert_eq!((q.meta.exif.focal_mm, q.meta.exif.iso), (Some(35.0), Some(1600)));
    // and with every field reset, no tag and no marker is left
    p.meta.exif = ExifOverrides::default();
    let x = save_over(&path, &p);
    for gone in ["exif:FocalLength", "exif:FNumber", "exif:ExposureTime", "ISOSpeedRatings", "PhotographicSensitivity", "exifOverrides"] {
        assert!(!x.contains(gone), "{gone} should be gone:\n{x}");
    }
    assert!(read_back(&photo(sony), &x).meta.exif.is_empty());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn saving_leaves_another_apps_exif_tags_alone_when_nothing_is_edited() {
    let path = temp("foreign");
    let theirs = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/" tiff:Model="THEIRS">
        <exif:FNumber>56/10</exif:FNumber></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    std::fs::write(&path, theirs).unwrap();
    let mut p = photo(sony);
    p.rating = 3;
    let x = save_over(&path, &p);
    assert!(x.contains("tiff:Model=\"THEIRS\"") && x.contains("56/10"), "{x}");
    // editing the ISO replaces only the ISO tags
    p.meta.set_exif(ExifField::Iso, Some("200")).unwrap();
    let x = save_over(&path, &p);
    assert!(x.contains("tiff:Model=\"THEIRS\"") && x.contains("56/10") && x.contains(">200<"), "{x}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn an_edit_replaces_the_foreign_tags_of_that_field_and_a_one_word_camera_drops_the_make() {
    let path = temp("replace");
    let theirs = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/" tiff:Make="Canon" tiff:Model="EOS R5">
        <exif:FNumber>56/10</exif:FNumber></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    std::fs::write(&path, theirs).unwrap();
    let mut p = photo(sony);
    p.meta.set_exif(ExifField::Camera, Some("X100V")).unwrap();
    let x = save_over(&path, &p);
    assert!(!x.contains("Canon") && !x.contains("EOS R5") && x.contains("<tiff:Model>X100V<"), "{x}");
    let q = read_back(&photo(sony), &x);
    assert_eq!(q.meta.exif.camera.as_deref(), Some("X100V"));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

fn hostile(props: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
        xmlns:exifEX="http://cipa.jp/exif/1.0/" xmlns:aux="http://ns.adobe.com/exif/1.0/aux/">{props}</rdf:Description>
      </rdf:RDF></x:xmpmeta>"#
    )
}

#[test]
fn malformed_values_are_skipped_never_a_panic() {
    let huge = "x".repeat(100_000);
    let long_digits = "9".repeat(5_000);
    let cases = [
        "<exif:ISOSpeedRatings><rdf:Seq><rdf:li>-100</rdf:li></rdf:Seq></exif:ISOSpeedRatings>".to_string(),
        "<exif:ISOSpeedRatings><rdf:Seq><rdf:li>0</rdf:li></rdf:Seq></exif:ISOSpeedRatings>".to_string(),
        "<exifEX:PhotographicSensitivity>99999999999999999999</exifEX:PhotographicSensitivity>".to_string(),
        "<exifEX:PhotographicSensitivity>5000000</exifEX:PhotographicSensitivity>".to_string(),
        "<exifEX:PhotographicSensitivity>abc</exifEX:PhotographicSensitivity>".to_string(),
        "<exif:ExposureTime>1/0</exif:ExposureTime>".to_string(),
        "<exif:ExposureTime>0/0</exif:ExposureTime>".to_string(),
        "<exif:ExposureTime>-1/250</exif:ExposureTime>".to_string(),
        "<exif:ExposureTime>1e999</exif:ExposureTime>".to_string(),
        "<exif:ExposureTime>1e-30</exif:ExposureTime>".to_string(),
        "<exif:ExposureTime>999999999</exif:ExposureTime>".to_string(),
        "<exif:FNumber>0/0</exif:FNumber>".to_string(),
        "<exif:FNumber>-2.8</exif:FNumber>".to_string(),
        "<exif:FNumber>NaN</exif:FNumber>".to_string(),
        "<exif:FNumber>inf</exif:FNumber>".to_string(),
        "<exif:FNumber>100000</exif:FNumber>".to_string(),
        "<exif:FocalLength>-35</exif:FocalLength>".to_string(),
        "<exif:FocalLength>1e300</exif:FocalLength>".to_string(),
        "<exif:FocalLength>99999999</exif:FocalLength>".to_string(),
        format!("<exif:FocalLength>{long_digits}</exif:FocalLength>"),
        format!("<exif:FNumber>{long_digits}.5</exif:FNumber>"),
        format!("<exif:ExposureTime>1/{long_digits}</exif:ExposureTime>"),
        format!("<aux:Lens>{huge}</aux:Lens>"),
        format!("<exifEX:LensModel>{huge}</exifEX:LensModel>"),
        format!("<tiff:Model>{huge}</tiff:Model>"),
        format!("<tiff:Make>{huge}</tiff:Make><tiff:Model>M</tiff:Model>"),
        "<tiff:Model>bad\u{1}name</tiff:Model>".to_string(),
        "<aux:Lens>----</aux:Lens>".to_string(),
        "<tiff:Model>  </tiff:Model><tiff:Make>Only Make</tiff:Make>".to_string(),
    ];
    for props in cases {
        let x = hostile(&props);
        // (the packet itself may be refused as XML, which is an error value, never a panic)
        let Ok(sc) = parse_sidecar(&x, crate::crs::Target::Rendered) else { continue };
        let mut q = photo(sony);
        q.meta.set_exif(ExifField::Iso, Some("800")).unwrap();
        merge_into(&mut q, &sc, "now");
        let overrides = q.meta.exif.clone();
        let short: String = props.chars().take(70).collect();
        assert_eq!(overrides, ExifOverrides { iso: Some(800), ..Default::default() }, "{short}");
    }
}

#[test]
fn a_hostile_override_in_the_catalog_still_writes_a_packet() {
    // (an override the commands refuse could still come from a damaged catalog file)
    let mut p = photo(sony);
    p.meta.exif = ExifOverrides {
        camera: Some(" ".into()),
        lens: Some(String::new()),
        focal_mm: Some(f32::NAN),
        aperture: Some(f32::INFINITY),
        shutter: Some("1/0".into()),
        iso: Some(u32::MAX),
    };
    let x = packet(&p);
    let sc = parse_sidecar(&x, crate::crs::Target::Rendered).unwrap();
    let mut q = photo(sony);
    merge_into(&mut q, &sc, "now");
    assert!(q.meta.exif.focal_mm.is_none() && q.meta.exif.aperture.is_none() && q.meta.exif.shutter.is_none());
}

/// A sidecar stating only an exposure time.
fn shutter_sidecar(time: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/"><exif:ExposureTime>{time}</exif:ExposureTime></rdf:Description>
      </rdf:RDF></x:xmpmeta>"#
    )
}

#[test]
fn a_long_exposure_the_file_stores_rounded_is_no_override() {
    // the catalog keeps a file's 2.5 s as `2` and its 0.7 s as `1/1` (files/probe.rs); the sidecar says the real time
    for (file, time) in [("2", "5/2"), ("2", "2.5"), ("1/1", "7/10"), ("1/1", "0.7"), ("30", "30/1"), ("1/250", "1/250"), ("1/3", "0.333")] {
        let q = read_back(&photo(|m| m.shutter = file.into()), &shutter_sidecar(time));
        assert!(q.meta.exif.is_empty(), "file {file}, sidecar {time}: {:?}", q.meta.exif);
    }
    // and one that clears an override the person had set
    let mut p = photo(|m| m.shutter = "2".into());
    p.meta.set_exif(ExifField::Shutter, Some("1/60")).unwrap();
    merge_into(&mut p, &parse_sidecar(&shutter_sidecar("5/2"), crate::crs::Target::Rendered).unwrap(), "now");
    assert!(p.meta.exif.is_empty());
}

#[test]
fn a_different_exposure_is_an_override() {
    for (file, time, want) in [("2", "1/250", "1/250"), ("1/1", "5/2", "2.5"), ("1/250", "2.5", "2.5"), ("1/250", "1/500", "1/500")] {
        let q = read_back(&photo(|m| m.shutter = file.into()), &shutter_sidecar(time));
        assert_eq!(q.meta.exif.shutter.as_deref(), Some(want), "file {file}, sidecar {time}");
    }
}
