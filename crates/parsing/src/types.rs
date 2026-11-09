//! What a parse gives back: people, each with a face box, dense landmarks and part masks as float planes.
//!
//! Every position is a fraction of the photo's width and height (0 to 1, y down), so a mask means the same at
//! any resolution. A [`Plane`] holds one part's mask over a rectangle of the photo at the resolution the model saw
//! it; [`crate::render`] samples it at any output size.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The parts of a person. Background (everything that is not a person) is [`crate::Parsing::background`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Part {
    EntirePerson,
    /// Face skin, nose and ears.
    FacialSkin,
    /// Neck, arms, hands and legs.
    BodySkin,
    Hair,
    Eyebrows,
    EyeWhites,
    Iris,
    Lips,
    Teeth,
    ClothesTop,
    ClothesBottom,
    Dress,
    Shoes,
    Hat,
    Bag,
    /// Glasses, sunglasses, earrings, necklaces, belts and scarves.
    Others,
}

impl Part {
    pub const ALL: [Part; 16] = [
        Part::EntirePerson,
        Part::FacialSkin,
        Part::BodySkin,
        Part::Hair,
        Part::Eyebrows,
        Part::EyeWhites,
        Part::Iris,
        Part::Lips,
        Part::Teeth,
        Part::ClothesTop,
        Part::ClothesBottom,
        Part::Dress,
        Part::Shoes,
        Part::Hat,
        Part::Bag,
        Part::Others,
    ];

    /// The id commands and files use (`facialSkin`).
    pub fn id(self) -> &'static str {
        match self {
            Part::EntirePerson => "entirePerson",
            Part::FacialSkin => "facialSkin",
            Part::BodySkin => "bodySkin",
            Part::Hair => "hair",
            Part::Eyebrows => "eyebrows",
            Part::EyeWhites => "eyeWhites",
            Part::Iris => "iris",
            Part::Lips => "lips",
            Part::Teeth => "teeth",
            Part::ClothesTop => "clothesTop",
            Part::ClothesBottom => "clothesBottom",
            Part::Dress => "dress",
            Part::Shoes => "shoes",
            Part::Hat => "hat",
            Part::Bag => "bag",
            Part::Others => "others",
        }
    }

    /// The name the user sees.
    pub fn label(self) -> &'static str {
        match self {
            Part::EntirePerson => "Entire Person",
            Part::FacialSkin => "Facial Skin",
            Part::BodySkin => "Body Skin",
            Part::Hair => "Hair",
            Part::Eyebrows => "Eyebrows",
            Part::EyeWhites => "Eye Whites",
            Part::Iris => "Iris",
            Part::Lips => "Lips",
            Part::Teeth => "Teeth",
            Part::ClothesTop => "Clothes Top",
            Part::ClothesBottom => "Clothes Bottom",
            Part::Dress => "Dress",
            Part::Shoes => "Shoes",
            Part::Hat => "Hat",
            Part::Bag => "Bag",
            Part::Others => "Others",
        }
    }

    pub fn from_id(id: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|p| p.id() == id)
    }
}

/// A rectangle in fractions of the photo's width and height (0 to 1, y down; `x0 < x1`, `y0 < y1`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl NormRect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn centre(&self) -> (f32, f32) {
        ((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    /// Finite, ordered and not empty.
    pub fn is_valid(&self) -> bool {
        [self.x0, self.y0, self.x1, self.y1].iter().all(|v| v.is_finite()) && self.x1 > self.x0 && self.y1 > self.y0
    }

    /// The part inside the photo (`None` when nothing is).
    pub fn clamped(&self) -> Option<NormRect> {
        let r = NormRect { x0: self.x0.max(0.0), y0: self.y0.max(0.0), x1: self.x1.min(1.0), y1: self.y1.min(1.0) };
        r.is_valid().then_some(r)
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, o: &NormRect) -> NormRect {
        NormRect { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
}

/// One part's mask over `rect`, `width × height` values 0 to 1, row-major, at the resolution the model saw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub rect: NormRect,
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
    /// Cut from the whole photo's body map, which gives each of its cells to one person: different people's split planes
    /// share out one parse and add up. Any other plane (a face parsed up close, which may claim cells nobody owns, as
    /// the next face may too; an edge redrawn by a matte) joins other people's by the larger value (#124).
    pub split: bool,
}

impl Plane {
    /// A plane whose sizes agree with its data and rectangle (what [`crate::cache`] accepts from disk).
    pub fn is_valid(&self) -> bool {
        self.rect.is_valid() && self.width > 0 && self.height > 0 && self.width.checked_mul(self.height) == Some(self.data.len())
    }

    /// The value at a point of the photo (fractions), bilinear; 0 outside the rectangle.
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let r = &self.rect;
        if x < r.x0 || x > r.x1 || y < r.y0 || y > r.y1 || self.data.is_empty() {
            return 0.0;
        }
        let fx = (x - r.x0) / r.width() * self.width as f32 - 0.5;
        let fy = (y - r.y0) / r.height() * self.height as f32 - 0.5;
        crate::image::bilinear(&self.data, self.width, self.height, fx, fy)
    }

    /// Largest value.
    pub fn peak(&self) -> f32 {
        self.data.iter().fold(0.0f32, |a, &v| a.max(v))
    }

    /// Covered area as a fraction of the photo (sum of values × the area one value stands for).
    pub fn area(&self) -> f32 {
        let n = self.data.len().max(1) as f32;
        self.data.iter().sum::<f32>() / n * self.rect.width() * self.rect.height()
    }
}

/// A face as a detector (YuNet in `lightcraft-faces`, or the caller's faces catalog) found it: the hint a parse
/// starts from. Without hints the parse finds faces itself, from the body model's face class.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceHint {
    pub rect: NormRect,
    /// Right eye, left eye, nose tip, right and left mouth corner (the subject's right and left, YuNet's order),
    /// in fractions of the photo. Only the eyes are used, to turn the face upright.
    pub landmarks: Option<[(f32, f32); 5]>,
    pub score: f32,
    /// The person this face belongs to in the caller's faces catalog, passed through.
    pub person_id: Option<String>,
}

