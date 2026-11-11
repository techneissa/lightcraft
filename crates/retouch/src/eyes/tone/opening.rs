//! The eye's opening: the outline between the upper and the lower lid, from the face mesh's eye contour.

use super::super::geom::{Eye, smoothstep};
use crate::FaceCtx;

/// Where the face mesh's points start in the landmarks (after YuNet's five).
const MESH: usize = 5;
/// Points the mesh has.
const MESH_POINTS: usize = 478;

/// One eye's contour in the face mesh (MediaPipe's numbering): from the outer corner along the upper lid to the inner
/// corner, then back along the lower lid. The subject's right eye (on the left of the picture), then the left one, as
/// [`super::super::geom::eyes`] gives them.
const CONTOURS: [[usize; 16]; 2] = [
    [33, 246, 161, 160, 159, 158, 157, 173, 133, 155, 154, 153, 145, 144, 163, 7],
    [263, 466, 388, 387, 386, 385, 384, 398, 362, 382, 381, 380, 374, 373, 390, 249],
];

type P = (f64, f64);

/// The outline's first this many stretches (from the outer corner to the inner one) are the upper lid.
const UPPER_EDGES: usize = 8;

/// Where a point lies against the nearer opening: how far inside its outline, in half eye widths (negative outside),
/// and whether the nearest stretch of the outline is the lower lid.
#[derive(Clone, Copy)]
pub struct Depth {
    pub d: f64,
    pub lower: bool,
}

/// How far past the outline (out over the lid) a change fades out, and how far inside it is whole, in half eye
/// widths. The mesh's contour lies on the lid's edge, give or take a few hundredths.
pub type Fade = (f64, f64);

/// One eye's opening in the pixels of the image.
pub struct Opening {
    outline: Vec<P>,
    /// The outline's box: x0, y0, x1, y1.
    bounds: (f64, f64, f64, f64),
    /// The eye's half width, pixels.
    half: f64,
}

impl Opening {
    /// How far (`x`, `y`) lies inside the outline, in pixels (negative outside), and whether the nearest stretch of
    /// it is the lower lid.
    fn depth(&self, x: f64, y: f64) -> (f64, bool) {
        let n = self.outline.len();
        let (mut inside, mut best, mut lower) = (false, f64::INFINITY, false);
        for i in 0..n {
            let (Some(&a), Some(&b)) = (self.outline.get(i), self.outline.get((i + 1) % n)) else { continue };
            if (a.1 > y) != (b.1 > y) && x < a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0) {
                inside = !inside;
            }
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = (x - a.0 - t * dx).hypot(y - a.1 - t * dy);
            if d < best {
                best = d;
                lower = i >= UPPER_EDGES;
            }
        }
        (if inside { best } else { -best }, lower)
    }
}

/// The face's two openings, in the order of `eyes`; empty without the face mesh or when the eyes aren't placed.
pub fn of(face: &FaceCtx<'_>, eyes: &[Eye]) -> Vec<Opening> {
    if face.landmarks.len() < MESH + MESH_POINTS || eyes.len() != CONTOURS.len() {
        return Vec::new();
    }
    let lm = |i: usize| face.landmarks.get(MESH + i).copied().flatten();
    let mut out = Vec::with_capacity(2);
    for (contour, eye) in CONTOURS.iter().zip(eyes) {
        let Some(outline) = contour.iter().map(|&i| lm(i)).collect::<Option<Vec<P>>>() else { return Vec::new() };
        // a contour thrown far from its eye (a bad fit) is no outline: then the masks alone say where
        let far = outline.iter().any(|p| {
            let (a, b) = eye.local(p.0, p.1);
            !(a.abs() < 1.6 && b.abs() < 1.2)
        });
        if far {
            return Vec::new();
        }
        let bounds = outline.iter().fold((f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, p| {
            (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1))
        });
        out.push(Opening { outline, bounds, half: eye.half });
    }
    out
}

/// Past this many half eye widths outside an outline's box, a point is just "far outside": no use measures further.
const FAR: f64 = 0.3;

/// Where (`x`, `y`) lies against the nearer opening (at least −[`FAR`] deep for a point well away from both); `None`
/// without openings.
pub fn depth(openings: &[Opening], x: f64, y: f64) -> Option<Depth> {
    openings
        .iter()
        .map(|o| {
            let (x0, y0, x1, y1) = o.bounds;
            let pad = FAR * o.half;
            if x < x0 - pad || x > x1 + pad || y < y0 - pad || y > y1 + pad {
                return Depth { d: -FAR, lower: false };
            }
            let (d, lower) = o.depth(x, y);
            Depth { d: (d / o.half.max(1e-9)).max(-FAR), lower }
        })
        .reduce(|a, b| if b.d > a.d { b } else { a })
}

/// The weight at `depth` ([`depth`]): 1 from `fade.1` half eye widths inside the outline, fading to 0 at `fade.0`
/// outside it, by the upper lid's `upper` or the lower lid's `lower`; 1 everywhere without openings.
pub fn fade(depth: Option<Depth>, upper: Fade, lower: Fade) -> f32 {
    depth.map_or(1.0, |d| {
        let f = if d.lower { lower } else { upper };
        smoothstep(-f.0, f.1, d.d) as f32
    })
}
