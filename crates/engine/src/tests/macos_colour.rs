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

/// A macOS Colour photo that has a cached AI Denoise picture shows macOS's own picture while its Denoise is off: it is
/// not decoded through the colour transfer made for Denoise on, nor drawn from a source that was (a photo pasted onto
/// with Denoise on, then Reset, stayed navy with a red top edge). Its renders get keys of their own, so thumbnails
/// saved from such a source are made again.
#[test]
fn denoise_off_never_shows_the_denoise_colour_transfer_on_macos_colour() {
    use crate::media::{DecodedSource, DenoiseSpec, SourceLevel, SourceRef};
    let mut s = Session::new();
    let mut p = a7v(CameraColour::Macos);
    let level = SourceLevel::Preview;
    let paired = |r: &SourceRef| matches!(r, SourceRef::File { denoise: Some(_), .. });
    s.media.denoise.loader = Some(Arc::new(|_, _, _, _, _| Err("not read here".into())));
    let off_without = s.media.denoise_salt(p.id, &p.develop);
    s.media.denoise.set(p.id, DenoiseSpec { product: "/scratch/a.lcdn".into(), key: "k".into(), make: None });
    assert!(!paired(&s.media.source_ref(&p, level)), "Denoise off: macOS's own picture");
    assert_ne!(s.media.denoise_salt(p.id, &p.develop), off_without, "renders saved from the transfer are made again");

    let mut on = (*p.develop).clone();
    on.enhance.denoise = 60.0;
    on.enhance.denoise_on = Some(true);
    let off = std::mem::replace(&mut p.develop, Arc::new(on));
    assert!(paired(&s.media.source_ref(&p, level)), "Denoise on: with the denoised picture");
    let img = Arc::new(lightcraft_raster::Rgb32f::new(8, 8));
    s.media.insert_source(p.id, level, DecodedSource { paired: true, ..DecodedSource::new(img, None) });
    assert!(matches!(s.media.source_ref(&p, level), SourceRef::Loaded(_)), "Denoise on reuses it");
    p.develop = off;
    assert!(!paired(&s.media.source_ref(&p, level)), "Denoise off again: decoded without it, not drawn from it");
}
