//! Auto Mask brush (issue #280): which pixels a dab paints.
//!
//! Pixels are compared by their median colour ([`median_at`]), which keeps edges and drops noise:
//! 3 × 3 at a fit preview's scale, wider in proportion in a larger render ([`median_radius`]), so
//! an export's extra pixel noise is dropped as the preview's downscaling drops it. Each dab samples the colour under its centre, moved to the flattest spot near it
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
    std::array::from_fn(|ch| median9(std::array::from_fn(|k| v[k][ch])))
}

/// The median of nine values: a sorting network of 19 exchanges that leaves the middle one in place
/// (Paeth's, as in Devillard's "Fast median search").
#[inline]
fn median9(mut p: [f32; 9]) -> f32 {
    const PAIRS: [(usize, usize); 19] = [
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 1),
        (3, 4),
        (6, 7),
        (1, 2),
        (4, 5),
        (7, 8),
        (0, 3),
        (5, 8),
        (4, 7),
        (3, 6),
        (1, 4),
        (2, 5),
        (4, 7),
        (4, 2),
        (6, 4),
        (4, 2),
    ];
    for (a, b) in PAIRS {
        let (lo, hi) = (p[a].min(p[b]), p[a].max(p[b]));
        p[a] = lo;
        p[b] = hi;
    }
    p[4]
}

/// Render pixels per photo long edge at which pixels are compared by their 3 × 3 median (about a
/// fit preview's).
pub const AUTO_MEDIAN_LONG: f64 = 1600.0;

/// The widest median radius (pixels): 17 × 17, from about five times [`AUTO_MEDIAN_LONG`].
const MAX_MEDIAN_RADIUS: usize = 8;

/// The radius (pixels) of the median pixels are compared by in a render of `px_per_long` pixels
/// per photo long edge: 1 (3 × 3) up to [`AUTO_MEDIAN_LONG`], spanning as much of the photo as
/// that 3 × 3 does in a larger render.
pub fn median_radius(px_per_long: f64) -> usize {
    let s = px_per_long / AUTO_MEDIAN_LONG;
    if s.is_finite() { ((3.0 * s - 1.0) / 2.0).round().clamp(1.0, MAX_MEDIAN_RADIUS as f64) as usize } else { 1 }
}

/// The median of `v` (NaN sorts last); 0 when empty.
#[inline]
fn median_of(v: &mut [f32]) -> f32 {
    let m = v.len() / 2;
    if m < v.len() {
        v.select_nth_unstable_by(m, f32::total_cmp);
    }
    v.get(m).copied().unwrap_or(0.0)
}

/// The per-channel median of the `2 rho + 1` colours `at(d)`, `d` in `-rho..=rho`.
#[inline]
fn median_line(at: impl Fn(isize) -> AutoColor, rho: usize) -> AutoColor {
    let rho = rho.min(MAX_MEDIAN_RADIUS) as isize;
    let mut v = [[0.0f32; 4]; 2 * MAX_MEDIAN_RADIUS + 1];
    let n = (2 * rho + 1) as usize;
    for (k, c) in v.iter_mut().take(n).enumerate() {
        *c = at(k as isize - rho);
    }
    std::array::from_fn(|ch| {
        let mut x = [0.0f32; 2 * MAX_MEDIAN_RADIUS + 1];
        for (o, c) in x.iter_mut().zip(&v).take(n) {
            *o = c[ch];
        }
        median_of(x.get_mut(..n).unwrap_or_default())
    })
}

/// The median colour Auto Mask compares a pixel by, of radius `rho` ([`median_radius`]) over the
/// colours `at(dx, dy)` around it: the 3 × 3 median ([`median3`]) at radius 1, past it the median
/// of each column of the row medians (a separable median: it keeps straight edges as the square
/// one does, at a fraction of its cost).
pub fn median_at(at: impl Fn(isize, isize) -> AutoColor, rho: usize) -> AutoColor {
    if rho <= 1 {
        return median3(at);
    }
    median_line(|dy| median_line(|dx| at(dx, dy), rho), rho)
}

/// How far (pixels) a dab of radius `r` px looks for a flat spot to sample: a third of the radius.
#[inline]
pub fn search_reach(r: f64) -> isize {
    if r.is_finite() { (r / 3.0).round().clamp(0.0, 4096.0) as isize } else { 0 }
}

