//! Where each wrinkle area lies on a face: shapes built from the face mesh's 478 points (landmarks 5…482 of
//! [`crate::Face::landmarks`]), with sizes as fractions of the face's size.
//!
//! - **Forehead**: from just above the brows up past the top of the mesh (the hairline is cut by the Facial Skin
//!   mask), and between the brows (the frown lines down to the bridge of the nose).
//! - **Crow's Feet**: a fan beyond each eye's outer corner, starting a little past the corner so the lash line stays,
//!   and a thin crescent under each eye (the lower lid's lines).
//! - **Smile Lines** (the control keeps its id; the panel says "Cheeks and Smile Lines"): the band from each side of
//!   the nose down past the mouth corner, each cheek from under the eye to the jaw, and the chin below the lower lip.
//! - **Neck**: under the jaw line, down to about half a face below it (cut by the Body Skin mask).
//!
//! A face without the mesh (YuNet's five points only) has no areas, and is left alone.

use crate::FaceCtx;

/// Points the mesh adds after YuNet's five.
pub const MESH: usize = 478;
/// The first mesh point in [`crate::Face::landmarks`].
pub(super) const FIRST: usize = 5;

// Face mesh points (the subject's right is "r", left "l").
const EYE_OUTER_R: usize = 33;
const EYE_INNER_R: usize = 133;
const EYE_OUTER_L: usize = 263;
const EYE_INNER_L: usize = 362;
const CHIN: usize = 152;
const NOSE_WING_R: usize = 129;
const NOSE_WING_L: usize = 358;
const MOUTH_R: usize = 61;
const MOUTH_L: usize = 291;
/// The upper edge of the brows, from the subject's right temple to the left one.
const BROWS: [usize; 10] = [70, 63, 105, 66, 107, 336, 296, 334, 293, 300];
/// The top of the face's outline, right to left.
const OUTLINE_TOP: [usize; 11] = [21, 54, 103, 67, 109, 10, 338, 297, 332, 284, 251];
/// The jaw line, right to left.
const JAW: [usize; 15] = [58, 172, 136, 150, 149, 176, 148, 152, 377, 400, 378, 379, 365, 397, 288];
/// The face's outline from beside the eye down to the jaw near the chin, per side (the subject's right, then left):
/// the cheek's outer edge, running from the jaw up.
const CHEEK_OUTLINE: [[usize; 9]; 2] = [[149, 150, 136, 172, 58, 132, 93, 234, 127], [378, 379, 365, 397, 288, 361, 323, 454, 356]];
/// The chin's lower edge, from the subject's right to the left.
const CHIN_EDGE: [usize; 5] = [176, 148, 152, 377, 400];
const INNER_BROWS: [usize; 2] = [107, 336];
const INNER_EYES: [usize; 2] = [EYE_INNER_R, EYE_INNER_L];

/// Every mesh point the areas read (the tests place these).
pub const USED: [usize; 46] = [
    EYE_OUTER_R,
    EYE_INNER_R,
    EYE_OUTER_L,
    EYE_INNER_L,
    CHIN,
    NOSE_WING_R,
    NOSE_WING_L,
    MOUTH_R,
    MOUTH_L,
    70,
    63,
    105,
    66,
    107,
    336,
    296,
    334,
    293,
    300,
    21,
    54,
    103,
    67,
    109,
    10,
    338,
    297,
    332,
    284,
    251,
    58,
    172,
    136,
    150,
    149,
    176,
    148,
    377,
    400,
    378,
    379,
    365,
    397,
    288,
    1,
    4,
];

