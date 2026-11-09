//! Where each eye is, from the face's landmarks, in the pixels of the image being retouched: its corners, its lower
//! lid and its iris (from the 478 face mesh points when the face has them), else from YuNet's two eye points.

use crate::FaceCtx;

/// Where the face mesh's points start in the landmarks (after YuNet's five).
const MESH: usize = 5;
/// Points the mesh has.
const MESH_POINTS: usize = 478;

/// One eye's points in the face mesh (MediaPipe's numbering): the corners, the lower lid from the outer corner to the
/// inner one, the iris centre and the four points on its rim.
struct MeshEye {
    outer: usize,
    inner: usize,
    lower: [usize; 7],
    iris: usize,
    rim: [usize; 4],
}

/// The subject's right eye (on the left of the picture), then the left one.
const MESH_EYES: [MeshEye; 2] = [
    MeshEye { outer: 33, inner: 133, lower: [7, 163, 144, 145, 153, 154, 155], iris: 468, rim: [469, 470, 471, 472] },
    MeshEye { outer: 263, inner: 362, lower: [249, 390, 373, 374, 380, 381, 382], iris: 473, rim: [474, 475, 476, 477] },
];

/// An eye's half width, as a share of its distance to the other eye, when only YuNet's eye points are known.
const HALF_OF_SPACING: f64 = 0.22;
/// How far below the eye's centre line its lower lid is, in half widths, when only YuNet's points are known.
const LID_DROP: f64 = 0.3;
/// The eye's half width is kept within this share of the face's size, whatever the landmarks say.
const HALF_RANGE: (f64, f64) = (0.03, 0.16);
/// The half width used for the iris's sharpening when no eye could be placed (a share of the face's size).
const HALF_DEFAULT: f64 = 0.09;

type P = (f64, f64);

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn dot(a: P, b: P) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

fn len(a: P) -> f64 {
    dot(a, a).sqrt()
}

pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    if t.is_finite() { t * t * (3.0 - 2.0 * t) } else { 0.0 }
}

/// 0 below `lo0`, rising to 1 at `lo1`, 1 up to `hi1`, falling to 0 at `hi0`.
pub fn window(x: f64, lo0: f64, lo1: f64, hi1: f64, hi0: f64) -> f64 {
    smoothstep(lo0, lo1, x) * (1.0 - smoothstep(hi1, hi0, x))
}

/// One eye, in the pixels of the image.
#[derive(Clone, Debug, PartialEq)]
pub struct Eye {
    /// Halfway between the corners.
    pub centre: P,
    /// Unit vector from the outer corner to the inner one.
    pub across: P,
    /// Unit vector down the face, square to `across`.
    pub down: P,
    /// Half the corner-to-corner width, in pixels.
    pub half: f64,
    /// The lower lid in eye-local coordinates (`a`, `b`: across and down, in half widths from the centre), sorted
    /// by `a`, corners included.
    lid: Vec<P>,
    /// The iris's centre and radius in pixels, from the mesh.
    pub iris: Option<(P, f64)>,
}

impl Eye {
    /// Pixel (`x`, `y`) in eye-local coordinates: across (−1 outer corner, +1 inner) and down, in half widths.
    pub fn local(&self, x: f64, y: f64) -> P {
        let d = sub((x, y), self.centre);
        (dot(d, self.across) / self.half, dot(d, self.down) / self.half)
    }

    /// Eye-local (`a`, `b`) as a pixel position.
    pub fn to_px(&self, a: f64, b: f64) -> P {
        let (a, b) = (a * self.half, b * self.half);
        (self.centre.0 + a * self.across.0 + b * self.down.0, self.centre.1 + a * self.across.1 + b * self.down.1)
    }

    /// How far below the lower lid (in half widths) eye-local (`a`, `b`) is; negative above it.
    pub fn below_lid(&self, a: f64, b: f64) -> f64 {
        b - self.lid_at(a)
    }

    /// The lower lid's `b` at `a` (held at the corners' beyond them).
    fn lid_at(&self, a: f64) -> f64 {
        let (Some(first), Some(last)) = (self.lid.first(), self.lid.last()) else { return 0.0 };
        if a <= first.0 {
            return first.1;
        }
        if a >= last.0 {
            return last.1;
        }
        for w in self.lid.windows(2) {
            if let [p, q] = w
                && a <= q.0
            {
                let t = if q.0 > p.0 { (a - p.0) / (q.0 - p.0) } else { 0.0 };
                return p.1 + (q.1 - p.1) * t;
            }
        }
        last.1
    }

    /// How far the lower lid drops below the corners' line at most, in half widths.
    pub fn lid_drop(&self) -> f64 {
        self.lid.iter().map(|p| p.1).fold(0.0f64, f64::max)
    }

