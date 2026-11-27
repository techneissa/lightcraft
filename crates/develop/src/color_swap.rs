//! Fork (issue #460, `fork/color-swap.md`): Recolor, which replaces one colour with another across the whole photo,
//! several at once. The settings are one list of entries, each with a From colour, a To colour, how far from the From
//! colour still counts (Range, with a width of its own for hue, saturation and lightness), how softly that range
//! fades out, how much of the change is applied (Intensity), whether the pixel keeps its own lightness, and an on/off
//! switch. `lightcraft_pipeline::color_swap` turns an entry into the per-pixel change.
//!
//! The list lives in its own namespace of the settings JSON ([`KEY`]): a build without it ignores it, and a block
//! that cannot be read (or an element of it) reads as no entry rather than failing the photo's whole settings. The
//! sliders are indexed controls (`colorSwap.<i>.range`), so undo, presets, copy and paste, sync, the control channel
//! and MCP reach them as they reach any other slider.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::controls::{ControlSpec, Section, Track};

/// The settings' JSON key.
pub const KEY: &str = "fork_color_swaps";

/// The section id (its eye, `section.colorSwap`).
pub const SECTION: &str = "colorSwap";

/// The most entries a photo holds.
pub const MAX_SWAPS: usize = 8;

/// The longest name an entry keeps (characters).
pub const MAX_NAME: usize = 60;

/// One colour swap.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ColorSwap {
    /// The name the user gave it (empty: the panel shows "Recolor 1", "Recolor 2", … in the user's language).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Switched on. Off keeps every value and changes nothing.
    pub on: bool,
    /// The colour to replace, sRGB-encoded 0..1 (`None`: not chosen yet, the entry changes nothing).
    pub from: Option<[f64; 3]>,
    /// The colour it becomes, sRGB-encoded 0..1 (`None`: not chosen yet).
    pub to: Option<[f64; 3]>,
    /// How far from the From colour a colour still counts, 0..100 (50 = the usual reach; it scales the three widths).
    pub range: f64,
    /// How much of the change is applied, 0..100.
    pub intensity: f64,
    /// The hue's share of the range, 0..100 (50 = as Range says).
    pub hue_range: f64,
    /// The saturation's share of the range, 0..100.
    pub sat_range: f64,
    /// The lightness' share of the range, 0..100.
    pub lum_range: f64,
    /// How gradually the range fades out at its edge, 0 (a crisp edge) .. 100 (a fade from the From colour outwards).
    pub softness: f64,
    /// Each pixel keeps its own lightness, so shading and texture stay exactly as they were; off, the To colour's
    /// lightness is taken too (a dark shirt can become a white one, with its folds in proportion).
    pub keep_lightness: bool,
    /// Not saved: the entry's range is drawn instead of its change (the Option-hover view, `Overlay::ColorSwapRange`).
    #[serde(skip)]
    pub view: bool,
}

impl Default for ColorSwap {
    fn default() -> Self {
        Self {
            name: String::new(),
            on: true,
            from: None,
            to: None,
            range: 50.0,
            intensity: 100.0,
            hue_range: 50.0,
            sat_range: 50.0,
            lum_range: 50.0,
            softness: 50.0,
            keep_lightness: true,
            view: false,
        }
    }
}

impl ColorSwap {
    /// Whether the entry changes the photo: on, both colours chosen, some Intensity.
    pub fn is_active(&self) -> bool {
        self.on && self.from.is_some() && self.to.is_some() && self.intensity.is_finite() && self.intensity > 0.0
    }

    /// The entry with every number in its range (a number out of range is clamped, a NaN reads as the default; a colour channel the same, a NaN as 0) and the
    /// name cut to [`MAX_NAME`] characters.
    pub fn sanitized(&self) -> ColorSwap {
        let d = ColorSwap::default();
        let num = |v: f64, default: f64| if v.is_finite() { v.clamp(0.0, 100.0) } else { default };
        let colour = |c: Option<[f64; 3]>| c.map(|c| c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }));
        ColorSwap {
            name: self.name.trim().chars().take(MAX_NAME).collect(),
            on: self.on,
            from: colour(self.from),
            to: colour(self.to),
            range: num(self.range, d.range),
            intensity: num(self.intensity, d.intensity),
            hue_range: num(self.hue_range, d.hue_range),
            sat_range: num(self.sat_range, d.sat_range),
            lum_range: num(self.lum_range, d.lum_range),
            softness: num(self.softness, d.softness),
            keep_lightness: self.keep_lightness,
            view: self.view,
        }
    }
}

