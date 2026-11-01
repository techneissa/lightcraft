//! Edited camera, lens and exposure values in XMP sidecars (the six Info fields of
//! `lightcraft_catalog::exif_override`).
//!
//! **Write:** only the overridden fields go into the packet, in the standard tags: `tiff:Make` +
//! `tiff:Model` (camera), `aux:Lens` + `exifEX:LensModel`, `exif:FocalLength`, `exif:FNumber`,
//! `exif:ExposureTime`, and `exif:ISOSpeedRatings` (a `rdf:Seq`) + `exifEX:PhotographicSensitivity`.
//! The packet also names the overridden fields in `lc:exifOverrides` (`camera,lens,iso`), so a
//! later save that finds a field reset removes the tags it wrote, and leaves every other app's
//! `exif:` / `tiff:` tags alone. A sidecar a foreign app wrote keeps its tags for fields that are
//! not overridden.
//!
//! **Read:** a sidecar's values are overrides when they differ from the file's own (a value equal
//! to the file's clears the override: the sidecar wins, as for the other fields). A value that
//! can't be read as a valid one (a negative ISO, `1/0` seconds, a name of 5,000 characters) is
//! skipped, never an error.

use lightcraft_catalog::exif_override::{format_aperture, format_focal, parse_aperture, parse_focal, parse_shutter_text};
use lightcraft_catalog::{ExifField, ExifOverrides, Meta};
use lightcraft_meta::Metadata;

/// The `lc:` property that names the overridden fields.
pub(super) const MARKER: &str = "exifOverrides";

/// Makes of more than one word, so the camera `Phase One IQ4` splits into make and model right.
const MULTI_WORD_MAKES: &[&str] = &["Phase One", "Konica Minolta", "OM Digital Solutions", "Leica Camera AG", "Leica Camera"];

/// What a sidecar says about the six fields, as stated (`None` = not stated or not a valid value).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidecarExif {
    camera: Option<String>,
    lens: Option<String>,
    focal_mm: Option<f64>,
    aperture: Option<f64>,
    shutter_secs: Option<f64>,
    iso: Option<u32>,
}

/// The standard tags of `field`, all of which are replaced (or removed) together.
fn tags(field: ExifField) -> &'static [&'static str] {
    match field {
        ExifField::Camera => &["tiff:Make", "tiff:Model"],
        ExifField::Lens => &["aux:Lens", "exifEX:LensModel"],
        ExifField::Focal => &["exif:FocalLength"],
        ExifField::Aperture => &["exif:FNumber"],
        ExifField::Shutter => &["exif:ExposureTime"],
        ExifField::Iso => &["exif:ISOSpeedRatings", "exifEX:PhotographicSensitivity"],
    }
}

/// What the parsed packet states for the six fields; `None` when it states none of them.
pub(super) fn read(m: &Metadata) -> Option<SidecarExif> {
    // (a body's `----` for a manual lens, or a camera with no model, is no name)
    fn name(s: &Option<String>) -> Option<&str> {
        s.as_deref().map(str::trim).filter(|s| !lightcraft_catalog::exif_override::is_placeholder(s))
    }
    let camera = name(&m.model).map(|model| [name(&m.make).unwrap_or_default(), model].join(" ").trim().to_string());
    let lens = name(&m.lens_model).map(str::to_string);
    let out = SidecarExif { camera, lens, focal_mm: m.focal_length, aperture: m.f_number, shutter_secs: m.exposure_time, iso: m.iso };
    (out != SidecarExif::default()).then_some(out)
}

/// Applies what a sidecar states to `meta`: a value differing from the file's becomes an override,
/// a value equal to it removes one, an invalid or unstated one leaves the field as it is.
pub(super) fn apply(meta: &mut Meta, sc: &SidecarExif) {
    for field in ExifField::ALL {
        let Some(text) = text_of(sc, field) else { continue };
        if same_as_file(meta, field, &text) {
            meta.reset_exif(field);
        } else {
            // (an invalid value leaves the field as it was)
            let _ = meta.set_exif(field, Some(&text));
        }
    }
}

