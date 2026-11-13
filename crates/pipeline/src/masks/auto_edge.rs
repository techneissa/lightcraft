//! Auto Mask brush (issue #280): which pixels a dab paints.
//!
//! Pixels are compared by their 3 × 3 median colour ([`median3`]), which keeps edges and drops
//! noise. Each dab samples the colour under its centre, moved to the flattest spot near it
//! ([`dab_sample`]) so a centre on the blur of an edge samples the surface beside it rather than
//! the mix of both. A dab's reference is the range of the stroke's recent samples that agree with
//! their medoid ([`AutoRefs`]): a centre that slips over an edge for a moment still paints the
//! surface the stroke is on, and a textured surface paints all its shades. A pixel takes a dab's
//! paint by its distance to that range ([`AutoRef::weight`]). The committed stroke and the live
//! one while painting share all of it.

use std::collections::VecDeque;

use lightcraft_raster::{Plane, Rgb32f};

use super::{AUTO_TOL_CHROMA, AUTO_TOL_EV, chromaticity, smooth};

/// A pixel's colour as Auto Mask compares it: log2 luminance, then chromaticity `rgb / Y`.
pub type AutoColor = [f32; 4];

/// The [`AutoColor`] of a pixel of log luminance `l` and linear colour `c`.
#[inline]
pub fn auto_color(l: f32, c: [f32; 3]) -> AutoColor {
    let ch = chromaticity(c);
    [l, ch[0], ch[1], ch[2]]
}

/// The length of a colour difference `d` (per channel), in tolerances: 1 at [`AUTO_TOL_EV`] or
/// [`AUTO_TOL_CHROMA`]. NaN counts as far.
#[inline]
fn tolerances(d: AutoColor) -> f32 {
    let dl = d[0] / AUTO_TOL_EV;
    let dc = (d[1] * d[1] + d[2] * d[2] + d[3] * d[3]).sqrt() / AUTO_TOL_CHROMA;
    let v = (dl * dl + dc * dc).sqrt();
    if v.is_nan() { f32::MAX } else { v }
}

/// How far apart two colours are, in tolerances (0 the same).
#[inline]
pub fn auto_distance(a: AutoColor, b: AutoColor) -> f32 {
    tolerances(std::array::from_fn(|k| a[k] - b[k]))
}

/// A dab's reference: the colours from `lo` to `hi`, channel by channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoRef {
    pub lo: AutoColor,
    pub hi: AutoColor,
}

impl AutoRef {
    /// How much a pixel of colour `c` takes the dab's paint: 1 up to half a tolerance from the
    /// range, none from a whole one.
    #[inline]
    pub fn weight(&self, c: AutoColor) -> f32 {
        let d = std::array::from_fn(|k| (self.lo[k] - c[k]).max(c[k] - self.hi[k]).max(0.0));
        1.0 - smooth(0.5, 1.0, tolerances(d))
    }
}

/// The per-channel median of the 3 × 3 colours `at(dx, dy)` around a pixel, `dx, dy` in -1..=1.
#[inline]
pub fn median3(at: impl Fn(isize, isize) -> AutoColor) -> AutoColor {
    let mut v = [[0.0f32; 4]; 9];
    for (k, c) in v.iter_mut().enumerate() {
        *c = at(k as isize % 3 - 1, k as isize / 3 - 1);
    }
    std::array::from_fn(|ch| {
        let mut s: [f32; 9] = std::array::from_fn(|k| v[k][ch]);
        s.sort_unstable_by(f32::total_cmp);
        s[4]
    })
}

/// How far (pixels) a dab of radius `r` px looks for a flat spot to sample: a third of the radius.
#[inline]
pub fn search_reach(r: f64) -> isize {
    if r.is_finite() { (r / 3.0).round().clamp(0.0, 4096.0) as isize } else { 0 }
}

/// How far past its box (pixels) a stroke of radius `r` px reads median colours for its dabs'
/// samples: their reach and one grid step more.
pub fn search_margin(r: f64) -> usize {
    let reach = search_reach(r);
    (reach + (reach + SEARCH_STEPS - 1) / SEARCH_STEPS + 1) as usize
}

/// Positions a dab's search tries across its reach, each way from the centre.
const SEARCH_STEPS: isize = 6;
/// What moving the sample the whole reach costs, in tolerances of local contrast.
const SEARCH_MOVE: f32 = 0.6;
/// What taking a sample of another colour than the centre's costs, per tolerance.
const SEARCH_CHANGE: f32 = 0.5;