/// How far above the brows the forehead starts, a fraction of the face.
const BROW_GAP: f64 = 0.015;
/// How far above the mesh's top the forehead reaches, a fraction of the face.
const FOREHEAD_UP: f64 = 0.16;
/// The forehead's soft edge, a fraction of the face (the shape's edge is where its weight is ½).
const FOREHEAD_FEATHER: f64 = 0.035;
/// Crow's feet: the fan's centre beyond the outer corner, and its half-length along and across the eye's line.
const FAN_OUT: f64 = 0.05;
const FAN_ALONG: f64 = 0.075;
const FAN_ACROSS: f64 = 0.075;
const FAN_FEATHER: f64 = 0.03;
/// Crow's feet start this far past the outer corner (0 weight) and reach full weight at `FAN_FULL`.
const FAN_START: f64 = 0.004;
const FAN_FULL: f64 = 0.03;
/// Smile lines: the band's half-width, and how far its ends lie outside the nose wing and the mouth corner.
const FOLD_RADIUS: f64 = 0.042;
const FOLD_NOSE_OUT: f64 = 0.03;
const FOLD_MOUTH_OUT: f64 = 0.045;
const FOLD_MOUTH_DOWN: f64 = 0.05;
const FOLD_FEATHER: f64 = 0.025;
/// Between the brows: a capsule from above the brows' inner ends down to the bridge of the nose (radius, feather).
const GLABELLA_UP: f64 = 0.03;
const GLABELLA_DOWN: f64 = 0.015;
const GLABELLA_RADIUS: f64 = 0.045;
const GLABELLA_FEATHER: f64 = 0.02;
/// Under the eye: a crescent centred this far below the line between the eye's corners, a little longer than the
/// eye, this far across, with this soft edge.
const LID_DOWN: f64 = 0.05;
const LID_LONG: f64 = 0.02;
const LID_ACROSS: f64 = 0.03;
const LID_FEATHER: f64 = 0.02;
/// The cheeks and the chin: how far under the eyes the cheek starts, how far outside the nose wing and the mouth
/// corner its inner edge lies, how far below the mouth corner it reaches, and how far below the corners the chin starts.
const CHEEK_EYE_DOWN: (f64, f64) = (0.07, 0.09);
const CHEEK_OUT: f64 = 0.02;
const CHEEK_MOUTH_DOWN: f64 = 0.12;
const CHIN_DOWN: f64 = 0.06;
const CHEEK_FEATHER: f64 = 0.025;
/// Neck: how far below the jaw it reaches, and its soft edge.
const NECK_DOWN: f64 = 0.55;
const NECK_GAP: f64 = 0.01;
const NECK_FEATHER: f64 = 0.06;

pub(super) type P = (f64, f64);

/// Where mesh point `i` lies on a frontal face, in fractions of the face's box: the points the areas read, the
/// rest in the middle. For tests and examples that need a face mesh without the face mesh model.
pub fn frontal(i: usize) -> (f64, f64) {
    match i {
        33 => (0.25, 0.45),
        133 => (0.42, 0.45),
        263 => (0.75, 0.45),
        362 => (0.58, 0.45),
        152 => (0.5, 0.98),
        1 => (0.5, 0.62),
        4 => (0.5, 0.6),
        129 => (0.42, 0.64),
        358 => (0.58, 0.64),
        61 => (0.38, 0.78),
        291 => (0.62, 0.78),
        70 => (0.2, 0.36),
        63 => (0.26, 0.33),
        105 => (0.32, 0.32),
        66 => (0.38, 0.33),
        107 => (0.44, 0.35),
        336 => (0.56, 0.35),
        296 => (0.62, 0.33),
        334 => (0.68, 0.32),
        293 => (0.74, 0.33),
        300 => (0.8, 0.36),
        21 => (0.12, 0.3),
        54 => (0.17, 0.2),
        103 => (0.25, 0.12),
        67 => (0.33, 0.07),
        109 => (0.42, 0.04),
        10 => (0.5, 0.03),
        338 => (0.58, 0.04),
        297 => (0.67, 0.07),
        332 => (0.75, 0.12),
        284 => (0.83, 0.2),
        251 => (0.88, 0.3),
        58 => (0.15, 0.75),
        172 => (0.18, 0.82),
        136 => (0.22, 0.88),
        150 => (0.28, 0.92),
        149 => (0.33, 0.95),
        176 => (0.4, 0.97),
        148 => (0.45, 0.98),
        377 => (0.55, 0.98),
        400 => (0.6, 0.97),
        378 => (0.67, 0.95),
        379 => (0.72, 0.92),
        365 => (0.78, 0.88),
        397 => (0.82, 0.82),
        288 => (0.85, 0.75),
        // the rest of the outline (the ears lie beyond its sides) and the nostrils
        389 => (0.9, 0.36),
        356 => (0.93, 0.45),
        454 => (0.94, 0.55),
        323 => (0.92, 0.65),
        361 => (0.88, 0.7),
        162 => (0.1, 0.36),
        127 => (0.07, 0.45),
        234 => (0.06, 0.55),
        93 => (0.08, 0.65),
        132 => (0.12, 0.7),
        98 => (0.45, 0.66),
        240 => (0.44, 0.65),
        327 => (0.55, 0.66),
        460 => (0.56, 0.65),
        _ => (0.5, 0.5),
    }
}

