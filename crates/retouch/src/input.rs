//! What the retouching step knows about the people in a photo: per face a box, landmarks, an optional person and
//! part masks. Plain types: whoever finds faces or parses people (the engine's providers) fills them, and nothing
//! here depends on a model.
//!
//! Coordinates are normalised to the photo as the develop settings see it before the crop: 0…1 across its width
//! and height, y down, after the orientation, lens corrections and transform (the frame Remove spots use). The
//! retouching step maps them into the pixels of the image it works on (see [`crate::View`]).

use std::fmt;
use std::sync::{Arc, OnceLock};

use lightcraft_raster::{Plane, Rgb32f};
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

    /// The mask for sampling at many points (a feature's every pixel): the plane's values made finite and clamped
    /// to 0…1 once, the area's scale worked out once. It gives [`PartMask::sample`]'s value to within rounding.
    /// `None` for an empty or malformed plane or area (where `sample` gives 0 everywhere).
    pub fn sampler(&self) -> Option<MaskSampler> {
        let (w, h) = (self.plane.width, self.plane.height);
        if w == 0 || h == 0 || self.plane.data.len() != w.saturating_mul(h) || !self.area.is_valid() {
            return None;
        }
        let data = self.plane.data.iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }).collect();
        let a = self.area;
        Some(MaskSampler { data, w, h, origin: [a.x0, a.y0], scale: [w as f64 / a.width(), h as f64 / a.height()] })
    }
}

/// A part mask ready to sample at many points ([`PartMask::sampler`]).
#[derive(Clone, Debug)]
pub struct MaskSampler {
    data: Vec<f32>,
    w: usize,
    h: usize,
    origin: [f64; 2],
    /// Samples per normalised unit, across and down.
    scale: [f64; 2],
}

impl MaskSampler {
    /// The mask at normalised point (`nx`, `ny`): as [`PartMask::sample`].
    pub fn at(&self, nx: f64, ny: f64) -> f32 {
        let (w, h) = (self.w, self.h);
        let (sx, sy) = ((nx - self.origin[0]) * self.scale[0], (ny - self.origin[1]) * self.scale[1]);
        // (also false for a point that is not a number)
        if !((0.0..=w as f64).contains(&sx) && (0.0..=h as f64).contains(&sy)) {
            return 0.0;
        }
        let fx = (sx - 0.5).clamp(0.0, (w - 1) as f64);
        let fy = (sy - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |x: usize, y: usize| self.data.get(y * w + x).copied().unwrap_or(0.0);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let top = a + (b - a) * tx;
        let bottom = c + (d - c) * tx;
        top + (bottom - top) * ty
    }
}

/// A change made for one face by a model run outside the render (Glasses Glare's repair, `fork/glare-repair.md`):
/// over `area` of the photo, each pixel is multiplied by `gain` (per channel), weighted by `alpha` (0 = unchanged).
/// `gain` and `alpha` are the same size and cover `area` as a [`PartMask`] does. A gain (not new pixels) keeps the
/// user's later edits: exposure and white balance act on the repaired pixels as on the rest.
#[derive(Clone, PartialEq)]
pub struct Patch {
    pub area: NormRect,
    pub gain: Rgb32f,
    pub alpha: Plane,
    /// Names this patch's content (a hash of what made it): part of [`Input::fingerprint`] in place of its pixels.
    pub stamp: u64,
}

impl fmt::Debug for Patch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Patch({}×{} over {:?}, {:016x})", self.alpha.width, self.alpha.height, self.area, self.stamp)
    }
}

