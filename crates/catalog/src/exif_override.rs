//! Edited camera and exposure fields.
//!
//! A photo's [`Meta`] keeps what the file says in `camera`, `lens`, `focal_mm`, `aperture`,
//! `shutter` and `iso`; nothing ever changes those after import, and nothing is written into the
//! original file. A person's edit is stored beside them as an [`ExifOverrides`] (`Meta::exif`),
//! one optional value per field, and reset by removing it.
//!
//! Who reads which:
//! - **Display, search, filters, smart albums, rename tokens and export** want what the person
//!   sees: [`Meta::shown`] (or the `effective_*` getters).
//! - **Anything that changes pixels** (camera colour profiles and matching, lens profiles, focus
//!   points, per-camera defaults, denoise by ISO) wants what the camera recorded: the plain
//!   fields, which an override never touches.

use crate::Meta;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Longest name (camera, lens) kept, in characters.
pub const MAX_NAME_CHARS: usize = 200;
/// Longest number text read before it is refused outright.
const MAX_NUMBER_CHARS: usize = 40;

/// Focal length range accepted, in millimetres.
pub const FOCAL_RANGE: (f32, f32) = (1.0, 5000.0);
/// Aperture (f-number) range accepted.
pub const APERTURE_RANGE: (f32, f32) = (0.3, 256.0);
/// ISO range accepted.
pub const ISO_RANGE: (u32, u32) = (1, 4_000_000);
/// Exposure time range accepted, in seconds (1/100 000 s to a day).
pub const SHUTTER_RANGE: (f64, f64) = (1.0e-5, 86_400.0);

/// One of the six editable fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExifField {
    Camera,
    Lens,
    Focal,
    Aperture,
    Shutter,
    Iso,
}

impl ExifField {
    pub const ALL: [ExifField; 6] = [ExifField::Camera, ExifField::Lens, ExifField::Focal, ExifField::Aperture, ExifField::Shutter, ExifField::Iso];

    /// The name commands use (`photo.setMeta` keys, `photo.resetExif` fields).
    pub fn key(self) -> &'static str {
        match self {
            ExifField::Camera => "camera",
            ExifField::Lens => "lens",
            ExifField::Focal => "focal",
            ExifField::Aperture => "aperture",
            ExifField::Shutter => "shutter",
            ExifField::Iso => "iso",
        }
    }

    pub fn from_key(key: &str) -> Option<ExifField> {
        ExifField::ALL.into_iter().find(|f| f.key().eq_ignore_ascii_case(key.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            ExifField::Camera => "Camera",
            ExifField::Lens => "Lens",
            ExifField::Focal => "Focal length",
            ExifField::Aperture => "Aperture",
            ExifField::Shutter => "Shutter speed",
            ExifField::Iso => "ISO",
        }
    }
}

/// The values a person set over the file's, one per field; `None` = the file's value shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExifOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_mm: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<f32>,
    /// As the file's `Meta::shutter`: `1/250`, `2.5` (seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<u32>,
}

impl ExifOverrides {
    pub fn is_empty(&self) -> bool {
        *self == ExifOverrides::default()
    }

    pub fn has(&self, field: ExifField) -> bool {
        match field {
            ExifField::Camera => self.camera.is_some(),
            ExifField::Lens => self.lens.is_some(),
            ExifField::Focal => self.focal_mm.is_some(),
            ExifField::Aperture => self.aperture.is_some(),
            ExifField::Shutter => self.shutter.is_some(),
            ExifField::Iso => self.iso.is_some(),
        }
    }

    fn clear(&mut self, field: ExifField) {
        match field {
            ExifField::Camera => self.camera = None,
            ExifField::Lens => self.lens = None,
            ExifField::Focal => self.focal_mm = None,
            ExifField::Aperture => self.aperture = None,
            ExifField::Shutter => self.shutter = None,
            ExifField::Iso => self.iso = None,
        }
    }
}

impl Meta {
    /// Whether `field` has an edited value.
    pub fn exif_overridden(&self, field: ExifField) -> bool {
        self.exif.has(field)
    }

    /// The camera as shown: the edited name, else the file's.
    pub fn effective_camera(&self) -> &str {
        self.exif.camera.as_deref().unwrap_or_else(|| no_placeholder(&self.camera))
    }
    pub fn effective_lens(&self) -> &str {
        self.exif.lens.as_deref().unwrap_or_else(|| no_placeholder(&self.lens))
    }
    pub fn effective_focal_mm(&self) -> Option<f32> {
        self.exif.focal_mm.or(self.focal_mm.filter(|v| v.is_finite() && *v > 0.0))
    }
    pub fn effective_aperture(&self) -> Option<f32> {
        self.exif.aperture.or(self.aperture.filter(|v| v.is_finite() && *v > 0.0))
    }
    pub fn effective_shutter(&self) -> &str {
        self.exif.shutter.as_deref().unwrap_or(&self.shutter)
    }
    pub fn effective_iso(&self) -> Option<u32> {
        self.exif.iso.or(self.iso)
    }

