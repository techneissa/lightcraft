//! Fork (issue #300, `fork/relight.md`): what type each Relight light is, and the numbers of its type. The numbers
//! are plain, in the units the controls show; `lightcraft-pipeline` turns them into `lightcraft_relight::LightKind`.
//!
//! A light is a Point (the first light type, and what a light saved before this reads as), a Spot, a Sun / Window, a
//! Softbox, a Rim or a Fill. Saved with the light as `kind` (its name) and each number of a type that differs from its
//! default; read leniently: a kind this build doesn't know reads as a Point, a number that isn't one is its default.

use serde::Serialize;
use serde::ser::SerializeMap;
use serde_json::Value;

use crate::controls::{ControlSpec, Section, Track};

/// The types' names, in the order `relight.<n>.kind` counts them (0 is a Point).
pub const NAMES: [&str; 6] = ["point", "spot", "sun", "softbox", "rim", "fill"];

/// What a light is and the numbers of each type (those of a type the light isn't stay as they were, so switching back
/// finds them).
#[derive(Clone, Debug, PartialEq)]
pub struct KindSettings {
    /// The type: an index of [`NAMES`] (a control: `relight.<n>.kind`).
    pub which: f64,
    /// Spot: what it points at (normalized photo coordinates), the cone's half-angle in degrees and its edge softness.
    pub spot_aim_x: f64,
    pub spot_aim_y: f64,
    pub spot_angle: f64,
    pub spot_edge: f64,
    /// Sun / Window: where it shines from round the scene's vertical axis (degrees, 0 from the camera's side, 90 from
    /// the right) and its height (degrees, 0 on the horizon, 90 straight above).
    pub sun_azimuth: f64,
    pub sun_elevation: f64,
    /// Softbox: its width and height as shares of the photo's (0.02..1.5) and its rotation in degrees.
    pub box_width: f64,
    pub box_height: f64,
    pub box_rotation: f64,
    /// Softbox: where it shines (issue #335), two tilts from straight into the scene in degrees: towards the photo's
    /// right (negative: left) and towards its bottom (negative: up). 0 and 0 is the straight-in softbox.
    pub box_aim_h: f64,
    pub box_aim_v: f64,
    /// Rim: how far it reaches into things (0..100), the side of the photo it comes from (degrees: 0 from above, 90 from
    /// the right, −90 from the left) and how one-sided it is (0 the whole outline .. 100 only that side).
    pub rim_width: f64,
    pub rim_angle: f64,
    pub rim_side: f64,
    /// Fill: how much more near things get than far ones, 0..100.
    pub fill_near: f64,
}

impl Default for KindSettings {
    fn default() -> Self {
        Self {
            which: 0.0,
            spot_aim_x: 0.5,
            spot_aim_y: 0.65,
            spot_angle: 30.0,
            spot_edge: 40.0,
            sun_azimuth: 40.0,
            sun_elevation: 35.0,
            box_width: 0.3,
            box_height: 0.2,
            box_rotation: 0.0,
            box_aim_h: 0.0,
            box_aim_v: 0.0,
            rim_width: 50.0,
            rim_angle: 90.0,
            rim_side: 0.0,
            fill_near: 50.0,
        }
    }
}

/// `(json key, control field)` of each number, besides `kind`.
const NUMBERS: [(&str, &str); 15] = [
    ("spotAimX", "spotAimX"),
    ("spotAimY", "spotAimY"),
    ("spotAngle", "spotAngle"),
    ("spotEdge", "spotEdge"),
    ("sunAzimuth", "sunAzimuth"),
    ("sunElevation", "sunElevation"),
    ("boxWidth", "boxWidth"),
    ("boxHeight", "boxHeight"),
    ("boxRotation", "boxRotation"),
    ("boxAimH", "boxAimH"),
    ("boxAimV", "boxAimV"),
    ("rimWidth", "rimWidth"),
    ("rimAngle", "rimAngle"),
    ("rimSide", "rimSide"),
    ("fillNear", "fillNear"),
];

impl KindSettings {
    /// The type's index in [`NAMES`] (a value between two is the nearer one).
    pub fn index(&self) -> usize {
        if self.which.is_finite() { (self.which.round().max(0.0) as usize).min(NAMES.len() - 1) } else { 0 }
    }