/// Reads one entry leniently: a missing key is its default, a key of the wrong type too.
impl<'de> Deserialize<'de> for ColorSwap {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(de)?;
        Ok(ColorSwap::from_value(&v))
    }
}

impl ColorSwap {
    fn from_value(v: &Value) -> ColorSwap {
        let d = ColorSwap::default();
        let num = |k: &str, default: f64| v.get(k).and_then(Value::as_f64).unwrap_or(default);
        let flag = |k: &str, default: bool| v.get(k).and_then(Value::as_bool).unwrap_or(default);
        let colour = |k: &str| -> Option<[f64; 3]> {
            let a = v.get(k)?.as_array()?;
            let [r, g, b] = a.as_slice() else { return None };
            Some([r.as_f64()?, g.as_f64()?, b.as_f64()?])
        };
        ColorSwap {
            name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
            on: flag("on", d.on),
            from: colour("from"),
            to: colour("to"),
            range: num("range", d.range),
            intensity: num("intensity", d.intensity),
            hue_range: num("hue_range", d.hue_range),
            sat_range: num("sat_range", d.sat_range),
            lum_range: num("lum_range", d.lum_range),
            softness: num("softness", d.softness),
            keep_lightness: flag("keep_lightness", d.keep_lightness),
            view: false,
        }
    }
}

/// Reads the list leniently: not a list is no entries, an element that is not an object is dropped, and no more than
/// [`MAX_SWAPS`] are kept.
pub fn deserialize_list<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<ColorSwap>, D::Error> {
    let v = Value::deserialize(de)?;
    Ok(v.as_array().map(|a| a.iter().filter(|e| e.is_object()).take(MAX_SWAPS).map(ColorSwap::from_value).collect()).unwrap_or_default())
}

const fn spec(id: &'static str, label: &'static str, default: f64, track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::ColorSwap, min: 0.0, max: 100.0, default, step: 1.0, decimals: 0, track }
}

/// The indexed sliders of an entry (`colorSwap.<i>.<field>`), as templates for [`crate::controls::INDEXED`].
pub static SPECS: &[ControlSpec] = &[
    spec("colorSwap.range", "Range", 50.0, Track::Plain),
    spec("colorSwap.intensity", "Intensity", 100.0, Track::Plain),
    spec("colorSwap.hueRange", "Hue Range", 50.0, Track::Rainbow),
    spec("colorSwap.satRange", "Saturation Range", 50.0, Track::Gradient { from: "#7a7a7a", to: "#e04a3a" }),
    spec("colorSwap.lumRange", "Lightness Range", 50.0, Track::Gradient { from: "#202020", to: "#f0f0f0" }),
    spec("colorSwap.softness", "Edge Softness", 50.0, Track::Plain),
];

/// The number `field` of an entry (`range`, `intensity`, `hueRange`, `satRange`, `lumRange`, `softness`).
pub fn field_mut<'a>(e: &'a mut ColorSwap, field: &str) -> Option<&'a mut f64> {
    Some(match field {
        "range" => &mut e.range,
        "intensity" => &mut e.intensity,
        "hueRange" => &mut e.hue_range,
        "satRange" => &mut e.sat_range,
        "lumRange" => &mut e.lum_range,
        "softness" => &mut e.softness,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_list_reads_as_what_can_be_read() {
        let v: Vec<ColorSwap> =
            deserialize_list(serde_json::json!([{"from": [1.0, 0.0, 0.0], "to": "nope", "range": "wide"}, 7, {"on": false}])).unwrap_or_default();
        assert_eq!(v.len(), 2, "an element that is not an object is dropped");
        assert_eq!(v[0].from, Some([1.0, 0.0, 0.0]));
        assert_eq!((v[0].to, v[0].range), (None, 50.0), "a value of the wrong type is its default");
        assert!(!v[1].on);
        let none: Vec<ColorSwap> = deserialize_list(serde_json::json!("x")).unwrap_or_default();
        assert!(none.is_empty());
    }

    #[test]
    fn an_empty_copy_removes_the_entries_it_is_pasted_onto() {
        let mut photo = crate::DevelopSettings::default();
        photo.color_swaps.push(ColorSwap::default());
        let clip = crate::extract_groups(&crate::DevelopSettings::default(), &[crate::SettingsGroup::ColorSwap]);
        assert!(photo.merged(&clip).map(|s| s.color_swaps.is_empty()).unwrap_or(false));
    }
}