    /// The metadata as a person sees it: this one when no field is edited (the usual case, no
    /// copy), else a copy whose camera and exposure fields hold the edited values. For display,
    /// search, filters, smart albums, rename and export; never for rendering.
    pub fn shown(&self) -> Cow<'_, Meta> {
        let clean = |n: &str| !n.is_empty() && !is_placeholder(n);
        let plain = (self.focal_mm.is_none_or(|v| v > 0.0)) && self.aperture.is_none_or(|v| v > 0.0);
        if self.exif.is_empty() && plain && (self.camera.is_empty() || clean(&self.camera)) && (self.lens.is_empty() || clean(&self.lens)) {
            return Cow::Borrowed(self);
        }
        let mut m = self.clone();
        m.camera = self.effective_camera().to_string();
        m.lens = self.effective_lens().to_string();
        m.focal_mm = self.effective_focal_mm();
        m.aperture = self.effective_aperture();
        m.shutter = self.effective_shutter().to_string();
        m.iso = self.effective_iso();
        m.exif = ExifOverrides::default();
        Cow::Owned(m)
    }

    /// `field` as text: the file's (`file`) or the shown value; empty when there is none. The
    /// text [`Meta::set_exif`] reads back to the same value.
    pub fn exif_text(&self, field: ExifField, file: bool) -> String {
        let ov = if file { &ExifOverrides::default() } else { &self.exif };
        match field {
            ExifField::Camera => ov.camera.clone().unwrap_or_else(|| no_placeholder(&self.camera).to_string()),
            ExifField::Lens => ov.lens.clone().unwrap_or_else(|| no_placeholder(&self.lens).to_string()),
            ExifField::Focal => ov.focal_mm.or(self.focal_mm.filter(|v| *v > 0.0)).map(format_focal).unwrap_or_default(),
            ExifField::Aperture => ov.aperture.or(self.aperture.filter(|v| *v > 0.0)).map(format_aperture).unwrap_or_default(),
            ExifField::Shutter => ov.shutter.clone().unwrap_or_else(|| self.shutter.clone()),
            ExifField::Iso => ov.iso.or(self.iso).map(|i| i.to_string()).unwrap_or_default(),
        }
    }

    /// Edits `field`. `None` or an empty text resets it; a value equal to the file's resets it
    /// too (there is nothing to override). The error says what was wrong, and `self` is left
    /// as it was.
    pub fn set_exif(&mut self, field: ExifField, text: Option<&str>) -> Result<(), String> {
        let text = text.map(str::trim).filter(|t| !t.is_empty());
        let Some(text) = text else {
            self.exif.clear(field);
            return Ok(());
        };
        match field {
            // (a name differing from the file's only in case, as the pickers' `Sony` for the file's `SONY`, is the file's)
            ExifField::Camera => self.exif.camera = Some(parse_name(text)?).filter(|v| !v.eq_ignore_ascii_case(&self.camera)),
            ExifField::Lens => self.exif.lens = Some(parse_name(text)?).filter(|v| !v.eq_ignore_ascii_case(&self.lens)),
            ExifField::Focal => self.exif.focal_mm = Some(parse_focal(text)?).filter(|v| Some(*v) != self.focal_mm),
            ExifField::Aperture => self.exif.aperture = Some(parse_aperture(text)?).filter(|v| Some(*v) != self.aperture),
            ExifField::Shutter => self.exif.shutter = Some(parse_shutter(text)?).filter(|v| *v != self.shutter),
            ExifField::Iso => self.exif.iso = Some(parse_iso(text)?).filter(|v| Some(*v) != self.iso),
        }
        Ok(())
    }

    /// Resets `field` to the file's value.
    pub fn reset_exif(&mut self, field: ExifField) {
        self.exif.clear(field);
    }
}

/// Whether a camera or lens name is a placeholder, not a name: empty, or only dashes (`----`, what a body writes
/// for a manual lens).
pub fn is_placeholder(name: &str) -> bool {
    name.chars().all(|c| c.is_whitespace() || c == '-' || c == '?' || c == '\0')
}

/// `name`, or "" when it is a placeholder.
fn no_placeholder(name: &str) -> &str {
    if is_placeholder(name) { "" } else { name }
}