    /// The type's name.
    pub fn name(&self) -> &'static str {
        NAMES.get(self.index()).copied().unwrap_or("point")
    }

    /// Whether the light has a place on the photo (a Rim and a Fill have none).
    pub fn has_place(&self) -> bool {
        self.index() < 4
    }

    pub(crate) fn field(&mut self, field: &str) -> Option<&mut f64> {
        Some(match field {
            "kind" => &mut self.which,
            "spotAimX" => &mut self.spot_aim_x,
            "spotAimY" => &mut self.spot_aim_y,
            "spotAngle" => &mut self.spot_angle,
            "spotEdge" => &mut self.spot_edge,
            "sunAzimuth" => &mut self.sun_azimuth,
            "sunElevation" => &mut self.sun_elevation,
            "boxWidth" => &mut self.box_width,
            "boxHeight" => &mut self.box_height,
            "boxRotation" => &mut self.box_rotation,
            "boxAimH" => &mut self.box_aim_h,
            "boxAimV" => &mut self.box_aim_v,
            "rimWidth" => &mut self.rim_width,
            "rimAngle" => &mut self.rim_angle,
            "rimSide" => &mut self.rim_side,
            "fillNear" => &mut self.fill_near,
            _ => return None,
        })
    }

    /// Every number within its control's range, the type a whole index.
    pub fn sanitize(&mut self) {
        for spec in SPECS {
            if let Some(v) = spec.id.strip_prefix("relight.").and_then(|f| self.field(f)) {
                *v = spec.clamp(*v);
            }
        }
        self.which = self.which.round();
    }

    /// The type a name (`spot`, any case) or an index (0..5) names; `None` for anything else.
    pub fn kind_of(v: &Value) -> Option<f64> {
        match v {
            Value::String(name) => NAMES.iter().position(|n| n.eq_ignore_ascii_case(name.trim())).map(|i| i as f64),
            Value::Number(n) => n.as_f64().filter(|i| i.fract() == 0.0 && (0.0..NAMES.len() as f64).contains(i)),
            _ => None,
        }
    }

    /// Read from a stored light (an object): `kind` as a name (or an index), the numbers as numbers; anything else is
    /// the default, a kind this build doesn't know is a Point.
    pub fn read(light: &Value) -> Self {
        let mut s = Self::default();
        let Some(o) = light.as_object() else { return s };
        s.which = match o.get("kind") {
            Some(Value::String(name)) => NAMES.iter().position(|n| n.eq_ignore_ascii_case(name.trim())).map_or(0.0, |i| i as f64),
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            _ => 0.0,
        };
        for (key, field) in NUMBERS {
            if let (Some(v), Some(f)) = (o.get(key).and_then(Value::as_f64), s.field(field)) {
                *f = v;
            }
        }
        s.sanitize();
        s
    }
}

/// Written as `kind` (its name, left out for a Point) and each number that differs from its default.
impl Serialize for KindSettings {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let d = KindSettings::default();
        let mut m = s.serialize_map(None)?;
        if self.index() != 0 {
            m.serialize_entry("kind", self.name())?;
        }
        for (key, field) in NUMBERS {
            let (mut a, mut b) = (self.clone(), d.clone());
            if let (Some(a), Some(b)) = (a.field(field).map(|v| *v), b.field(field).map(|v| *v))
                && a != b
            {
                m.serialize_entry(key, &a)?;
            }
        }
        m.end()
    }
}

const fn spec(id: &'static str, label: &'static str, (min, max, default): (f64, f64, f64), (step, decimals): (f64, u8)) -> ControlSpec {
    ControlSpec { id, label, section: Section::Relight, min, max, default, step, decimals, track: Track::Plain }
}

