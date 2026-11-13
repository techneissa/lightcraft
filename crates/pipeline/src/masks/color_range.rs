//! Colour Range masks: pixels whose colour is near one of the sampled colours.
//!
//! Issue #279: one click on a colour selects that colour's surface in light and shadow. A pixel is compared with a
//! sample on two things:
//!
//! - its **chroma relative to its lightness**: OkLab `(a, b) / L`. Shading a surface scales its linear RGB, and
//!   OkLab's cube root scales `L`, `a` and `b` alike, so this stays put from the lit to the shaded side. It is taken
//!   from the exposed scene colour (before the selection tone map, which would desaturate the lit side). Hue (across
//!   the sample's direction) counts more than chroma (along it), and both are relative to the sample's own chroma: a
//!   vivid red tolerates a wider absolute spread than a muted teal, the way the eye sees it;
//! - its **lightness** (through [`tonemap_for_select`], as on screen): a vivid sample lets it go far (a surface in
//!   shade), a near-neutral one keeps it tight, since lightness is the only thing that tells greys apart (issue #157).
//!
//! The three distances are scaled so that 1 is where Refine 50 lets a pixel go ([`color_range_tolerance`]), and the
//! alpha is then smoothed along the photo's edges ([`SMOOTH_EPS`]), so noise does not speckle the selection.
//! The GPU (`mask.wgsl` kind 3) takes the same [`Key`]s and tolerance and runs the same smoothing.

use lightcraft_color::perceptual::{oklab_from_2020, oklab_to_2020};
use lightcraft_raster::{Plane, Rgb32f};

use super::smooth;
use crate::for_rows;
use crate::geometry::Frame;

/// OkLab lightness a colour's chroma is measured against at least (`hypot(L, L_FLOOR)`), so the noise of the deepest
/// shadows does not swing it about.
const L_FLOOR: f32 = 0.1;
/// Chroma-ratio distance at which Refine 50 lets a pixel go from a neutral sample.
const CHROMA_FLOOR: f32 = 0.03;
/// How the hue and the chroma tolerances grow with the sample's chroma ratio (across, along its direction).
const HUE_SCALE: f32 = 0.3;
const CHROMA_SCALE: f32 = 0.4;
/// Lightness distance (tone-mapped OkLab L) at which Refine 50 lets a pixel go: from a neutral and from a colourful
/// sample; colourful from a chroma ratio of [`COLOURFUL`]`.1` on, neutral up to `.0`.
const L_NEUTRAL: f32 = 0.06;
const L_COLOUR: f32 = 0.45;
const COLOURFUL: (f32, f32) = (0.04, 0.10);
/// The tolerance at Refine 0 ("this exact shade") and 100 ("this colour family"); it is 1 at Refine 50.
const TOL_MIN: f32 = 0.3;
const TOL_MAX: f32 = 1.8;
/// Edge-aware smoothing: guided-filter window (fraction of the output's long edge, at least a pixel) and its
/// regularization (EV², as Refine Edges).
const SMOOTH_WINDOW: f32 = 0.002;
pub const SMOOTH_EPS: f32 = 0.02;
/// Neighbourhood a click samples: its radius, as a fraction of the sampled render's long edge.
const PICK_RADIUS: f32 = 0.006;
/// How much of a dragged box the colours it picks must hold within Refine 50's full selection (the rest is taken
/// for noise or stray pixels).
const BOX_COVER: f32 = 0.9;

/// One sample made ready for the per-pixel test: chroma ratio `a / L`, `b / L`, lightness, the chroma direction
/// (unit x, y; `1, 0` for a neutral), and the reciprocal tolerances along it, across it and in lightness. The GPU
/// uploads these eight values per sample.
pub type Key = [f32; 8];

/// The keys of `samples` (OkLab through [`tonemap_for_select`], as stored), at most
/// [`MAX_COLOR_RANGE_SAMPLES`](lightcraft_develop::MAX_COLOR_RANGE_SAMPLES) (a settings value built in code can skip the
/// deserializer's cap: cost is pixels x samples). A sample that is not a finite colour is left out.
pub fn keys(samples: &[[f64; 3]]) -> Vec<Key> {
    samples.iter().take(lightcraft_develop::MAX_COLOR_RANGE_SAMPLES).filter_map(|s| key(s.map(|v| v as f32))).collect()
}

