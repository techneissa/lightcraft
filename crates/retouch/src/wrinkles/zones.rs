//! Wrinkles v2, what kind of line each part of a face crop holds: the zone of each pixel (the region of the four areas
//! that weighs most there), with the widths its lines take and the direction they usually run in.
//!
//! - **Forehead**: long lines across it, and the frown lines up from the brows in its middle, 0.12–0.9 % of the face
//!   wide (a line's Gaussian sigma).
//! - **Between the brows**: frown lines down, the line over the nose across: any direction, up to 1.35 %.
//! - **Crow's feet**: the fan beyond each eye's outer corner, lines running out from the corner and round it, up to 0.6 %.
//! - **Under the eyes**: fine lines in any direction, up to 0.4 % (Eye Bags works on the broader shading there).
//! - **The fold from the nose to the mouth**: along itself, 0.4–2 %; softened, never taken away ([`Zone::keep`]).
//! - **Cheeks and chin**: any direction, up to 0.6 %.
//! - **Neck**: rings across the neck, 0.18–1.35 %.

use lightcraft_raster::Plane;

use super::area_weight;
use super::areas::{self, Area, P, Region, Shape};
use crate::FaceCtx;

/// The widths a line is looked for at (Gaussian sigma), fractions of the face, finest first.
pub(super) const LADDER: [f64; 8] = [0.0012, 0.0018, 0.0027, 0.004, 0.006, 0.009, 0.0135, 0.02];

/// The direction a zone's lines run in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Course {
    /// Any direction.
    Any,
    /// Along this unit direction (or its opposite).
    Along(P),
    /// The forehead: across the face (the first direction), and also up and down within a band of half-width `half`
    /// pixels round the middle line through `mid` (the frown lines between the brows go up into the forehead).
    Forehead { across: P, mid: P, half: f64 },
}

/// A zone: which of the four sliders it answers to, the widths of [`LADDER`] its lines take (first and last index), the
/// direction they run in, and how much of a line goes at 100 (1: all of it; less keeps a softened fold).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Zone {
    pub area: Area,
    pub widths: (usize, usize),
    pub course: Course,
    pub keep: f32,
    /// The shortest line, a fraction of the face.
    pub min_len: f64,
}

/// A line's direction within this many degrees of its zone's course counts fully, past [`OFF_COURSE`] not at all.
const ON_COURSE: f64 = 25.0;
const OFF_COURSE: f64 = 50.0;

impl Zone {
    /// How well a line through `p` running along the unit direction `t` fits the zone (0…1).
    pub fn fits(&self, p: P, t: P) -> f32 {
        let along = match self.course {
            Course::Any => return 1.0,
            Course::Along(d) => d,
            Course::Forehead { across, mid, half } => {
                let fit = Zone { course: Course::Along(across), ..*self }.fits(p, t);
                let off = ((p.0 - mid.0) * across.0 + (p.1 - mid.1) * across.1).abs();
                if off > half {
                    return fit;
                }
                return fit.max(Zone { course: Course::Along((-across.1, across.0)), ..*self }.fits(p, t));
            }
        };
        let cos = (t.0 * along.0 + t.1 * along.1).abs();
        let (lo, hi) = (OFF_COURSE.to_radians().cos(), ON_COURSE.to_radians().cos());
        let k = ((cos - lo) / (hi - lo)).clamp(0.0, 1.0);
        (k * k * (3.0 - 2.0 * k)) as f32
    }
}

/// The zones of a crop: the zone of each pixel (an index into `list`, [`NONE`] outside every area) and the weight of the
/// areas there (the largest region's, 0…1).
pub(super) struct Zones {
    pub list: Vec<Zone>,
    pub index: Vec<u8>,
    pub weight: Plane,
}

/// No zone.
pub(super) const NONE: u8 = u8::MAX;

impl Zones {
    /// The zone of pixel (`x`, `y`), if any.
    pub fn at(&self, x: usize, y: usize) -> Option<&Zone> {
        let i = *self.index.get(y.checked_mul(self.weight.width)?.checked_add(x)?)?;
        self.list.get(usize::from(i))
    }
}

/// The face's "across" direction (unit, from its right eye's outer corner to its left one's) and the point between its
/// brows' inner ends.
fn axes(ctx: &FaceCtx<'_>) -> Option<(P, P)> {
    let pt = |i: usize| ctx.landmarks.get(areas::FIRST + i).copied().flatten();
    let (r, l) = (pt(33)?, pt(263)?);
    let (dx, dy) = (l.0 - r.0, l.1 - r.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (a, b) = (pt(107)?, pt(336)?);
    (len.is_finite() && len > 1e-9).then(|| ((dx / len, dy / len), ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)))
}

