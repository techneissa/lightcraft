//! Fork feature (`fork/glare-highlights.md`): what Remove ▸ Reflections works on, and its Highlights mode.
//!
//! [`ReflectionArea`] is the part of the photo a reflection model or the glare finder runs on: a box, or a mask of the
//! Masks panel (run on the mask's bounding box, kept inside the mask). Coordinates are percent of the original photo
//! before crop or rotation, like Limit to Area, so the area stays on the same content when the crop changes.
//!
//! [`Glare`] holds the Highlights mode's settings: how strict the finder is, how far it grows what it found, the fill,
//! and the strokes the user painted to add to or take from the found glare.
//!
//! Every field is left out of the file while it has its default value, so photos without these are written as before.

use lightcraft_geom::Point;
use serde::{Deserialize, Serialize};

use crate::SpotFill;

/// Most strokes one photo keeps for the glare (a sidecar or an agent with thousands would stall every recalculation).
pub const MAX_GLARE_STROKES: usize = 256;
/// Most points one stroke keeps.
pub const MAX_STROKE_POINTS: usize = 4096;

/// What the Reflections section removes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReflectionMode {
    /// A reflection seen in glass, separated by a reflection model.
    #[default]
    Reflections,
    /// Glare and shine: bright, colourless spots found in the photo and filled from their surroundings.
    Highlights,
}

impl ReflectionMode {
    pub fn is_reflections(&self) -> bool {
        *self == ReflectionMode::Reflections
    }

    /// The name used in commands and files.
    pub fn name(self) -> &'static str {
        match self {
            ReflectionMode::Reflections => "reflections",
            ReflectionMode::Highlights => "highlights",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "reflections" => Some(ReflectionMode::Reflections),
            "highlights" => Some(ReflectionMode::Highlights),
            _ => None,
        }
    }
}

/// The part of the photo the model runs on. Percent of the original photo, before crop or rotation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReflectionArea {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    /// A mask of the Masks panel (its id): the model runs on its bounding box and the result is kept inside it. The
    /// box is then the mask's bounding box when the result was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<u32>,
}

impl Default for ReflectionArea {
    fn default() -> Self {
        Self { left: 0.0, top: 0.0, right: 100.0, bottom: 100.0, mask: None }
    }
}

impl ReflectionArea {
    /// A box, if its sides are numbers in 0..=100 and it has an inside.
    pub fn boxed(left: f64, top: f64, right: f64, bottom: f64) -> Option<Self> {
        let a = Self { left, top, right, bottom, mask: None };
        a.is_valid().then_some(a)
    }

    pub fn is_valid(&self) -> bool {
        [self.left, self.top, self.right, self.bottom].iter().all(|v| v.is_finite() && (0.0..=100.0).contains(v))
            && self.left < self.right
            && self.top < self.bottom
    }

    /// The box as fractions of the photo: `(left, top, right, bottom)`.
    pub fn fractions(&self) -> (f64, f64, f64, f64) {
        (self.left / 100.0, self.top / 100.0, self.right / 100.0, self.bottom / 100.0)
    }
}

/// One stroke painted on the photo for the glare: points as fractions of the original photo (before crop or
/// rotation), `size` the brush radius as a fraction of its long edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlareStroke {
    /// Takes away from the found glare instead of adding to it.
    #[serde(skip_serializing_if = "is_false")]
    pub erase: bool,
    pub size: f64,
    pub points: Vec<Point>,
}

impl Default for GlareStroke {
    fn default() -> Self {
        Self { erase: false, size: 0.01, points: Vec::new() }
    }
}

impl GlareStroke {
    /// A stroke kept within bounds: a radius in 0.0005..=0.25 of the long edge, at most [`MAX_STROKE_POINTS`] finite
    /// points. `None` when nothing usable is left.
    pub fn sanitized(mut self) -> Option<Self> {
        if !self.size.is_finite() {
            return None;
        }
        self.size = self.size.clamp(0.0005, 0.25);
        self.points.retain(|p| p.x.is_finite() && p.y.is_finite() && (-0.5..=1.5).contains(&p.x) && (-0.5..=1.5).contains(&p.y));
        self.points.truncate(MAX_STROKE_POINTS);
        (!self.points.is_empty()).then_some(self)
    }
}