/// The four wrinkle areas, one slider each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Forehead,
    CrowsFeet,
    SmileLines,
    Neck,
}

impl Area {
    pub const ALL: [Area; 4] = [Area::Forehead, Area::CrowsFeet, Area::SmileLines, Area::Neck];
}

/// A shape on the image, in pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Polygon(Vec<P>),
    /// Centre, unit axis of `a`, half-lengths `a` (along the axis) and `b` (across it).
    Ellipse {
        c: P,
        axis: P,
        a: f64,
        b: f64,
    },
    /// A segment with a radius.
    Capsule {
        p0: P,
        p1: P,
        r: f64,
    },
}

/// One region of an area: a shape with a soft edge, and an optional ramp that fades it in along a direction.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub shape: Shape,
    /// Half the width of the soft edge, in pixels: the weight goes from 0 to 1 over `2 × feather` across the shape's
    /// edge.
    pub feather: f64,
    /// (origin, unit direction, start, full): weight 0 before `start` pixels along the direction, 1 from `full`.
    pub ramp: Option<(P, P, f64, f64)>,
}

impl Region {
    /// The weight of the region at pixel centre `p` (0…1).
    pub fn weight(&self, p: P) -> f32 {
        let d = self.shape.inside(p);
        let mut w = smoothstep(-self.feather, self.feather, d);
        if let Some((o, dir, start, full)) = self.ramp {
            w *= smoothstep(start, full, (p.0 - o.0) * dir.0 + (p.1 - o.1) * dir.1);
        }
        if w.is_finite() { w.clamp(0.0, 1.0) as f32 } else { 0.0 }
    }

    /// The pixels where the weight can be above 0: x0, y0, x1, y1 (not clamped to the image).
    pub fn bounds(&self) -> [f64; 4] {
        let [x0, y0, x1, y1] = self.shape.bounds();
        let f = self.feather;
        [x0 - f, y0 - f, x1 + f, y1 + f]
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    if e1 <= e0 {
        return if x >= e1 { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Shape {
    /// The signed distance from `p` to the shape's edge: positive inside.
    pub fn inside(&self, p: P) -> f64 {
        match self {
            Shape::Polygon(pts) => polygon_inside(pts, p),
            Shape::Ellipse { c, axis, a, b } => {
                let (dx, dy) = (p.0 - c.0, p.1 - c.1);
                let (u, v) = (dx * axis.0 + dy * axis.1, -dx * axis.1 + dy * axis.0);
                let k = ((u / a).powi(2) + (v / b).powi(2)).sqrt();
                (1.0 - k) * a.min(*b)
            }
            Shape::Capsule { p0, p1, r } => r - segment_distance(p, *p0, *p1),
        }
    }

    pub fn bounds(&self) -> [f64; 4] {
        match self {
            Shape::Polygon(pts) => pts.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]
            }),
            Shape::Ellipse { c, a, b, .. } => {
                let r = a.max(*b);
                [c.0 - r, c.1 - r, c.0 + r, c.1 + r]
            }
            Shape::Capsule { p0, p1, r } => [p0.0.min(p1.0) - r, p0.1.min(p1.1) - r, p0.0.max(p1.0) + r, p0.1.max(p1.1) + r],
        }
    }
}

fn segment_distance(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

pub fn polygon_inside(pts: &[P], p: P) -> f64 {
    let n = pts.len();
    if n < 3 {
        return f64::NEG_INFINITY;
    }
    let mut inside = false;
    let mut dist = f64::INFINITY;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        dist = dist.min(segment_distance(p, a, b));
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) / (b.1 - a.1) * (b.0 - a.0) {
            inside = !inside;
        }
    }
    if inside { dist } else { -dist }
}