fn key(s: [f32; 3]) -> Option<Key> {
    if !s.iter().all(|v| v.is_finite()) {
        return None;
    }
    let [ra, rb, l] = sample_point(s);
    let c = ra.hypot(rb);
    let (ux, uy) = if c > 1e-6 { (ra / c, rb / c) } else { (1.0, 0.0) };
    let colourful = smooth(COLOURFUL.0, COLOURFUL.1, c);
    let sl = L_NEUTRAL + (L_COLOUR - L_NEUTRAL) * colourful;
    Some([ra, rb, l, ux, uy, 1.0 / (CHROMA_FLOOR + CHROMA_SCALE * c), 1.0 / (CHROMA_FLOOR + HUE_SCALE * c), 1.0 / sl])
}

/// A stored sample (tone-mapped OkLab) as the point [`distance`] compares: its scene colour's chroma ratio (the tone
/// map undone) and its own lightness.
fn sample_point(s: [f32; 3]) -> [f32; 3] {
    // (the tone map reaches 1 only at infinity; a colour past the gamut can have a channel a little below 0)
    let scene = oklab_to_2020(s).map(|t| {
        let t = t.clamp(-0.5, 0.995);
        t / (1.0 - t)
    });
    let [ra, rb] = chroma_ratio(oklab_from_2020(scene));
    [ra, rb, s[0]]
}

#[inline]
fn chroma_ratio(lab: [f32; 3]) -> [f32; 2] {
    let n = lab[0].hypot(L_FLOOR);
    [lab[1] / n, lab[2] / n]
}

/// The point [`distance`] compares for an exposed linear Rec.2020 colour `c`.
#[inline]
pub fn pixel(c: [f32; 3]) -> [f32; 3] {
    let [ra, rb] = chroma_ratio(oklab_from_2020(c));
    [ra, rb, oklab_from_2020(tonemap_for_select(c))[0]]
}

/// How far pixel point `p` ([`pixel`]) is from sample `k`, 1 being where Refine 50 lets it go.
#[inline]
pub fn distance(k: &Key, p: [f32; 3]) -> f32 {
    let (dx, dy, dl) = (p[0] - k[0], p[1] - k[1], p[2] - k[2]);
    let along = (dx * k[3] + dy * k[4]) * k[5];
    let across = (dy * k[3] - dx * k[4]) * k[6];
    let dl = dl * k[7];
    (along * along + across * across + dl * dl).sqrt()
}

/// The alpha of a pixel `d` ([`distance`]) from its nearest sample, at tolerance `tol` ([`color_range_tolerance`]):
/// fully selected up to half of it, fading out over the other half.
#[inline]
pub fn alpha(d: f32, tol: f32) -> f32 {
    1.0 - smooth(tol * 0.5, tol, d)
}

/// The alpha of a colour range with `samples` (OkLab, see [`tonemap_for_select`]) and Refine `refine`, over `img`
/// (before exposure: `gain` is the exposure's linear factor), smoothed along the edges of `log_l` (skipped when it
/// is not the size of `out`).
pub(super) fn fill(out: &mut Plane, img: &Rgb32f, log_l: &Plane, samples: &[[f64; 3]], refine: f64, gain: f32, frame: &Frame) {
    let (w, h) = (out.width, out.height);
    let tol = color_range_tolerance(refine);
    let keys = keys(samples);
    if keys.is_empty() || img.data.len() != out.data.len() {
        return;
    }
    for_rows(&mut out.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let Some(c) = img.data.get(y * w + x) else { continue };
            let p = pixel(c.map(|v| v * gain));
            let d = keys.iter().map(|k| distance(k, p)).fold(f32::MAX, f32::min);
            *v = alpha(d, tol);
        }
    });
    if (log_l.width, log_l.height) == (w, h) && w > 0 && h > 0 {
        *out = super::guided_cross(log_l, out, smooth_sigma(frame, w, h), SMOOTH_EPS);
    }
}

/// The edge-aware smoothing window of a colour range (px of a `w × h` output, the same share of the photo at every
/// size so previews and exports match).
pub fn smooth_sigma(frame: &Frame, w: usize, h: usize) -> f32 {
    (SMOOTH_WINDOW * frame.output_long(w, h) as f32).max(1.0)
}