impl Patch {
    /// The weight and gain at normalised point (`nx`, `ny`), bilinear between samples; `None` outside the area, for a
    /// point that is not a number, a malformed patch, or where the weight is 0.
    pub fn sample(&self, nx: f64, ny: f64) -> Option<(f32, [f32; 3])> {
        let (w, h) = (self.alpha.width, self.alpha.height);
        let n = w.checked_mul(h)?;
        if n == 0 || self.alpha.data.len() != n || self.gain.width != w || self.gain.height != h || self.gain.data.len() != n {
            return None;
        }
        if !self.area.is_valid() || !nx.is_finite() || !ny.is_finite() {
            return None;
        }
        let (u, v) = ((nx - self.area.x0) / self.area.width(), (ny - self.area.y0) / self.area.height());
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let fx = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
        let fy = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let wts = [(x0, y0, (1.0 - tx) * (1.0 - ty)), (x1, y0, tx * (1.0 - ty)), (x0, y1, (1.0 - tx) * ty), (x1, y1, tx * ty)];
        let (mut a, mut g) = (0.0f32, [0.0f32; 3]);
        for (x, y, k) in wts {
            let i = y * w + x;
            let av = self.alpha.data.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0);
            let gv = self.gain.data.get(i).copied().unwrap_or([1.0; 3]);
            a += av * k;
            for c in 0..3 {
                let v = gv[c];
                g[c] += if v.is_finite() { v.clamp(0.0, GAIN_MAX) } else { 1.0 } * k;
            }
        }
        (a > 0.0).then_some((a, g))
    }
}

/// The largest gain a [`Patch`] applies.
pub const GAIN_MAX: f32 = 16.0;

/// The landmarks every face has first, in this order (YuNet's): the subject's right eye, left eye, nose tip, right
/// and left mouth corner ("right" is the subject's, so the right eye is on the left of the picture). A provider
/// with more landmarks appends them after these five.
pub const LANDMARKS: [&str; 5] = ["right eye", "left eye", "nose tip", "right mouth corner", "left mouth corner"];

/// The frame a face's key box ([`Face::key_bounds`], [`crate::FaceKey::bounds`]) is in. Both are before the user's
/// rotation; they differ by the lens corrections and transform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyFrame {
    /// After lens corrections and transform: faces found on the developed photo (YuNet), and every key saved
    /// before the frame was kept.
    #[default]
    Edit,
    /// Before lens corrections and transform: faces from the people parse ([`Input::on_photo`]).
    Photo,
}

impl KeyFrame {
    pub fn is_edit(&self) -> bool {
        *self == KeyFrame::Edit
    }
}

/// Maps a point of the [`KeyFrame::Edit`] frame to the [`KeyFrame::Photo`] frame (both normalised, before the user's
/// rotation) through the photo's lens corrections and transform, so keys of both frames can be compared. `None`
/// for a point the warp can't map.
pub trait KeyMap: Sync {
    fn edit_to_photo(&self, x: f64, y: f64) -> Option<(f64, f64)>;
}

/// The box around `b`'s corners, edge midpoints and centre after `map`; `None` when one doesn't map or the result
/// is not a box.
pub fn edit_box_to_photo(map: &dyn KeyMap, b: &NormRect) -> Option<NormRect> {
    let (cx, cy) = b.centre();
    let points = [(b.x0, b.y0), (cx, b.y0), (b.x1, b.y0), (b.x0, cy), (cx, cy), (b.x1, cy), (b.x0, b.y1), (cx, b.y1), (b.x1, b.y1)];
    let mut out = NormRect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (x, y) in points {
        let (px, py) = map.edit_to_photo(x, y)?;
        out = NormRect::new(out.x0.min(px), out.y0.min(py), out.x1.max(px), out.y1.max(py));
    }
    out.is_valid().then_some(out)
}

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
    /// The frame [`Face::key_bounds`] is in: [`KeyFrame::Photo`] for faces from the people parse.
    pub key_frame: KeyFrame,
    /// The glasses repair made for this face (Glasses Glare), in the frame of its masks; `None` until one is made.
    pub patch: Option<Arc<Patch>>,
    /// The spots on the face's skin (Blemishes), found once by the provider on the picture its masks come from
    /// ([`crate::blemish::with_spots`]). `None`: not looked for; Blemishes then looks on the image it retouches.
    pub spots: Option<Spots>,
    /// The stray strands around the face and across it (Stray Hair), found once by the provider on the picture its
    /// masks come from ([`crate::hair::with_strands`]). `None`: not looked for yet; Stray Hair then changes nothing
    /// (it never searches in a render).
    pub strands: Option<Strands>,
}