fn text_of(sc: &SidecarExif, field: ExifField) -> Option<String> {
    match field {
        ExifField::Camera => sc.camera.clone(),
        ExifField::Lens => sc.lens.clone(),
        ExifField::Focal => sc.focal_mm.map(|v| v.to_string()),
        ExifField::Aperture => sc.aperture.map(|v| v.to_string()),
        ExifField::Shutter => sc.shutter_secs.map(|v| v.to_string()),
        ExifField::Iso => sc.iso.map(|v| v.to_string()),
    }
}

/// Whether `text` is the file's focal length or aperture once both are rounded as an edit is
/// (a file's 24.04 mm and a sidecar's `24` are one value, not an edit). The other fields compare
/// in [`Meta::set_exif`].
fn same_as_file(meta: &Meta, field: ExifField, text: &str) -> bool {
    match field {
        ExifField::Focal => {
            let file = meta.focal_mm.filter(|v| v.is_finite() && *v > 0.0).and_then(|v| parse_focal(&format_focal(v)).ok());
            file.is_some() && file == parse_focal(text).ok()
        }
        ExifField::Aperture => {
            let file = meta.aperture.filter(|v| v.is_finite() && *v > 0.0).and_then(|v| parse_aperture(&format_aperture(v)).ok());
            file.is_some() && file == parse_aperture(text).ok()
        }
        _ => false,
    }
}

/// `camera` split into make and model (`Sony ILCE-7M5` → `Sony`, `ILCE-7M5`); a one-word name is a model.
fn split_camera(camera: &str) -> (Option<String>, String) {
    let make_len = MULTI_WORD_MAKES
        .iter()
        .find(|make| camera.get(..make.len()).is_some_and(|p| p.eq_ignore_ascii_case(make)) && camera[make.len()..].starts_with(' '))
        .map(|make| make.len())
        .or_else(|| camera.find(' '));
    match make_len {
        Some(n) => (Some(camera[..n].to_string()), camera[n..].trim().to_string()),
        None => (None, camera.to_string()),
    }
}

/// Puts the overridden values into `out` (the packet's metadata) and returns the value of
/// `lc:exifOverrides`, or `None` when nothing is overridden.
pub(super) fn write(out: &mut Metadata, ex: &ExifOverrides) -> Option<String> {
    let mut fields = Vec::new();
    for field in ExifField::ALL.into_iter().filter(|f| ex.has(*f)) {
        match field {
            ExifField::Camera => {
                if let Some(camera) = ex.camera.as_deref().filter(|c| !c.trim().is_empty()) {
                    let (make, model) = split_camera(camera.trim());
                    (out.make, out.model) = (make, Some(model));
                }
            }
            ExifField::Lens => out.lens_model = ex.lens.clone().filter(|l| !l.trim().is_empty()),
            // (through the text, so 2.8 is written as 28/10, not as the f32's 2.799999952)
            ExifField::Focal => out.focal_length = ex.focal_mm.and_then(|v| format_focal(v).parse().ok()),
            ExifField::Aperture => out.f_number = ex.aperture.and_then(|v| format_aperture(v).parse().ok()),
            ExifField::Shutter => out.exposure_time = ex.shutter.as_deref().and_then(|s| parse_shutter_text(s).ok()),
            ExifField::Iso => out.iso = ex.iso,
        }
        fields.push(field.key());
    }
    (!fields.is_empty()).then(|| fields.join(","))
}

/// The fields a packet names in `lc:exifOverrides`.
fn marked(packet: &str) -> Vec<ExifField> {
    if !packet.contains(MARKER) {
        return Vec::new();
    }
    let Ok(d) = lightcraft_meta::parse_xmp(packet) else { return Vec::new() };
    d.properties
        .get(&format!("lc:{MARKER}"))
        .and_then(|v| v.first())
        .map(|list| list.split(',').filter_map(ExifField::from_key).collect())
        .unwrap_or_default()
}

/// The tags the merge owns beyond [`super::OWNED`]: those of every field the new packet or the
/// existing sidecar names as overridden. A field that was overridden is replaced, or removed
/// when reset; a field nobody overrode keeps whatever another app wrote.
pub(super) fn extra_owned(existing: &str, fresh: &str) -> Vec<&'static str> {
    let mut owned = Vec::new();
    for field in marked(fresh).into_iter().chain(marked(existing)) {
        for tag in tags(field) {
            if !owned.contains(tag) {
                owned.push(*tag);
            }
        }
    }
    owned
}

#[cfg(test)]
mod tests;
