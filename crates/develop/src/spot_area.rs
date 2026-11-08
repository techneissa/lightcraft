//! Fork feature (`fork/object-remove.md`): a Remove spot in the shape of an object or a mask instead of a brushed
//! stroke. The shape is stored with the spot as a model segmentation is stored with a mask ([`SegMask`]), so a render
//! never needs the model and a preview, the loupe and an export draw the same outline.

use lightcraft_geom::Point;
use serde::{Deserialize, Serialize};

use crate::{SegMask, Spot};

/// The shape of a shaped Remove spot ([`Spot::area`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpotArea {
    /// What made it: `object` (clicked or brushed in the Remove panel), `mask` (Remove This Area), `person` or `wire`
    /// (Find People / Find Wires). Only names the spot's row; an unknown name reads as an area.
    pub kind: String,
    /// The coverage, in the spot's own coordinates (normalized coordinates of the uncropped picture, as
    /// [`Spot::points`]): a logit above 0 is inside. Its `rect` is the part of the picture it covers.
    pub seg: SegMask,
    /// The clicks that picked an object (the model's photo coordinates, as `MaskShape::Object`), so a later click
    /// adds to or takes from the same selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hint: Vec<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Point>,
}

/// The kinds [`SpotArea::kind`] names.
pub const AREA_KINDS: [&str; 4] = ["object", "mask", "person", "wire"];

impl SpotArea {
    /// A shape of `kind` (one of [`AREA_KINDS`]) covering `seg`.
    pub fn new(kind: &str, seg: SegMask) -> SpotArea {
        SpotArea { kind: kind.to_string(), seg, hint: Vec::new(), exclude: Vec::new() }
    }

    /// The name shown on the spot's row: "Object", "Mask", "Person", "Wire" (else "Area").
    pub fn label(&self) -> &'static str {
        match self.kind.as_str() {
            "object" => "Object",
            "mask" => "Mask",
            "person" => "Person",
            "wire" => "Wire",
            _ => "Area",
        }
    }

    /// The part of the picture the shape covers, `[x0, y0, x1, y1]` in spot coordinates; `None` when damaged.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        self.seg.bounds()
    }

    /// The shape moved by `(dx, dy)` (normalized), as a spot's target moves. The clicks no longer describe it, so they go.
    pub fn shift(&mut self, dx: f64, dy: f64) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        if let Some(r) = self.seg.rect.or_else(|| self.seg.bounds()) {
            self.seg.rect = Some([r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]);
        }
        self.hint.clear();
        self.exclude.clear();
    }
}

impl Spot {
    /// Whether the spot is drawn in the shape of [`Spot::area`] rather than along its stroke.
    pub fn is_shaped(&self) -> bool {
        self.area.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DevelopSettings;

    fn shaped() -> Spot {
        let seg = SegMask::from_logits_in(4, &[1.0; 16], [0.2, 0.3, 0.4, 0.6]);
        Spot { points: vec![Point::new(0.3, 0.45)], area: Some(SpotArea::new("object", seg)), ..Default::default() }
    }

    // Given a spot saved before shaped spots, it reads and writes back byte for byte
    #[test]
    fn an_old_spot_saves_as_it_did() {
        let old = r#"{"mode":"remove","points":[{"x":0.5,"y":0.5}],"size":0.02,"feather":50.0,"opacity":100.0,"source_offset":null}"#;
        let spot: Spot = serde_json::from_str(old).unwrap();
        assert!(spot.area.is_none());
        assert_eq!(serde_json::to_string(&spot).unwrap(), old);
        let d = DevelopSettings { spots: vec![spot], ..Default::default() };
        let back: DevelopSettings = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn a_shaped_spot_round_trips() {
        let s = shaped();
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["area"]["kind"], "object");
        assert!(json["area"].get("hint").is_none(), "no clicks, nothing written");
        let back: Spot = serde_json::from_value(json).unwrap();
        assert_eq!(back, s);
        assert!(back.is_shaped());
    }

    // Given a shape from a newer build, it still loads; damaged data loads too (and draws nothing)
    #[test]
    fn unknown_kinds_and_damaged_shapes_load() {
        let v = serde_json::json!({"points": [{"x": 0.5, "y": 0.5}], "area": {"kind": "lasso", "seg": {"side": 99999, "data": "!!"}}});
        let s: Spot = serde_json::from_value(v).unwrap();
        let a = s.area.unwrap();
        assert_eq!(a.label(), "Area");
        assert!(a.seg.logits().is_none());
    }

    #[test]
    fn shifting_moves_the_rectangle_and_forgets_the_clicks() {
        let mut a = shaped().area.unwrap();
        a.hint.push(Point::new(0.3, 0.4));
        a.shift(0.1, -0.1);
        let r = a.bounds().unwrap();
        assert!((r[0] - 0.3).abs() < 1e-12 && (r[1] - 0.2).abs() < 1e-12 && (r[2] - 0.5).abs() < 1e-12, "{r:?}");
        assert!(a.hint.is_empty());
        a.shift(f64::NAN, 0.0);
        assert_eq!(a.bounds(), Some(r));
    }
}