/// The regions of `area` on `face`, in the image's pixels; empty without the face mesh or when a point it needs
/// can't be mapped.
pub fn regions(face: &FaceCtx<'_>, area: Area) -> Vec<Region> {
    build(face, area).unwrap_or_default()
}

fn build(face: &FaceCtx<'_>, area: Area) -> Option<Vec<Region>> {
    if face.landmarks.len() < FIRST + MESH {
        return None;
    }
    let pt = |i: usize| face.landmarks.get(FIRST + i).copied().flatten();
    let s = face.size;
    let (er, el) = (pt(EYE_OUTER_R)?, pt(EYE_OUTER_L)?);
    let mid = ((er.0 + el.0) / 2.0, (er.1 + el.1) / 2.0);
    // the face's "down": from between the eyes to the chin
    let down = unit(sub(pt(CHIN)?, mid))?;
    let up = (-down.0, -down.1);
    let shift = |p: P, d: P, k: f64| (p.0 + d.0 * k * s, p.1 + d.1 * k * s);
    let regions = match area {
        Area::Forehead => {
            let brows: Option<Vec<P>> = BROWS.iter().map(|&i| pt(i).map(|p| shift(p, up, BROW_GAP))).collect();
            let top: Option<Vec<P>> = OUTLINE_TOP.iter().rev().map(|&i| pt(i).map(|p| shift(p, up, FOREHEAD_UP))).collect();
            let mut poly = brows?;
            poly.extend(top?);
            let mut regions = vec![Region { shape: Shape::Polygon(poly), feather: FOREHEAD_FEATHER * s, ramp: None }];
            // between the brows: down from above their inner ends to the bridge of the nose
            let mean = |ids: &[usize]| -> Option<P> {
                let ps: Option<Vec<P>> = ids.iter().map(|&i| pt(i)).collect();
                let ps = ps?;
                let n = ps.len().max(1) as f64;
                Some((ps.iter().map(|p| p.0).sum::<f64>() / n, ps.iter().map(|p| p.1).sum::<f64>() / n))
            };
            if let (Some(brow), Some(eyes)) = (mean(&INNER_BROWS), mean(&INNER_EYES)) {
                regions.push(Region {
                    shape: Shape::Capsule { p0: shift(brow, up, GLABELLA_UP), p1: shift(eyes, up, GLABELLA_DOWN), r: GLABELLA_RADIUS * s },
                    feather: GLABELLA_FEATHER * s,
                    ramp: None,
                });
            }
            regions
        }
        Area::CrowsFeet => {
            let mut regions: Vec<Region> = [(EYE_OUTER_R, EYE_INNER_R), (EYE_OUTER_L, EYE_INNER_L)]
                .iter()
                .filter_map(|&(outer, inner)| {
                    let c = pt(outer)?;
                    let out = unit(sub(c, pt(inner)?))?;
                    Some(Region {
                        shape: Shape::Ellipse { c: shift(c, out, FAN_OUT), axis: out, a: FAN_ALONG * s, b: FAN_ACROSS * s },
                        feather: FAN_FEATHER * s,
                        ramp: Some((c, out, FAN_START * s, FAN_FULL * s)),
                    })
                })
                .collect();
            // under each eye: a crescent below the lash line, along the eye
            for (outer, inner) in [(EYE_OUTER_R, EYE_INNER_R), (EYE_OUTER_L, EYE_INNER_L)] {
                let (Some(o), Some(i)) = (pt(outer), pt(inner)) else { continue };
                let (Some(axis), mid) = (unit(sub(o, i)), ((o.0 + i.0) / 2.0, (o.1 + i.1) / 2.0)) else { continue };
                let half = ((o.0 - i.0).powi(2) + (o.1 - i.1).powi(2)).sqrt() / 2.0 + LID_LONG * s;
                regions.push(Region {
                    shape: Shape::Ellipse { c: shift(mid, down, LID_DOWN), axis, a: half, b: LID_ACROSS * s },
                    feather: LID_FEATHER * s,
                    ramp: None,
                });
            }
            regions
        }
        Area::SmileLines => {
            let centre = pt(1).or(pt(4))?;
            // "out": away from the face's middle line, across the face's "down"
            let out_of = |w: P| {
                let side = sub(w, centre);
                let across = (down.1, -down.0);
                if side.0 * across.0 + side.1 * across.1 >= 0.0 { across } else { (-across.0, -across.1) }
            };
            let mut regions: Vec<Region> = [(NOSE_WING_R, MOUTH_R), (NOSE_WING_L, MOUTH_L)]
                .iter()
                .filter_map(|&(wing, corner)| {
                    let (w, c) = (pt(wing)?, pt(corner)?);
                    let out = out_of(w);
                    let p0 = shift(w, out, FOLD_NOSE_OUT);
                    let p1 = shift(shift(c, out, FOLD_MOUTH_OUT), down, FOLD_MOUTH_DOWN);
                    Some(Region { shape: Shape::Capsule { p0, p1, r: FOLD_RADIUS * s }, feather: FOLD_FEATHER * s, ramp: None })
                })
                .collect();
            // each cheek: from under the eye along the nose and the mouth to below its corner, out to the outline
            for (k, (eye_outer, eye_inner, wing, corner)) in
                [(EYE_OUTER_R, EYE_INNER_R, NOSE_WING_R, MOUTH_R), (EYE_OUTER_L, EYE_INNER_L, NOSE_WING_L, MOUTH_L)].into_iter().enumerate()
            {
                let (Some(eo), Some(ei), Some(w), Some(c)) = (pt(eye_outer), pt(eye_inner), pt(wing), pt(corner)) else { continue };
                let out = out_of(w);
                let outline: Option<Vec<P>> = CHEEK_OUTLINE[k].iter().map(|&i| pt(i)).collect();
                let Some(outline) = outline else { continue };
                let mut poly = vec![
                    shift(eo, down, CHEEK_EYE_DOWN.0),
                    shift(ei, down, CHEEK_EYE_DOWN.1),
                    shift(w, out, CHEEK_OUT),
                    shift(c, out, CHEEK_OUT),
                    shift(shift(c, out, CHEEK_OUT), down, CHEEK_MOUTH_DOWN),
                ];
                poly.extend(outline);
                regions.push(Region { shape: Shape::Polygon(poly), feather: CHEEK_FEATHER * s, ramp: None });
            }
            // the chin: below the mouth's corners, down to the jaw
            let (mr, ml) = (pt(MOUTH_R)?, pt(MOUTH_L)?);
            let edge: Option<Vec<P>> = CHIN_EDGE.iter().map(|&i| pt(i)).collect();
            if let Some(edge) = edge {
                let mut poly = vec![shift(mr, down, CHIN_DOWN), shift(ml, down, CHIN_DOWN)];
                poly.extend(edge.into_iter().rev());
                regions.push(Region { shape: Shape::Polygon(poly), feather: CHEEK_FEATHER * s, ramp: None });
            }
            regions
        }
        Area::Neck => {
            let jaw: Option<Vec<P>> = JAW.iter().map(|&i| pt(i).map(|p| shift(p, down, NECK_GAP))).collect();
            let jaw = jaw?;
            let mut poly = jaw.clone();
            poly.extend(jaw.iter().rev().map(|p| shift(*p, down, NECK_DOWN)));
            vec![Region { shape: Shape::Polygon(poly), feather: NECK_FEATHER * s, ramp: None }]
        }
    };
    Some(regions)
}

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn unit(v: P) -> Option<P> {
    let l = (v.0 * v.0 + v.1 * v.1).sqrt();
    (l.is_finite() && l > 1e-9).then(|| (v.0 / l, v.1 / l))
}