/// The stray strands found for one face (Stray Hair): a grid of cells over `area`, stored only for the tiles of
/// [`Strands::TILE`] × [`Strands::TILE`] cells that hold a strand. The render repairs the cells with a weight from
/// the pixels beside them, across the strand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strands {
    /// The rectangle the grid covers, normalised like the face's box (same frame).
    pub area: NormRect,
    /// Cells across and down.
    pub width: usize,
    pub height: usize,
    /// The searched picture's size in pixels: the frame `across` and the distances below are measured in.
    pub picture: (usize, usize),
    /// How far across a strand its sides are read, and the spacing of the samples along it, in the searched
    /// picture's pixels.
    pub side: f64,
    pub step: f64,
    /// Per tile, row by row (`width` and `height` divided by [`Strands::TILE`], rounded up): its place in `cells`
    /// divided by `TILE²`, or `u32::MAX` for a tile without a strand.
    pub tiles: Vec<u32>,
    /// `TILE²` cells per stored tile, row by row: the weight (0…255 for 0…1), the direction across the strand
    /// (0…255 for 0…π, in the searched picture's pixels, x right and y down) and the polarity (0 none, 1 darker than
    /// its sides, 2 lighter).
    pub cells: Vec<[u8; 3]>,
    /// How the search went: what a readout says when no strand was found.
    pub searched: Searched,
    /// The strands found: runs of cells above half a weight that are long enough to be a strand.
    pub count: u32,
    /// The face's size in the searched picture's pixels, and the search's time in milliseconds (a readout; the
    /// time is not part of [`Input::fingerprint`]).
    pub face_px: u32,
    pub ms: u32,
    /// Of `ms`, what cutting and reducing the head's crop took.
    pub crop_ms: u32,
}

/// How a face's strand search went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Searched {
    /// Searched (maybe nothing found).
    #[default]
    Yes,
    /// The face is under [`crate::hair::MIN_FACE_PX`] pixels in the searched picture: its strands are thinner than a
    /// pixel there.
    TooSmall,
    /// The face has neither a Hair nor a Facial Skin mask: there is nowhere to look.
    NoParts,
}

impl Strands {
    /// A tile's side, in cells.
    pub const TILE: usize = 16;

    /// The tiles across and down.
    pub fn tiles_size(&self) -> (usize, usize) {
        (self.width.div_ceil(Self::TILE), self.height.div_ceil(Self::TILE))
    }

    /// Cell (`x`, `y`): weight, direction and polarity; `None` outside the grid or in a tile without a strand.
    pub fn cell(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let (tw, _) = self.tiles_size();
        let t = *self.tiles.get((y / Self::TILE).saturating_mul(tw).saturating_add(x / Self::TILE))?;
        if t == u32::MAX {
            return None;
        }
        let i = (t as usize).saturating_mul(Self::TILE * Self::TILE).saturating_add((y % Self::TILE) * Self::TILE + x % Self::TILE);
        self.cells.get(i).copied()
    }

    /// Whether no strand was found.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// What a spot on the skin is ([`Spot::kind`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpotKind {
    /// A blemish: a pimple, a red or dark mark. Always removed.
    #[default]
    Blemish,
    /// A mole: darker, round, sharp-edged and brown. Kept while Keep Moles and Freckles is on.
    Mole,
    /// One of many small faint brown spots grouped together. Kept while Keep Moles and Freckles is on.
    Freckle,
}

/// One spot on a face's skin. Points are normalised like the face's box (same frame); sizes are fractions of the
/// face's size ([`crate::FaceCtx::size`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spot {
    pub at: (f64, f64),
    /// The spot's radius, a fraction of the face's size.
    pub radius: f64,
    /// Where its skin texture is taken from (the centre of a clean patch of skin next to it); `None` when no patch
    /// next to it is all skin (the spot is then filled from the skin around it alone).
    pub donor: Option<(f64, f64)>,
    pub kind: SpotKind,
    /// How much it stands out (0…1): a low Amount removes the strongest spots only.
    pub strength: f32,
}