/// The colour a dab centred on pixel `(x, y)` samples: of the median colours `med(x, y)` within
/// `reach` px, the one with the least contrast around it, held near the centre and near the
/// centre's colour. On a flat surface that is the centre's colour; on the blur of an edge, the
/// surface beside it on the centre's side.
pub fn dab_sample(med: impl Fn(isize, isize) -> AutoColor, x: isize, y: isize, reach: isize) -> AutoColor {
    let centre = med(x, y);
    if reach <= 0 {
        return centre;
    }
    let step = (reach + SEARCH_STEPS - 1) / SEARCH_STEPS;
    let n = reach / step;
    // the median colours on a grid `step` apart, one past the reach each way
    let side = 2 * n + 3;
    let grid: Vec<AutoColor> = (0..side * side).map(|k| med(x + (k % side - n - 1) * step, y + (k / side - n - 1) * step)).collect();
    let at = |i: isize, j: isize| grid.get(((j + n + 1) * side + i + n + 1) as usize).copied().unwrap_or(centre);
    let mut best = (f32::MAX, centre);
    for j in -n..=n {
        for i in -n..=n {
            let d2 = i * i + j * j;
            if d2 > n * n {
                continue;
            }
            let c = at(i, j);
            let contrast = 0.5 * (auto_distance(at(i + 1, j), at(i - 1, j)) + auto_distance(at(i, j + 1), at(i, j - 1)));
            let score = contrast + SEARCH_MOVE * (d2 as f32).sqrt() / n.max(1) as f32 + SEARCH_CHANGE * auto_distance(c, centre);
            if score < best.0 {
                best = (score, c);
            }
        }
    }
    best.1
}

/// Dabs a stroke's reference looks back over: about two brush radii of travel (dabs are a quarter
/// radius apart).
pub const AUTO_RECENT: usize = 9;

/// How far (tolerances) a recent sample may be from their medoid and still count.
const AUTO_AGREE: f32 = 2.0;

/// The references of a stroke's dabs, one after another. A dab's reference is the range of the
/// last [`AUTO_RECENT`] samples within [`AUTO_AGREE`] of their medoid (the sample closest to the
/// others): a sample from over an edge is left out until the stroke has stayed there for over half
/// the window, and then it is the old surface's samples that drop out.
#[derive(Clone, Default)]
pub struct AutoRefs {
    recent: VecDeque<AutoColor>,
}

impl AutoRefs {
    /// The reference of the next dab, whose sample ([`dab_sample`]) is `sample`.
    pub fn next(&mut self, sample: AutoColor) -> AutoRef {
        if self.recent.len() >= AUTO_RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(sample);
        let mut medoid = (f32::MAX, sample);
        // (ties go to the latest)
        for a in self.recent.iter().rev() {
            let cost = self.recent.iter().map(|b| auto_distance(*a, *b).min(2.0 * AUTO_AGREE)).sum::<f32>();
            if cost < medoid.0 {
                medoid = (cost, *a);
            }
        }
        let mut r = AutoRef { lo: medoid.1, hi: medoid.1 };
        for s in self.recent.iter().filter(|s| auto_distance(**s, medoid.1) < AUTO_AGREE) {
            for k in 0..4 {
                r.lo[k] = r.lo[k].min(s[k]);
                r.hi[k] = r.hi[k].max(s[k]);
            }
        }
        r
    }
}

/// The median colours ([`median3`]) of the `x0..x1 × y0..y1` box of the photo, row by row (the
/// frame's edge pixels repeat past it). Empty when `img` and `log_l` differ in size.
pub fn median_box(img: &Rgb32f, log_l: &Plane, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> Vec<AutoColor> {
    let (w, h) = (log_l.width, log_l.height);
    if w == 0 || h == 0 || img.width != w || img.height != h || x0 >= x1 || y0 >= y1 {
        return Vec::new();
    }
    let (x1, y1) = (x1.min(w), y1.min(h));
    let (bw, bh) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    // the colours with a 1 px margin
    let (mx0, my0) = (x0.saturating_sub(1), y0.saturating_sub(1));
    let (mx1, my1) = ((x1 + 1).min(w), (y1 + 1).min(h));
    let mw = mx1 - mx0;
    let mut raw = vec![[0.0f32; 4]; mw * (my1 - my0)];
    crate::for_rows(&mut raw, mw, |ry, row| {
        let y = my0 + ry;
        for (c, x) in row.iter_mut().zip(mx0..) {
            let i = y * w + x;
            *c = auto_color(log_l.data.get(i).copied().unwrap_or(0.0), img.data.get(i).copied().unwrap_or([0.0; 3]));
        }
    });
    let at = |x: isize, y: isize| {
        let x = x.clamp(mx0 as isize, mx1 as isize - 1) as usize - mx0;
        let y = y.clamp(my0 as isize, my1 as isize - 1) as usize - my0;
        raw.get(y * mw + x).copied().unwrap_or([0.0; 4])
    };
    let mut out = vec![[0.0f32; 4]; bw * bh];
    crate::for_rows(&mut out, bw, |ry, row| {
        let y = (y0 + ry) as isize;
        for (c, x) in row.iter_mut().zip(x0 as isize..) {
            *c = median3(|dx, dy| at(x + dx, y + dy));
        }
    });
    out
}
