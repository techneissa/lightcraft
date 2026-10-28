//! macOS Colour outside the loader: no camera-JPEG stand-in (T26) and the white balance scale of Lightroom XMP.

use std::sync::Arc;

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::CameraColour;

use crate::Session;

fn a7v(colour: CameraColour) -> Photo {
    let mut p = Photo::new(PhotoId(1), Source::File { path: "/scratch/a.arw".into() }, "a.arw", "ARW", 7008, 4672, "2026-10-28");
    p.kind = MediaKind::Raw;
    p.meta.camera = "SONY ILCE-7M5".into();
    let mut d = p.camera_defaults();
    d.profile.camera_colour = colour;
    p.develop = Arc::new(d);
    p
}

/// T26: a photo on macOS Colour that this Mac makes never shows the camera's JPEG first (grid, filmstrip, loupe): it
/// waits for the macOS render. Where macOS Colour isn't made (another computer, a camera the decoder doesn't list), the
/// photo renders as Camera Matching and the stand-in rule is the usual one.
#[test]
fn a_macos_colour_photo_never_shows_the_camera_jpeg_while_it_renders() {
    let mut s = Session::new();
    s.media.preview_loader = Some(Arc::new(|_, _| None));
    let auto = a7v(CameraColour::Auto);
    assert!(crate::import::has_import_look(&auto));
    assert!(s.embedded_thumb_is_final(&auto), "an unedited Camera Matching raw starts from its JPEG");
    let mut macos = a7v(CameraColour::Macos);
    // (as a photo imported with macOS Colour as its starting colour: unedited)
    macos.import_look = Some(macos.develop.clone());
    assert_eq!(s.embedded_thumb_is_final(&macos), !crate::files::macos_shows(&macos));
    assert_eq!(crate::files::macos_shows(&macos), crate::files::macos_lists(&macos));
    s.catalog.apply(Op::AddPhoto { photo: Box::new(macos.clone()) }).unwrap();
    let quick = s.quick_view_job(PhotoId(1), 512, true, true);
    if crate::files::macos_shows(&macos) {
        assert!(quick.is_none_or(|q| q.embedded.is_none()), "no camera JPEG in the loupe either");
    }
}

/// A Lightroom white balance on a photo that edits in Kelvin here (macOS Colour on a Mac whose decoder lists the
/// camera) is read as Kelvin; on Camera Matching the A7 V's relative scale stays.
#[test]
fn lightroom_kelvin_is_read_as_kelvin_on_macos_colour() {
    use crate::crs::Target;
    assert_eq!(Target::for_photo(&a7v(CameraColour::Auto)), Target::RawRelative);
    let macos = a7v(CameraColour::Macos);
    let expected = if crate::files::macos_lists(&macos) { Target::RawAbsolute } else { Target::RawRelative };
    assert_eq!(Target::for_photo(&macos), expected);
}

/// T13 (export half): a JPEG export of a macOS Colour photo is rendered from macOS's base (its job asks the loader for
/// macOS Colour; the loader's macOS path is tested with a stand-in decoder in `files::macos`), while a DNG written from
/// it says Camera Matching in its XMP (its own matrices carry the colour).
#[test]
fn a_jpeg_export_renders_from_macos_and_a_dng_says_camera_matching() {
    let mut s = Session::new();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(a7v(CameraColour::Macos)) }).unwrap();
    let job = s.export_job(PhotoId(1), 1600, 1600, lightcraft_pipeline::OutputSpace::Srgb, lightcraft_pipeline::OutputDepth::U8).unwrap();
    let crate::media::SourceRef::File { options, .. } = &job.source else { panic!("rendered from the file") };
    assert_eq!(options.colour, CameraColour::Macos);

    let photo = s.catalog.photo(PhotoId(1)).unwrap().clone();
    assert!(crate::sidecar::sidecar_packet(&photo, &s.catalog).contains("macos"), "the photo's own packet names it");
    let packet = crate::sidecar::sidecar_packet(&crate::export::for_dng(&photo), &s.catalog);
    assert!(!packet.contains("macos"), "the DNG's XMP names no colour source, so it reads as Camera Matching");
}
