//! Find People / Find Wires: candidates for the Remove tool, found locally.
//!
//! - [`people`]: a person mask from SAM 3's "person" prompt (or a rough body shape from a detected face) becomes one
//!   candidate per person; the main subject (largest and most central) is marked and not proposed by default.
//! - [`wires`]: a pure-Rust thin-line detector for power lines and cables against smooth areas such as sky (the fallback
//!   when SAM 3 is not installed; with it, wires come from its "power line" mask like people do).
//! - [`cover`]: turns a candidate's shape into content-aware remove spots (dabs of one size on a grid or along a path)
//!   that cover it, grown a little.
//!
//! Everything here is plain data in, plain data out (no model, no session), so it is tested on procedural images.

pub mod cover;
pub mod people;
pub mod shape;
pub mod wires;

#[cfg(test)]
mod tests;

/// What a candidate is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Person,
    Wire,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Person => "person",
            Kind::Wire => "wire",
        }
    }
}

/// One spot that removes (part of) a candidate: dab centres (0..1 of the photo's width and height) and their radius
/// (a fraction of the long edge), as a [`lightcraft_develop::Spot`] holds them.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotPlan {
    pub points: Vec<(f64, f64)>,
    pub size: f64,
}

/// Something the Remove tool can take out.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub kind: Kind,
    /// How it was found: `sam3`, `faces` (a rough body shape from a face) or `lines`.
    pub source: &'static str,
    /// The outline to draw (0..1 of the photo): a ring for a person, a line for a wire.
    pub outline: Vec<(f64, f64)>,
    pub closed: bool,
    /// `[x0, y0, x1, y1]`, 0..1.
    pub bbox: [f64; 4],
    /// Share of the photo it covers (0..1).
    pub area: f64,
    /// How sure the detector is (0..1).
    pub score: f32,
    /// The photo's main subject: not proposed unless asked for.
    pub main: bool,
    /// The spots that remove it.
    pub spots: Vec<SpotPlan>,
}

impl Candidate {
    /// Whether a click at `(x, y)` (0..1) picks this candidate; `tol` (0..1 of the long edge) is how close to a wire.
    pub fn hit(&self, x: f64, y: f64, tol: f64) -> bool {
        if self.closed {
            shape::in_polygon(&self.outline, x, y) || shape::distance_to_polyline(&self.outline, x, y) <= tol
        } else {
            shape::distance_to_polyline(&self.outline, x, y) <= tol
        }
    }
}

/// Feather of the spots made here (the default is 50): the dabs' full strength reaches 80 % of their radius, which
/// is what the grid spacing in [`cover`] relies on.
pub const SPOT_FEATHER: f64 = 25.0;