/// A rough display mapping for colour picking (so samples taken on screen match).
pub(crate) fn tonemap_for_select(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| v / (1.0 + v))
}

/// Colour range: the [`distance`] at which a pixel leaves the selection for a Refine amount of 0..100. A pixel is
/// fully selected up to half of it and fades out over the other half (CPU here, `mask.wgsl` kind 3; both take this
/// value). Refine 0 keeps the sample's own shade (0.3), 50 its surface in light and shade (1), 100 its colour family
/// (1.8); exponential in between.
///
/// Issue #157: a slightly teal mid-grey (sRGB 120,150,155) and a neutral light grey (200,198,195) are close in
/// colour; a sample of the first must not take the second. The teal's chroma ratio is low, so lightness keeps its
/// tight neutral tolerance and the grey (0.11 lighter) stays out up to Refine 85, mostly out at 100.
pub fn color_range_tolerance(refine: f64) -> f32 {
    let r = if refine.is_finite() { (refine / 100.0).clamp(0.0, 1.0) as f32 } else { 0.5 };
    if r <= 0.5 { TOL_MIN * (1.0 / TOL_MIN).powf(r / 0.5) } else { TOL_MAX.powf((r - 0.5) / 0.5) }
}

/// The radius in px of the neighbourhood a click samples on a `w × h` render.
pub fn pick_radius(w: usize, h: usize) -> usize {
    (PICK_RADIUS * w.max(h) as f32).round().max(1.0) as usize
}

/// The per-channel median of `labs` (one colour robust to noise and to a minority of other colours), `None` when
/// empty.
pub fn median(labs: &[[f32; 3]]) -> Option<[f64; 3]> {
    if labs.is_empty() {
        return None;
    }
    let mut ch: Vec<f32> = Vec::with_capacity(labs.len());
    let mut out = [0f64; 3];
    for (k, o) in out.iter_mut().enumerate() {
        ch.clear();
        ch.extend(labs.iter().map(|l| l[k]).filter(|v| v.is_finite()));
        if ch.is_empty() {
            return None;
        }
        let mid = ch.len() / 2;
        let (_, m, _) = ch.select_nth_unstable_by(mid, f32::total_cmp);
        *o = f64::from(*m);
    }
    Some(out)
}

/// At most `max` colours that together cover a dragged box's colours `labs` (selection-space OkLab): its median,
/// then, while more than a tenth of the box is beyond Refine 50's full selection from all of them, the median of the
/// pixels around the furthest of the rest.
pub fn representatives(labs: &[[f32; 3]], max: usize) -> Vec<[f64; 3]> {
    let mut reps: Vec<[f64; 3]> = Vec::new();
    let Some(first) = median(labs).filter(|_| max > 0) else {
        return reps;
    };
    let points: Vec<[f32; 3]> = labs.iter().map(|&l| sample_point(l)).collect();
    let mut keys: Vec<Key> = Vec::new();
    let mut next = Some(first);
    while let Some(rep) = next.take() {
        let Some(k) = key(rep.map(|v| v as f32)) else { break };
        reps.push(rep);
        keys.push(k);
        if reps.len() >= max {
            break;
        }
        let far: Vec<(f32, usize)> = points
            .iter()
            .enumerate()
            .map(|(i, p)| (keys.iter().map(|k| distance(k, *p)).fold(f32::MAX, f32::min), i))
            .filter(|(d, _)| d.is_finite() && *d > 0.5)
            .collect();
        if (far.len() as f32) <= (1.0 - BOX_COVER) * points.len() as f32 {
            break;
        }
        // (the furthest pixel once the stray tenth is set aside, and the pixels of its own colour)
        let mut ds: Vec<f32> = far.iter().map(|(d, _)| *d).collect();
        let at = ((ds.len() as f32 * BOX_COVER) as usize).min(ds.len() - 1);
        let (_, cut, _) = ds.select_nth_unstable_by(at, f32::total_cmp);
        let cut = *cut;
        let Some(&(_, i)) = far.iter().find(|(d, _)| *d == cut) else { break };
        let Some(seed) = labs.get(i).and_then(|&l| key(l)) else { break };
        let own: Vec<[f32; 3]> = far.iter().filter(|(_, j)| distance(&seed, points[*j]) <= 0.5).map(|(_, j)| labs[*j]).collect();
        next = median(&own);
    }
    reps
}