/// The controls of a light's type, per light (`relight.<n>.kind` …).
pub static SPECS: &[ControlSpec] = &[
    spec("relight.kind", "Light Type", (0.0, 5.0, 0.0), (1.0, 0)),
    spec("relight.spotAimX", "Aim Left to Right", (-0.5, 1.5, 0.5), (0.001, 3)),
    spec("relight.spotAimY", "Aim Top to Bottom", (-0.5, 1.5, 0.65), (0.001, 3)),
    spec("relight.spotAngle", "Cone Angle", (5.0, 85.0, 30.0), (1.0, 0)),
    spec("relight.spotEdge", "Cone Edge", (0.0, 100.0, 40.0), (1.0, 0)),
    spec("relight.sunAzimuth", "Sun Direction", (-180.0, 180.0, 40.0), (1.0, 0)),
    spec("relight.sunElevation", "Sun Height", (0.0, 90.0, 35.0), (1.0, 0)),
    spec("relight.boxWidth", "Box Width", (0.02, 1.5, 0.3), (0.001, 3)),
    spec("relight.boxHeight", "Box Height", (0.02, 1.5, 0.2), (0.001, 3)),
    spec("relight.boxRotation", "Box Angle", (-180.0, 180.0, 0.0), (1.0, 0)),
    spec("relight.boxAimH", "Box Aim Left to Right", (-80.0, 80.0, 0.0), (1.0, 0)),
    spec("relight.boxAimV", "Box Aim Top to Bottom", (-80.0, 80.0, 0.0), (1.0, 0)),
    spec("relight.rimWidth", "Rim Width", (0.0, 100.0, 50.0), (1.0, 0)),
    spec("relight.rimAngle", "Rim Direction", (-180.0, 180.0, 90.0), (1.0, 0)),
    spec("relight.rimSide", "Rim One-Sided", (0.0, 100.0, 0.0), (1.0, 0)),
    spec("relight.fillNear", "Near Bias", (0.0, 100.0, 50.0), (1.0, 0)),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DevelopSettings;
    use crate::relight::{LightSettings, get, set};
    use serde_json::json;

    #[test]
    fn a_point_writes_nothing_extra_and_a_spot_writes_its_kind_and_what_differs() {
        let mut s = DevelopSettings::default();
        assert!(set(&mut s, "relight.amount", 50.0));
        assert_eq!(s.to_json()[crate::relight::KEY]["lights"][0].get("kind"), None);
        assert!(set(&mut s, "relight.kind", 1.0) && set(&mut s, "relight.spotAngle", 12.0));
        let v = s.to_json();
        let light = &v[crate::relight::KEY]["lights"][0];
        assert_eq!((light["kind"].as_str(), light["spotAngle"].as_f64()), (Some("spot"), Some(12.0)));
        assert!(light.get("spotEdge").is_none() && light.get("sunAzimuth").is_none());
        let back = DevelopSettings::from_json(&v).unwrap();
        assert_eq!(back.relight, s.relight);
        assert_eq!(
            (get(&back, "relight.kind"), get(&back, "relight.spotAngle"), get(&back, "relight.spotEdge")),
            (Some(1.0), Some(12.0), Some(40.0))
        );
    }

    #[test]
    fn an_unknown_kind_reads_as_a_point_and_numbers_stay_in_range() {
        for kind in [json!("hologram"), json!(null), json!(["spot"]), json!(99.0), json!(-4.0), json!(f64::NAN)] {
            let v = json!({"fork_relight": {"lights": [{"amount": 40.0, "kind": kind, "spotAngle": 20.0}]}});
            let l = DevelopSettings::from_json(&v).unwrap().relight.lights[0].clone();
            let point = l.kind.index() == 0;
            // (an index past the last reads as the last type, a negative one as a Point; a name or a shape this build doesn't know as a Point)
            assert!(point || l.kind.index() == 5, "{l:?}");
            assert_eq!(l.amount, 40.0);
        }
        let v =
            json!({"fork_relight": {"lights": [{"kind": "Softbox", "boxWidth": 9.0, "boxHeight": "tall", "boxRotation": 1e9, "spotAngle": -1.0}]}});
        let l = DevelopSettings::from_json(&v).unwrap().relight.lights[0].clone();
        assert_eq!((l.kind.name(), l.kind.box_width, l.kind.box_height, l.kind.box_rotation, l.kind.spot_angle), ("softbox", 1.5, 0.2, 180.0, 5.0));
        assert!(l.kind.has_place());
        let mut s = DevelopSettings::default();
        assert!(set(&mut s, "relight.kind", 4.4) && set(&mut s, "relight.rimWidth", f64::NAN));
        assert_eq!((s.relight.lights[0].kind.name(), s.relight.lights[0].kind.has_place(), s.relight.lights[0].kind.rim_width), ("rim", false, 50.0));
        // a light saved before types reads as a Point, and the controls reach the new ones by index
        let old = LightSettings::default();
        assert_eq!(old.kind, KindSettings::default());
        s.relight.lights.push(LightSettings::default());
        assert!(set(&mut s, "relight.1.fillNear", 70.0) && get(&s, "relight.0.fillNear") == Some(50.0));
    }

    #[test]
    fn a_light_that_becomes_a_sun_starts_at_wrap_0_and_wrap_still_works() {
        let mut s = DevelopSettings::default();
        assert_eq!(get(&s, "relight.wrap"), Some(40.0));
        assert!(set(&mut s, "relight.kind", 2.0));
        assert_eq!(get(&s, "relight.wrap"), Some(0.0));
        assert!(set(&mut s, "relight.wrap", 30.0) && set(&mut s, "relight.kind", 1.0) && set(&mut s, "relight.kind", 2.0));
        assert_eq!(get(&s, "relight.wrap"), Some(30.0), "a Wrap that was set is kept");
        // (a Wrap read from a saved file is never touched)
        let v = json!({"fork_relight": {"lights": [{"kind": "sun"}]}});
        assert_eq!(DevelopSettings::from_json(&v).unwrap().relight.lights[0].wrap, 40.0);
    }
}