/// How far past its box (pixels) a stroke of radius `r` px reads colours for its dabs' samples:
/// their reach, one grid step and the widest median more.
pub fn search_margin(r: f64) -> usize {
    let reach = search_reach(r);
    (reach + (reach + SEARCH_STEPS - 1) / SEARCH_STEPS + 1) as usize + MAX_MEDIAN_RADIUS
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

/// The colours of a box of the photo an Auto Mask stroke reads: each pixel's, and the median
/// ([`median_at`]) of each pixel in the rows' spans it is made with (the pixels the stroke's dabs
/// cover); any other pixel's median is worked out when asked for. Past the box, its edge pixels
/// repeat.
pub struct MedianBox {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    rho: usize,
    raw: Vec<AutoColor>,
    /// The row medians a median past radius 1 is made of, and the medians (NaN luminance: not
    /// worked out).
    rows: Vec<AutoColor>,
    med: Vec<AutoColor>,
}

impl MedianBox {
    /// No pixels.
    pub fn empty() -> MedianBox {
        MedianBox { x0: 0, y0: 0, w: 0, h: 0, rho: 1, raw: Vec::new(), rows: Vec::new(), med: Vec::new() }
    }

    /// The colours of the `x0..x1 × y0..y1` box of the photo (clamped to it), with medians of
    /// radius `rho` in the span `span(y)` (`x` from, to: exclusive) of each row `y`. Empty when
    /// `img` and `log_l` differ in size.
    pub fn new(
        img: &Rgb32f,
        log_l: &Plane,
        (x0, y0, x1, y1): (usize, usize, usize, usize),
        rho: usize,
        span: impl Fn(usize) -> Option<(usize, usize)> + Sync,
    ) -> MedianBox {
        let (fw, fh) = (log_l.width, log_l.height);
        let (x1, y1) = (x1.min(fw), y1.min(fh));
        if img.width != fw || img.height != fh || x0 >= x1 || y0 >= y1 {
            return MedianBox::empty();
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let rho = rho.clamp(1, MAX_MEDIAN_RADIUS);
        let mut raw = vec![[0.0f32; 4]; w * h];
        crate::for_rows(&mut raw, w, |ry, row| {
            let y = y0 + ry;
            for (c, x) in row.iter_mut().zip(x0..) {
                let i = y * fw + x;
                *c = auto_color(log_l.data.get(i).copied().unwrap_or(0.0), img.data.get(i).copied().unwrap_or([0.0; 3]));
            }
        });
        let mut b = MedianBox { x0, y0, w, h, rho, raw, rows: Vec::new(), med: Vec::new() };
        let fill =
            |out: &mut Vec<AutoColor>, need: &(dyn Fn(usize) -> Option<(usize, usize)> + Sync), f: &(dyn Fn(isize, isize) -> AutoColor + Sync)| {
                crate::for_rows(out, w, |ry, row| {
                    let y = y0 + ry;
                    let Some((a, z)) = need(y) else { return };
                    let (a, z) = (a.clamp(x0, x1), z.clamp(x0, x1));
                    for (c, x) in row.iter_mut().skip(a - x0).take(z.saturating_sub(a)).zip(a..) {
                        *c = f(x as isize, y as isize);
                    }
                });
            };
        if rho > 1 {
            // the row medians of the rows within `rho` of a span, across those spans
            let need = |y: usize| {
                let (lo, hi) = (y.saturating_sub(rho), y + rho);
                (lo..=hi).filter_map(&span).reduce(|p, q| (p.0.min(q.0), p.1.max(q.1)))
            };
            let mut rows = vec![[f32::NAN; 4]; w * h];
            fill(&mut rows, &need, &|x, y| b.row_median(x, y));
            b.rows = rows;
        }
        let mut med = vec![[f32::NAN; 4]; w * h];
        fill(&mut med, &span, &|x, y| b.median(x, y));
        b.med = med;
        b
    }

    /// The index of pixel `(x, y)` (frame pixels) clamped to the box.
    #[inline]
    fn index(&self, x: isize, y: isize) -> usize {
        let cx = (x - self.x0 as isize).clamp(0, self.w.saturating_sub(1) as isize) as usize;
        let cy = (y - self.y0 as isize).clamp(0, self.h.saturating_sub(1) as isize) as usize;
        cy * self.w + cx
    }

    #[inline]
    fn raw_at(&self, x: isize, y: isize) -> AutoColor {
        self.raw.get(self.index(x, y)).copied().unwrap_or([0.0; 4])
    }

    /// The median of the row through `(x, y)`, `rho` either side.
    #[inline]
    fn row_median(&self, x: isize, y: isize) -> AutoColor {
        median_line(|dx| self.raw_at(x + dx, y), self.rho)
    }

    /// The median of pixel `(x, y)`, worked out from the colours (and the row medians made).
    fn median(&self, x: isize, y: isize) -> AutoColor {
        if self.rho <= 1 {
            return median3(|dx, dy| self.raw_at(x + dx, y + dy));
        }
        median_line(
            |dy| match self.rows.get(self.index(x, y + dy)) {
                Some(c) if !c[0].is_nan() => *c,
                _ => self.row_median(x, y + dy),
            },
            self.rho,
        )
    }

    /// The median colour of pixel `(x, y)` (frame pixels).
    #[inline]
    pub fn at(&self, x: isize, y: isize) -> AutoColor {
        match self.med.get(self.index(x, y)) {
            Some(c) if !c[0].is_nan() => *c,
            _ => self.median(x, y),
        }
    }
}

#[cfg(test)]
#[test]
fn the_network_finds_the_median() {
    // every order of a few sets of nine values
    for k in 0..2000u32 {
        let v: [f32; 9] = std::array::from_fn(|i| ((k.wrapping_mul(2_654_435_761).rotate_left(i as u32 * 7) >> 20) % 13) as f32);
        let mut s = v;
        s.sort_unstable_by(f32::total_cmp);
        assert_eq!(median9(v), s[4], "{v:?}");
    }
}
