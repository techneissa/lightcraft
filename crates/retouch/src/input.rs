//! What the retouching step knows about the people in a photo: per face a box, landmarks, an optional person and
//! part masks. Plain types: whoever finds faces or parses people (the engine's providers) fills them, and nothing
//! here depends on a model.
//!
//! Coordinates are normalised to the photo as the develop settings see it before the crop: 0…1 across its width
//! and height, y down, after the orientation, lens corrections and transform (the frame Remove spots use). The
//! retouching step maps them into the pixels of the image it works on (see [`crate::View`]).

use std::fmt;
use std::sync::OnceLock;

use lightcraft_raster::Plane;
use serde::{Deserialize, Serialize};

/// A part of a person a mask can cover. The order is fixed: it is part of the contract with the people parsers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Part {
    EntirePerson,
    FacialSkin,
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
    /// Anything else the parser gives the person (glasses, jewellery…).
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

    /// Title Case name, as the UI shows it.
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
}

/// A rectangle in normalised photo coordinates (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl NormRect {
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> NormRect {
        NormRect { x0, y0, x1, y1 }
    }

    pub fn centre(&self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    /// Every corner is a finite number and the box has an area.
    pub fn is_valid(&self) -> bool {
        [self.x0, self.y0, self.x1, self.y1].iter().all(|v| v.is_finite()) && self.x1 > self.x0 && self.y1 > self.y0
    }
}

/// A soft mask of one part: `plane` (0 = not the part, 1 = the part) covers `area` of the photo, at whatever
/// resolution the parser made it. Outside `area` the mask is 0.
#[derive(Clone, PartialEq)]
pub struct PartMask {
    pub plane: Plane,
    pub area: NormRect,
}

impl fmt::Debug for PartMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PartMask({}×{} over {:?})", self.plane.width, self.plane.height, self.area)
    }
}

impl PartMask {
    /// The mask at normalised point (`nx`, `ny`), bilinear between its samples; 0 outside its area, for a point
    /// that is not a number, or for an empty or malformed plane.
    pub fn sample(&self, nx: f64, ny: f64) -> f32 {
        let (w, h) = (self.plane.width, self.plane.height);
        if w == 0 || h == 0 || self.plane.data.len() != w.saturating_mul(h) || !self.area.is_valid() || !nx.is_finite() || !ny.is_finite() {
            return 0.0;
        }
        let (u, v) = ((nx - self.area.x0) / self.area.width(), (ny - self.area.y0) / self.area.height());
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return 0.0;
        }
        // sample centres at +0.5, held at the edge samples
        let fx = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
        let fy = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |x: usize, y: usize| self.plane.data.get(y * w + x).copied().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bottom - top) * ty
    }
}

/// The landmarks every face has first, in this order (YuNet's): the subject's right eye, left eye, nose tip, right
/// and left mouth corner ("right" is the subject's, so the right eye is on the left of the picture). A provider
/// with more landmarks appends them after these five.
pub const LANDMARKS: [&str; 5] = ["right eye", "left eye", "nose tip", "right mouth corner", "left mouth corner"];

/// One face (and the person it belongs to).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Face {
    /// The face's box.
    pub bounds: NormRect,
    /// The same box on the photo before the user's rotation (Rotate Left / Right, flips), when the provider knows
    /// it: per-face settings are kept by this one ([`Face::key_bounds`]), so a rotation never moves them to
    /// another face. Lens corrections and transform are not undone.
    pub upright: Option<NormRect>,
    /// [`LANDMARKS`] first, then any more the provider has. May lie slightly outside 0…1.
    pub landmarks: Vec<(f64, f64)>,
    /// How sure the detector was (0…1).
    pub score: f32,
    /// The person's name from the photo's named faces (People), trimmed and lower case; `None` when the face is
    /// not named. Per-face settings follow it ([`crate::FaceKey`]).
    pub person: Option<String>,
    /// The person's part masks, at most one per part. Empty until a people parser has run: every feature that
    /// needs a mask then leaves the picture alone.
    pub masks: Vec<(Part, PartMask)>,
}

impl Face {
    /// A face known only by its box (no landmarks, no person, no masks).
    pub fn boxed(bounds: NormRect) -> Face {
        Face { bounds, score: 1.0, ..Face::default() }
    }

    /// The box per-face settings are kept by: [`Face::upright`], else [`Face::bounds`].
    pub fn key_bounds(&self) -> NormRect {
        self.upright.filter(NormRect::is_valid).unwrap_or(self.bounds)
    }

    /// The mask of `part`, when the parser gave one.
    pub fn mask(&self, part: Part) -> Option<&PartMask> {
        self.masks.iter().find(|(p, _)| *p == part).map(|(_, m)| m)
    }

    /// Landmark `i` ([`LANDMARKS`] order), when the provider gave it and it is a number.
    pub fn landmark(&self, i: usize) -> Option<(f64, f64)> {
        self.landmarks.get(i).copied().filter(|(x, y)| x.is_finite() && y.is_finite())
    }
}

/// Every face of one photo, as the retouching step gets it. Faces come in the provider's order, which may change
/// between two detections; per-face settings never depend on it ([`crate::overrides`]).
#[derive(Clone, Default)]
pub struct Input {
    pub faces: Vec<Face>,
    fingerprint: OnceLock<u64>,
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Input").field("faces", &self.faces).finish()
    }
}

/// Inputs are equal when their fingerprints are: comparing masks pixel by pixel on every render would cost more
/// than the render.
impl PartialEq for Input {
    fn eq(&self, other: &Self) -> bool {
        self.fingerprint() == other.fingerprint()
    }
}

impl Input {
    pub fn new(faces: Vec<Face>) -> Input {
        Input { faces, fingerprint: OnceLock::new() }
    }

    /// A stable 64-bit hash of everything here (boxes, landmarks, persons, masks), for render cache keys: a
    /// render made with one input is never shown for another.
    pub fn fingerprint(&self) -> u64 {
        *self.fingerprint.get_or_init(|| {
            let mut h = Fnv::new();
            h.u64(self.faces.len() as u64);
            for f in &self.faces {
                let up = f.key_bounds();
                for v in [f.bounds.x0, f.bounds.y0, f.bounds.x1, f.bounds.y1, up.x0, up.y0, up.x1, up.y1] {
                    h.u64(v.to_bits());
                }
                h.u64(f.landmarks.len() as u64);
                for (x, y) in &f.landmarks {
                    h.u64(x.to_bits());
                    h.u64(y.to_bits());
                }
                h.u64(u64::from(f.score.to_bits()));
                h.bytes(f.person.as_deref().unwrap_or("\u{0}").as_bytes());
                h.u64(f.masks.len() as u64);
                for (part, m) in &f.masks {
                    h.u64(*part as u64);
                    for v in [m.area.x0, m.area.y0, m.area.x1, m.area.y1] {
                        h.u64(v.to_bits());
                    }
                    h.u64(m.plane.width as u64);
                    h.u64(m.plane.height as u64);
                    for v in &m.plane.data {
                        h.u64(u64::from(v.to_bits()));
                    }
                }
            }
            h.0
        })
    }
}

/// FNV-1a, 64 bits.
pub(crate) struct Fnv(pub u64);

impl Fnv {
    pub fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub fn bytes(&mut self, b: &[u8]) {
        for byte in b {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}