/// The zone a region of `area` stands for (the regions come in [`areas::regions`]' order and shapes).
fn zone_of(area: Area, region: &Region, across: P, mid: P, size: f64) -> Zone {
    let zone = |widths, course, keep, min_len| Zone { area, widths, course, keep, min_len };
    match (area, &region.shape) {
        (Area::Forehead, Shape::Capsule { .. }) => zone((0, 6), Course::Any, 1.0, SHORT),
        (Area::Forehead, _) => zone((0, 5), Course::Forehead { across, mid, half: FROWN_BAND * size }, 1.0, FOREHEAD_LEN),
        (Area::CrowsFeet, _) => match region.ramp {
            // (they run out from the corner, and round it where the lid folds: any direction)
            Some(_) => zone((0, 4), Course::Any, 1.0, SHORT),
            None => zone((0, 3), Course::Any, 1.0, SHORT),
        },
        (Area::SmileLines, Shape::Capsule { p0, p1, .. }) => {
            let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
            let len = (dx * dx + dy * dy).sqrt();
            let course = if len.is_finite() && len > 1e-9 { Course::Along((dx / len, dy / len)) } else { Course::Any };
            zone((3, 7), course, FOLD_KEEP, SHORT)
        }
        (Area::SmileLines, _) => zone((0, 4), Course::Any, 1.0, SHORT),
        (Area::Neck, _) => zone((1, 6), Course::Along(across), 1.0, FOREHEAD_LEN),
    }
}

/// The shortest forehead (and neck) line and the shortest line elsewhere, fractions of the face: a forehead line runs a
/// good part of the way across it; a short stroke there is the skin's grain or a row of freckles.
const FOREHEAD_LEN: f64 = 0.03;
const SHORT: f64 = 0.025;
/// The band round the forehead's middle where its lines may also run up and down (half its width, a fraction of the
/// face): the frown lines.
const FROWN_BAND: f64 = 0.07;

/// How much of the fold from the nose to the mouth goes at 100: it is the face's shape, softened, never taken away.
const FOLD_KEEP: f32 = 0.6;

/// The zones of a `w × h` crop whose face is `ctx` (mapped into it); `corner` and `step` anchor the weights' lattice as
/// [`area_weight`] does. `None` without the face mesh.
pub(super) fn zones(ctx: &FaceCtx<'_>, w: usize, h: usize, corner: [f64; 2], step: f64) -> Option<Zones> {
    let (across, mid) = axes(ctx)?;
    let mut list = Vec::new();
    let mut index = vec![NONE; w.saturating_mul(h)];
    let mut weight = Plane::new(w, h);
    for area in Area::ALL {
        for region in areas::regions(ctx, area) {
            if list.len() >= usize::from(NONE) {
                break;
            }
            let r = region.bounds();
            let px = |v: f64, hi: usize| if v.is_finite() { (v.max(0.0) as usize).min(hi) } else { 0 };
            let b = [px(r[0].floor(), w), px(r[1].floor(), h), px(r[2].ceil() + 1.0, w), px(r[3].ceil() + 1.0, h)];
            if b[0] >= b[2] || b[1] >= b[3] {
                continue;
            }
            let k = list.len() as u8;
            list.push(zone_of(area, &region, across, mid, ctx.size));
            let a = area_weight(std::slice::from_ref(&region), b, corner, step);
            let bw = b[2] - b[0];
            for (row, arow) in a.data.chunks(bw.max(1)).enumerate() {
                let at = (b[1] + row) * w + b[0];
                let (Some(wrow), Some(irow)) = (weight.data.get_mut(at..at + bw), index.get_mut(at..at + bw)) else { continue };
                for ((wv, iv), v) in wrow.iter_mut().zip(irow.iter_mut()).zip(arow) {
                    if *v > *wv {
                        *wv = *v;
                        *iv = k;
                    }
                }
            }
        }
    }
    Some(Zones { list, index, weight })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_fits_its_zone_by_its_direction() {
        let across = Zone { area: Area::Forehead, widths: (0, 5), course: Course::Along((1.0, 0.0)), keep: 1.0, min_len: 0.04 };
        assert!(across.fits((0.0, 0.0), (1.0, 0.0)) > 0.99, "a line across the forehead");
        assert!(across.fits((0.0, 0.0), (-0.94, 0.34)) > 0.99, "20° off");
        assert!(across.fits((0.0, 0.0), (0.0, 1.0)) < 0.01, "a line down the forehead is not one of its lines");
        let any = Zone { area: Area::SmileLines, widths: (0, 4), course: Course::Any, keep: 1.0, min_len: 0.025 };
        assert_eq!(any.fits((3.0, 4.0), (0.6, 0.8)), 1.0);
        let forehead = Zone { course: Course::Forehead { across: (1.0, 0.0), mid: (100.0, 0.0), half: 10.0 }, ..across };
        assert!(forehead.fits((100.0, 50.0), (0.0, 1.0)) > 0.99, "a frown line in the middle");
        assert!(forehead.fits((150.0, 50.0), (0.0, 1.0)) < 0.01, "not out to the side");
        assert!(forehead.fits((150.0, 50.0), (1.0, 0.0)) > 0.99, "a line across anywhere");
    }
}