    /// The pixel box `[x0, y0, x1, y1)` holding eye-local box `[a0, a1, b0, b1]` (the lid's drop added below), grown
    /// by `pad` pixels and clamped to a `w × h` image; `None` when nothing of it is in the image.
    pub fn pixel_box(&self, local: [f64; 4], pad: f64, w: usize, h: usize) -> Option<[usize; 4]> {
        let drop = self.lid_drop();
        let [a0, a1, b0, b1] = local;
        let corners = [self.to_px(a0, b0), self.to_px(a1, b0), self.to_px(a0, b1 + drop), self.to_px(a1, b1 + drop)];
        let fold = |f: fn(f64, f64) -> f64, pick: fn(&P) -> f64, start: f64| corners.iter().map(pick).fold(start, f);
        let (x0, x1) = (fold(f64::min, |p| p.0, f64::INFINITY) - pad, fold(f64::max, |p| p.0, f64::NEG_INFINITY) + pad);
        let (y0, y1) = (fold(f64::min, |p| p.1, f64::INFINITY) - pad, fold(f64::max, |p| p.1, f64::NEG_INFINITY) + pad);
        if ![x0, x1, y0, y1].iter().all(|v| v.is_finite()) {
            return None;
        }
        let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
        let b = [clamp(x0.floor(), w), clamp(y0.floor(), h), clamp(x1.ceil(), w), clamp(y1.ceil(), h)];
        (b[0] < b[2] && b[1] < b[3]).then_some(b)
    }
}

/// The face's eyes that can be placed: both or none (one eye's landmarks alone can't say how wide it is).
pub fn eyes(face: &FaceCtx<'_>) -> Vec<Eye> {
    let lm = |i: usize| face.landmarks.get(i).copied().flatten();
    let (lo, hi) = (face.px(HALF_RANGE.0), face.px(HALF_RANGE.1));
    if !(lo > 0.0 && hi > lo) {
        return Vec::new();
    }
    // the mouth, to tell down from up (YuNet's mouth corners, else the mesh's lip centre)
    let mouth = match (lm(3), lm(4)) {
        (Some(a), Some(b)) => Some(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)),
        _ => lm(MESH + 13),
    };
    let mesh = face.landmarks.len() >= MESH + MESH_POINTS;
    let placed: Option<Vec<Eye>> = if mesh { MESH_EYES.iter().map(|m| from_mesh(m, &lm, mouth, lo, hi)).collect() } else { None };
    if let Some(e) = placed {
        return e;
    }
    let (Some(r), Some(l)) = (lm(0), lm(1)) else { return Vec::new() };
    let spacing = len(sub(l, r));
    if !(spacing.is_finite() && spacing > 0.0) {
        return Vec::new();
    }
    let half = (spacing * HALF_OF_SPACING).clamp(lo, hi);
    [(r, l), (l, r)].iter().filter_map(|&(c, other)| from_points(c, other, half, mouth)).collect()
}

/// The half width used for sizes when no eye is placed, else the eyes' mean.
pub fn eye_scale(face: &FaceCtx<'_>, eyes: &[Eye]) -> f64 {
    if eyes.is_empty() { face.px(HALF_DEFAULT) } else { eyes.iter().map(|e| e.half).sum::<f64>() / eyes.len() as f64 }
}

/// Down the face: square to `across`, on the mouth's side (the image's down without a mouth).
fn down_of(across: P, centre: P, mouth: Option<P>) -> P {
    let n = (-across.1, across.0);
    let flip = match mouth {
        Some(m) => dot(sub(m, centre), n) < 0.0,
        None => n.1 < 0.0,
    };
    if flip { (-n.0, -n.1) } else { n }
}

fn from_mesh(m: &MeshEye, lm: &dyn Fn(usize) -> Option<P>, mouth: Option<P>, lo: f64, hi: f64) -> Option<Eye> {
    let at = |i: usize| lm(MESH + i);
    let (outer, inner) = (at(m.outer)?, at(m.inner)?);
    let span = sub(inner, outer);
    let width = len(span);
    if !(width.is_finite() && width > 0.0) {
        return None;
    }
    let across = (span.0 / width, span.1 / width);
    let centre = ((outer.0 + inner.0) / 2.0, (outer.1 + inner.1) / 2.0);
    let half = (width / 2.0).clamp(lo, hi);
    let down = down_of(across, centre, mouth);
    let mut eye = Eye { centre, across, down, half, lid: Vec::new(), iris: None };
    let mut lid: Vec<P> = Vec::with_capacity(9);
    for i in std::iter::once(m.outer).chain(m.lower).chain(std::iter::once(m.inner)) {
        let p = at(i)?;
        let (a, b) = eye.local(p.0, p.1);
        // a lid point above the corners' line is a squint or a bad fit: the lid is never above it
        lid.push((a, b.max(0.0)));
    }
    lid.sort_by(|p, q| p.0.total_cmp(&q.0));
    eye.lid = lid;
    let c = at(m.iris);
    let rims: Option<Vec<P>> = m.rim.iter().map(|&i| at(i)).collect();
    if let (Some(c), Some(rims)) = (c, rims) {
        let r = rims.iter().map(|p| len(sub(*p, c))).sum::<f64>() / rims.len() as f64;
        if r.is_finite() && r > 0.15 * half && r < 1.2 * half {
            eye.iris = Some((c, r));
        }
    }
    Some(eye)
}

fn from_points(centre: P, other: P, half: f64, mouth: Option<P>) -> Option<Eye> {
    let d = sub(other, centre);
    let l = len(d);
    if !(l.is_finite() && l > 0.0) {
        return None;
    }
    let across = (d.0 / l, d.1 / l);
    let down = down_of(across, centre, mouth);
    let lid = (0..=8).map(|i| -1.0 + i as f64 / 4.0).map(|a: f64| (a, LID_DROP * (1.0 - a * a).max(0.0))).collect();
    Some(Eye { centre, across, down, half, lid, iris: None })
}