/// The Highlights mode's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glare {
    /// 0..100: higher finds only the brightest, most colourless spots.
    pub threshold: f64,
    /// 0..100: how far the found glare grows before it is filled.
    pub expand: f64,
    /// The fill: LaMa when its model is there, else Classic.
    pub fill: SpotFill,
    /// Painted strokes, in order: each adds to or erases from the found glare.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paint: Vec<GlareStroke>,
}

pub const DEFAULT_GLARE_THRESHOLD: f64 = 50.0;
pub const DEFAULT_GLARE_EXPAND: f64 = 30.0;

impl Default for Glare {
    fn default() -> Self {
        Self { threshold: DEFAULT_GLARE_THRESHOLD, expand: DEFAULT_GLARE_EXPAND, fill: SpotFill::Lama, paint: Vec::new() }
    }
}

impl Glare {
    pub fn is_default(&self) -> bool {
        *self == Glare::default()
    }

    /// Kept within bounds (values from a sidecar or an agent): sliders in 0..=100, at most [`MAX_GLARE_STROKES`]
    /// usable strokes.
    pub fn sanitized(mut self) -> Self {
        let clamp = |v: f64, d: f64| if v.is_finite() { v.clamp(0.0, 100.0) } else { d };
        self.threshold = clamp(self.threshold, DEFAULT_GLARE_THRESHOLD);
        self.expand = clamp(self.expand, DEFAULT_GLARE_EXPAND);
        self.paint = self.paint.into_iter().filter_map(GlareStroke::sanitized).take(MAX_GLARE_STROKES).collect();
        self
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reflection;

    #[test]
    fn defaults_are_not_written_and_old_files_read_as_whole_photo_reflections() {
        let r = Reflection::default();
        let json = serde_json::to_value(&r).unwrap();
        for key in ["mode", "area", "glare"] {
            assert!(json.get(key).is_none(), "{key} written while default: {json}");
        }
        let old: Reflection = serde_json::from_value(serde_json::json!({"enabled": true, "amount": 35.0})).unwrap();
        assert_eq!(old.mode, ReflectionMode::Reflections);
        assert_eq!(old.area, None);
        assert_eq!(old.glare, Glare::default());
    }

    #[test]
    fn highlights_area_and_paint_survive_a_round_trip() {
        let r = Reflection {
            mode: ReflectionMode::Highlights,
            area: Some(ReflectionArea { left: 10.0, top: 20.0, right: 30.0, bottom: 40.0, mask: Some(7) }),
            glare: Glare {
                threshold: 70.0,
                expand: 10.0,
                fill: SpotFill::Classic,
                paint: vec![GlareStroke { erase: true, size: 0.02, points: vec![Point::new(0.5, 0.5)] }],
            },
            ..Default::default()
        };
        let back: Reflection = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn hostile_values_are_kept_in_bounds() {
        assert!(ReflectionArea::boxed(10.0, 10.0, 5.0, 50.0).is_none());
        assert!(ReflectionArea::boxed(f64::NAN, 10.0, 50.0, 50.0).is_none());
        assert!(ReflectionArea::boxed(0.0, 0.0, 100.0, 100.0).is_some());
        let g = Glare {
            threshold: f64::INFINITY,
            expand: -4.0,
            paint: (0..1000)
                .map(|i| GlareStroke {
                    erase: false,
                    size: if i == 0 { f64::NAN } else { 9.0 },
                    points: vec![Point::new(0.5, f64::NAN), Point::new(0.1, 0.1)],
                })
                .collect(),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(g.threshold, DEFAULT_GLARE_THRESHOLD);
        assert_eq!(g.expand, 0.0);
        assert_eq!(g.paint.len(), MAX_GLARE_STROKES);
        assert!(g.paint.iter().all(|s| s.size == 0.25 && s.points.len() == 1));
    }
}
