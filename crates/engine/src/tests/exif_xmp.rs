//! Edited camera, lens and exposure values travel in XMP sidecars: Save Metadata to File and the automatic
//! write put the overridden fields in standard tags, Read Metadata from File and an import read them back as
//! overrides (a fresh catalog gets the same ones), a reset removes the tags, and another app's sidecar, or a
//! damaged one, reads without error and without a panic.

use std::path::Path;

use lightcraft_catalog::{ExifField, ExifOverrides, PhotoId};
use serde_json::json;

use crate::Session;
use crate::tests::xmp::temp_dir;

fn write_png(path: &Path) {
    let (w, h) = (40usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 6) as u8, (i / w * 9) as u8, 7, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn only(s: &Session) -> PhotoId {
    s.catalog.photos().next().unwrap().id
}

fn exif(s: &Session, id: PhotoId) -> ExifOverrides {
    s.catalog.photo(id).unwrap().meta.exif.clone()
}

/// A library with one imported PNG (`a.png`, no EXIF of its own) in `src`.
fn library(tag: &str) -> (Session, std::path::PathBuf, std::path::PathBuf, PhotoId) {
    let src = temp_dir(&format!("{tag}-src"));
    let lib = temp_dir(&format!("{tag}-lib"));
    write_png(&src.join("a.png"));
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.join("a.png").to_string_lossy()]})).unwrap();
    let id = only(&s);
    (s, src, lib, id)
}

const EDITS: &str = r#"{"camera": "Canon EOS R5", "lens": "Laowa 15mm f/2", "focal": "15", "aperture": "f/2", "shutter": "1/500", "iso": 640}"#;

fn edit(s: &mut Session, id: PhotoId) {
    let mut p: serde_json::Value = serde_json::from_str(EDITS).unwrap();
    p["ids"] = json!([id.0]);
    s.execute("photo.setMeta", &p).unwrap();
}