/// A camera or lens name: spaces collapsed, no control characters, at most [`MAX_NAME_CHARS`].
pub fn parse_name(text: &str) -> Result<String, String> {
    if text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err("a name can't hold control characters".into());
    }
    let name = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("a name is at most {MAX_NAME_CHARS} characters"));
    }
    if name.is_empty() {
        return Err("empty name".into());
    }
    Ok(name)
}

/// A number's text, lower-cased and bounded; `Err` for the absurdly long.
fn number_text(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.chars().count() > MAX_NUMBER_CHARS {
        return Err(format!("can't read `{}…` as a number", t.chars().take(12).collect::<String>()));
    }
    Ok(t.to_lowercase())
}

fn finite_in(v: f64, (lo, hi): (f64, f64), what: &str) -> Result<f64, String> {
    if v.is_finite() && v >= lo && v <= hi { Ok(v) } else { Err(format!("{what} must be between {lo} and {hi}")) }
}

/// `35`, `35 mm`, `35mm`, `70.5`.
pub fn parse_focal(text: &str) -> Result<f32, String> {
    let t = number_text(text)?;
    let n = t.strip_suffix("mm").unwrap_or(&t).trim();
    let v: f64 = n.parse().map_err(|_| format!("can't read `{text}` as a focal length (e.g. 35 or 35 mm)"))?;
    let v = finite_in(v, (FOCAL_RANGE.0 as f64, FOCAL_RANGE.1 as f64), "focal length")?;
    Ok(((v * 10.0).round() / 10.0) as f32)
}

/// `2.8`, `f/2.8`, `f2.8`, `F 2.8`, `ƒ/2.8`.
pub fn parse_aperture(text: &str) -> Result<f32, String> {
    let t = number_text(text)?;
    let n = t.trim_start_matches(['f', 'ƒ']).trim_start_matches(['/', '-']).trim();
    let v: f64 = n.parse().map_err(|_| format!("can't read `{text}` as an aperture (e.g. f/2.8 or 2.8)"))?;
    let v = finite_in(v, (APERTURE_RANGE.0 as f64, APERTURE_RANGE.1 as f64), "aperture")?;
    Ok(((v * 100.0).round() / 100.0) as f32)
}

/// `400`, `ISO 400`. Whole numbers only.
pub fn parse_iso(text: &str) -> Result<u32, String> {
    let t = number_text(text)?;
    let n = t.strip_prefix("iso").unwrap_or(&t).trim();
    let v: u32 = n.parse().map_err(|_| format!("can't read `{text}` as an ISO (a whole number, e.g. 400)"))?;
    if (ISO_RANGE.0..=ISO_RANGE.1).contains(&v) { Ok(v) } else { Err(format!("ISO must be between {} and {}", ISO_RANGE.0, ISO_RANGE.1)) }
}

/// A shutter speed in seconds: `1/250`, `1/250 s`, `0.5`, `2.5"`, `2.5 s`, `30 sec`.
pub fn parse_shutter_text(text: &str) -> Result<f64, String> {
    let t = number_text(text)?;
    let t = t.replace(['″', '”', '“'], "\"");
    let n = t.trim_end_matches(|c: char| c == '"' || c == 's' || c == 'e' || c == 'c' || c.is_whitespace());
    let bad = || format!("can't read `{text}` as a shutter speed (e.g. 1/250 or 2.5\")");
    let secs = match n.split_once('/') {
        Some((a, b)) => {
            let (a, b): (f64, f64) = (a.trim().parse().map_err(|_| bad())?, b.trim().parse().map_err(|_| bad())?);
            if b == 0.0 {
                return Err(bad());
            }
            a / b
        }
        None => n.trim().parse().map_err(|_| bad())?,
    };
    finite_in(secs, SHUTTER_RANGE, "shutter speed").map_err(|_| "shutter speed must be between 1/100000 s and 24 h".to_string())
}

/// A shutter speed as the catalog stores it (`Meta::shutter`'s style): `1/250` under a second
/// (a reciprocal within 1 % of a whole number), else seconds with at most one decimal.
pub fn format_shutter(secs: f64) -> String {
    if secs < 1.0 {
        let r = 1.0 / secs;
        if (r - r.round()).abs() <= r * 0.01 {
            return format!("1/{}", r.round() as u64);
        }
        return format!("{:.2}", secs).trim_end_matches('0').trim_end_matches('.').to_string();
    }
    format!("{secs:.1}").trim_end_matches('0').trim_end_matches('.').to_string()
}

/// [`parse_shutter_text`] then [`format_shutter`].
pub fn parse_shutter(text: &str) -> Result<String, String> {
    parse_shutter_text(text).map(format_shutter)
}

pub fn format_focal(mm: f32) -> String {
    format!("{mm:.1}").trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn format_aperture(f: f32) -> String {
    format!("{f:.2}").trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests;