/// The spots found on one face, and the area of its skin they were looked for on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spots {
    pub list: Vec<Spot>,
    /// The face's skin area (its Facial Skin mask's sum), in units of the face's size squared.
    pub skin: f64,
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
    /// The coordinates are on the photo before lens corrections and transform (after the orientation): the frame a
    /// people parse is made in. The step maps them through the lens and transform warp, as it maps local masks drawn
    /// on the photo. `false`: after lens corrections and transform (the module docs' frame).
    pub on_photo: bool,
    /// The picture Background Swap uses, once the engine has loaded it ([`Input::with_backdrop`]).
    pub backdrop: Option<crate::background::Backdrop>,
    /// The photo's depth map, when its file has one, in the faces' frame ([`Input::with_depth`]).
    pub depth: Option<crate::background::Depth>,
    fingerprint: OnceLock<u64>,
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Input")
            .field("faces", &self.faces)
            .field("on_photo", &self.on_photo)
            .field("backdrop", &self.backdrop)
            .field("depth", &self.depth)
            .finish()
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
        Input { faces, on_photo: false, backdrop: None, depth: None, fingerprint: OnceLock::new() }
    }

    /// Faces whose coordinates are on the photo before lens corrections and transform ([`Input::on_photo`]).
    pub fn new_on_photo(faces: Vec<Face>) -> Input {
        Input { faces, on_photo: true, backdrop: None, depth: None, fingerprint: OnceLock::new() }
    }

    /// These faces with Background Swap's picture (`None`: without one). Copies the faces.
    pub fn with_backdrop(&self, backdrop: Option<crate::background::Backdrop>) -> Input {
        Input { faces: self.faces.clone(), on_photo: self.on_photo, backdrop, depth: self.depth.clone(), fingerprint: OnceLock::new() }
    }

    /// These faces with the photo's depth map. Copies the faces.
    pub fn with_depth(&self, depth: Option<crate::background::Depth>) -> Input {
        Input { faces: self.faces.clone(), on_photo: self.on_photo, backdrop: self.backdrop.clone(), depth, fingerprint: OnceLock::new() }
    }

    /// A stable 64-bit hash of everything here (boxes, landmarks, persons, masks), for render cache keys: a
    /// render made with one input is never shown for another.
    pub fn fingerprint(&self) -> u64 {
        *self.fingerprint.get_or_init(|| {
            let mut h = Fnv::new();
            h.u64(self.faces.len() as u64);
            h.u64(u64::from(self.on_photo));
            // (only when there: an input without them keeps the fingerprint it had before they existed)
            if let Some(b) = &self.backdrop {
                h.bytes(b"backdrop");
                h.u64(b.key);
            }
            if let Some(d) = &self.depth {
                h.bytes(b"depth");
                h.u64(d.key);
            }
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
                h.u64(u64::from(f.key_frame == KeyFrame::Photo));
                match &f.spots {
                    None => h.u64(u64::MAX),
                    Some(s) => {
                        h.u64(s.list.len() as u64);
                        h.u64(s.skin.to_bits());
                        for p in &s.list {
                            let (dx, dy) = p.donor.unwrap_or((f64::NAN, f64::NAN));
                            for v in [p.at.0, p.at.1, p.radius, dx, dy] {
                                h.u64(v.to_bits());
                            }
                            h.u64(p.kind as u64);
                            h.u64(u64::from(p.strength.to_bits()));
                        }
                    }
                }
                match &f.strands {
                    None => h.u64(u64::MAX),
                    Some(s) => {
                        for v in [s.area.x0, s.area.y0, s.area.x1, s.area.y1, s.side, s.step] {
                            h.u64(v.to_bits());
                        }
                        for v in [s.width, s.height, s.picture.0, s.picture.1, s.tiles.len(), s.cells.len(), s.searched as usize, s.count as usize] {
                            h.u64(v as u64);
                        }
                        for t in &s.tiles {
                            h.u64(u64::from(*t));
                        }
                        for c in &s.cells {
                            h.bytes(c);
                        }
                    }
                }
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
                match &f.patch {
                    None => h.u64(0),
                    Some(p) => {
                        h.u64(1);
                        h.u64(p.stamp);
                        for v in [p.area.x0, p.area.y0, p.area.x1, p.area.y1] {
                            h.u64(v.to_bits());
                        }
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