#[test]
fn save_metadata_to_file_then_a_fresh_catalog_reads_the_same_overrides() {
    let (mut s, src, lib, id) = library("save");
    edit(&mut s, id);
    let edited = exif(&s, id);
    assert_eq!(edited.iso, Some(640));
    let r = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    assert_eq!(r["written"][0], src.join("a.xmp").to_string_lossy().as_ref(), "{r}");
    let x = std::fs::read_to_string(src.join("a.xmp")).unwrap();
    for tag in
        ["tiff:Make", "tiff:Model", "aux:Lens", "exifEX:LensModel", "exif:FocalLength", "exif:FNumber", "exif:ExposureTime", "exif:ISOSpeedRatings"]
    {
        assert!(x.contains(&format!("<{tag}")), "{tag} missing:\n{x}");
    }
    drop(s);

    // another library, nothing known about the photo
    std::fs::remove_dir_all(&lib).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["sidecars"], 1, "{r}");
    assert_eq!(exif(&s, only(&s)), edited, "the import takes the sidecar's values as overrides");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn the_automatic_write_follows_edits_and_resets() {
    let (mut s, src, lib, id) = library("auto");
    s.execute("library.xmpPreferences", &json!({"autoWrite": true})).unwrap();
    let sidecar = src.join("a.xmp");
    s.execute("photo.setMeta", &json!({"ids": [id.0], "iso": 1250})).unwrap();
    let x = std::fs::read_to_string(&sidecar).unwrap();
    assert!(x.contains("<exifEX:PhotographicSensitivity>1250<") && !x.contains("tiff:Model"), "{x}");
    s.execute("photo.setMeta", &json!({"ids": [id.0], "camera": "Nikon Z 8", "iso": 1600})).unwrap();
    let x = std::fs::read_to_string(&sidecar).unwrap();
    assert!(x.contains("<tiff:Make>Nikon<") && x.contains("<tiff:Model>Z 8<") && x.contains(">1600<") && !x.contains("1250"), "{x}");
    // a reset field is gone from the file (the other stays), and so is the whole set after Reset All
    s.execute("photo.resetExif", &json!({"fields": ["camera"]})).unwrap();
    let x = std::fs::read_to_string(&sidecar).unwrap();
    assert!(!x.contains("tiff:Make") && !x.contains("tiff:Model") && x.contains(">1600<"), "{x}");
    s.execute("photo.resetExif", &json!({})).unwrap();
    let x = std::fs::read_to_string(&sidecar).unwrap();
    for gone in ["tiff:Make", "tiff:Model", "ISOSpeedRatings", "PhotographicSensitivity", "exifOverrides"] {
        assert!(!x.contains(gone), "{gone}:\n{x}");
    }
    // undo of the reset writes the values back
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(std::fs::read_to_string(&sidecar).unwrap().contains(">1600<"));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn read_metadata_from_file_takes_the_values_and_undoes_in_one_step() {
    let (mut s, src, lib, id) = library("read");
    edit(&mut s, id);
    s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let saved = exif(&s, id);
    s.execute("photo.resetExif", &json!({})).unwrap();
    assert!(exif(&s, id).is_empty());
    let depth = s.undo.len();
    let r = s.execute("photo.readMetadataFromFile", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(r["read"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(exif(&s, id), saved);
    assert_eq!(s.undo.len(), depth + 1);
    s.undo_step().unwrap();
    assert!(exif(&s, id).is_empty(), "undo drops what the read added");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

const ANOTHER_APP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
    xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:aux="http://ns.adobe.com/exif/1.0/aux/" xmlns:exifEX="http://cipa.jp/exif/1.0/"
    xmp:Rating="2" tiff:Make="FUJIFILM" tiff:Model="X-T5" aux:Lens="XF16-55mmF2.8 R LM WR">
   <exif:FocalLength>350/10</exif:FocalLength>
   <exif:FNumber>28/10</exif:FNumber>
   <exif:ExposureTime>1/125</exif:ExposureTime>
   <exif:ISOSpeedRatings><rdf:Seq><rdf:li>800</rdf:li></rdf:Seq></exif:ISOSpeedRatings>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

#[test]
fn another_apps_sidecar_is_read_on_import_and_kept_by_a_later_save() {
    let src = temp_dir("foreign-src");
    let lib = temp_dir("foreign-lib");
    write_png(&src.join("a.png"));
    std::fs::write(src.join("a.xmp"), ANOTHER_APP).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["sidecars"], 1, "{r}");
    let id = only(&s);
    let m = &s.catalog.photo(id).unwrap().meta;
    assert_eq!(s.catalog.photo(id).unwrap().rating, 2);
    assert_eq!(
        (m.effective_camera(), m.effective_lens(), m.effective_focal_mm(), m.effective_aperture(), m.effective_shutter(), m.effective_iso()),
        ("FUJIFILM X-T5", "XF16-55mmF2.8 R LM WR", Some(35.0), Some(2.8), "1/125", Some(800))
    );
    // saving writes those overrides back as they were; the file stays valid and other apps' tags survive
    s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let x = std::fs::read_to_string(src.join("a.xmp")).unwrap();
    assert!(lightcraft_meta::parse_xmp(&x).is_ok(), "{x}");
    assert!(x.contains("FUJIFILM") && x.contains("X-T5") && x.contains("XF16-55mmF2.8 R LM WR"), "{x}");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn damaged_values_in_a_sidecar_import_and_read_without_error_or_panic() {
    let huge = "x".repeat(200_000);
    let hostile = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"
        xmlns:exif="http://ns.adobe.com/exif/1.0/" xmlns:aux="http://ns.adobe.com/exif/1.0/aux/" xmp:Rating="4" tiff:Model="{huge}">
       <exif:ISOSpeedRatings><rdf:Seq><rdf:li>-400</rdf:li></rdf:Seq></exif:ISOSpeedRatings>
       <exif:ExposureTime>1/0</exif:ExposureTime><exif:FNumber>NaN</exif:FNumber><exif:FocalLength>-24</exif:FocalLength>
       <aux:Lens>{huge}</aux:Lens>
      </rdf:Description></rdf:RDF></x:xmpmeta>"#
    );
    let src = temp_dir("hostile-src");
    let lib = temp_dir("hostile-lib");
    write_png(&src.join("a.png"));
    std::fs::write(src.join("a.xmp"), &hostile).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().unwrap().len(), 1, "{r}");
    let id = only(&s);
    assert_eq!(s.catalog.photo(id).unwrap().rating, 4, "the rest of the sidecar is still read");
    assert!(exif(&s, id).is_empty(), "{:?}", exif(&s, id));
    // the same on a later read, over an existing edit
    s.execute("photo.setMeta", &json!({"ids": [id.0], "iso": 200})).unwrap();
    s.execute("photo.readMetadataFromFile", &json!({"ids": [id.0]})).unwrap();
    assert_eq!(exif(&s, id).iso, Some(200), "an unusable ISO in the sidecar leaves the edit alone");
    let _ = (ExifField::Iso, std::fs::remove_dir_all(&src), std::fs::remove_dir_all(&lib));
}

/// The background writer (the desktop's automatic write) puts the edits in the sidecar and takes a reset out of it.
#[test]
fn the_background_writer_writes_edits_and_removes_resets() {
    use lightcraft_catalog::{Op, Photo, Source};
    let dir = temp_dir("queue");
    let path = dir.join("IMG_1.jpg");
    std::fs::write(&path, b"jpeg").unwrap();
    let mut s = Session::new();
    let p = Photo::new(PhotoId(1), Source::File { path: path.to_string_lossy().to_string() }, "IMG_1.jpg", "JPEG", 4, 3, "2026-10-28");
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    s.xmp.auto_write = true;
    s.queue_sidecar_writes(true);
    s.execute("library.select", &json!({"ids": [1]})).unwrap();
    s.execute("photo.setMeta", &json!({"lens": "Laowa 15mm f/2", "iso": 640})).unwrap();
    s.finish_sidecar_writes();
    let x = std::fs::read_to_string(dir.join("IMG_1.xmp")).unwrap();
    assert!(x.contains("<aux:Lens>Laowa 15mm f/2<") && x.contains("<lc:exifOverrides>lens,iso<"), "{x}");
    s.execute("photo.resetExif", &json!({"fields": ["lens"]})).unwrap();
    s.finish_sidecar_writes();
    let x = std::fs::read_to_string(dir.join("IMG_1.xmp")).unwrap();
    assert!(!x.contains("aux:Lens") && !x.contains("exifEX:LensModel") && x.contains("<lc:exifOverrides>iso<"), "{x}");
    let _ = std::fs::remove_dir_all(&dir);
}