/// A dense face landmark: `x`, `y` in fractions of the photo, `z` (depth, toward the camera negative) in fractions of
/// the photo's width, as MediaPipe's face mesh orders them (468 face points, then 5 per iris: right iris 468–472,
/// left iris 473–477).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Landmark {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// One person in the photo.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Person {
    /// The face box (`None`: a person seen without a face, from behind or cut off).
    pub face: Option<NormRect>,
    /// How sure the detector was of the face (0 to 1; 0 without a face).
    pub face_score: f32,
    /// Dense landmarks (478 with the face mesh); empty when the face was too small or not a face to the mesh.
    pub landmarks: Vec<Landmark>,
    /// How sure the face mesh was that it saw a face (0 to 1).
    pub landmark_score: f32,
    /// The caller's person id from its [`FaceHint`].
    pub person_id: Option<String>,
    /// The box around everything of this person, in fractions of the photo.
    pub bounds: Option<NormRect>,
    /// Each part's planes; a part's mask is their per-pixel maximum. Parts with nothing in them are left out.
    pub parts: BTreeMap<Part, Vec<Plane>>,
}

impl Person {
    /// The value of `part` at a point of the photo (fractions), bilinear.
    pub fn sample(&self, part: Part, x: f32, y: f32) -> f32 {
        self.parts.get(&part).map_or(0.0, |planes| planes.iter().fold(0.0f32, |a, p| a.max(p.sample(x, y))))
    }

    /// Covered area of `part` as a fraction of the photo (planes of one part overlap rarely; this adds them).
    pub fn area(&self, part: Part) -> f32 {
        self.parts.get(&part).map_or(0.0, |planes| planes.iter().map(Plane::area).sum())
    }
}

/// Several people's `part` at a point, joined as [`crate::render::mask`] draws it: their split planes
/// ([`Plane::split`]) add up across people, the larger within a person; every other plane by the larger value.
pub fn joined<'a>(people: impl IntoIterator<Item = &'a Person>, part: Part, x: f32, y: f32) -> f32 {
    let (mut shared, mut rest) = (0.0f32, 0.0f32);
    for q in people {
        let planes = q.parts.get(&part).map_or(&[][..], Vec::as_slice);
        shared += planes.iter().filter(|p| p.split).fold(0.0f32, |a, p| a.max(p.sample(x, y)));
        rest = planes.iter().filter(|p| !p.split).fold(rest, |a, p| a.max(p.sample(x, y)));
    }
    shared.min(1.0).max(rest)
}

/// The result of parsing one photo.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Parsing {
    /// The model set that made it ([`crate::ModelSet::id`]) and its version ([`crate::ModelSet::version`]).
    pub model_set: String,
    pub model_version: String,
    /// The size of the picture that was parsed (its aspect ratio is the photo's).
    pub width: usize,
    pub height: usize,
    /// Largest first.
    pub people: Vec<Person>,
}

impl Parsing {
    /// The value at a point that is not any person (1 − everybody's Entire Person, joined as [`joined`] does).
    pub fn background(&self, x: f32, y: f32) -> f32 {
        1.0 - joined(&self.people, Part::EntirePerson, x, y)
    }
}
