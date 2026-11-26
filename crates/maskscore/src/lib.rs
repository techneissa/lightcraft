//! Mask scoring (fork, issue #256): how close a mask is to its human-drawn truth, and where the next click goes.
//!
//! Masks are coverage planes (0 to 1). Overlap, edge and gap metrics cut them at 0.5; SAD and MSE compare the soft
//! values. Pixels marked `ignore` (unlabelled in the truth) are left out of every number.
//!
//! - [`Scores::iou`]: intersection over union.
//! - [`Scores::bg_iou`]: the same for both masks turned around (the app's Background).
//! - [`Scores::boundary_f`]: F-score of the two outlines, a pixel counting when the other outline is within
//!   [`tolerance`] (0.2 % of the long side, at least 1 px).
//! - [`Scores::band_iou`]: IoU of the pixels within [`band_radius`] (1 % of the long side) of the truth's outline.
//! - [`Scores::gaps_kept`]: of the background holes enclosed by the truth, the share of their pixels the mask leaves
//!   out (`None`: the truth has no hole).
//! - [`Scores::sad`] and [`Scores::mse`]: over the unknown band ([`unknown_band`]): soft pixels of the truth and
//!   everything within [`band_radius`] of its outline. SAD is the sum of differences divided by 1000, MSE the mean
//!   squared difference.
//!
//! The click rule ([`next_click`]) is the usual interactive one: of all wrong areas (missed or extra, 4-connected),
//! take the largest, and click its point farthest from its own edge (the image border counts as an edge; ties go to
//! the first in reading order). A missed area gets an include click, an extra one an exclude click. With nothing
//! selected yet, that is the middle of the object's largest part.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// A coverage plane, `width × height`, row by row, 0 (out) to 1 (in).
#[derive(Clone, Debug)]
pub struct Mask {
    width: usize,
    height: usize,
    values: Vec<f32>,
}

impl Mask {
    /// `None` when the sizes do not add up or the plane is empty.
    pub fn new(width: usize, height: usize, values: Vec<f32>) -> Option<Mask> {
        (width > 0 && height > 0 && width.checked_mul(height) == Some(values.len())).then_some(Mask { width, height, values })
    }

    /// From 8-bit grey (255 = in).
    pub fn from_grey(width: usize, height: usize, grey: &[u8]) -> Option<Mask> {
        Mask::new(width, height, grey.iter().map(|&v| f32::from(v) / 255.0).collect())
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// Which pixels count as selected (coverage 0.5 or more).
    pub fn binary(&self) -> Vec<bool> {
        self.values.iter().map(|&v| v >= 0.5).collect()
    }
}

/// The numbers of one mask against its truth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scores {
    pub iou: f64,
    pub bg_iou: f64,
    pub boundary_f: f64,
    pub band_iou: f64,
    pub gaps_kept: Option<f64>,
    pub sad: f64,
    pub mse: f64,
}

/// How far two outlines may be apart and still match: 0.2 % of the long side, at least 1 px.
pub fn tolerance(width: usize, height: usize) -> usize {
    ((width.max(height) as f64 * 0.002).round() as usize).max(1)
}

/// The half-width of the band round the truth's outline: 1 % of the long side, at least 2 px.
pub fn band_radius(width: usize, height: usize) -> usize {
    ((width.max(height) as f64 * 0.01).round() as usize).max(2)
}

/// Every metric of `pred` against `truth` (`ignore`: pixels left out, same size). `None` when sizes differ.
pub fn score(pred: &Mask, truth: &Mask, ignore: Option<&[bool]>) -> Option<Scores> {
    let (w, h) = (truth.width, truth.height);
    if (pred.width, pred.height) != (w, h) || ignore.is_some_and(|i| i.len() != w * h) {
        return None;
    }
    let ignored = |i: usize| ignore.is_some_and(|m| m.get(i).copied().unwrap_or(false));
    let p = pred.binary();
    let t = truth.binary();
    let (iou, bg_iou) = (iou(&p, &t, &ignored), iou(&invert(&p), &invert(&t), &ignored));
    let p_edge = outline(&p, w, h, &ignored);
    let t_edge = outline(&t, w, h, &ignored);
    let boundary_f = boundary_f(&p_edge, &t_edge, w, h, tolerance(w, h));
    // distance to the truth's outline, for the two bands
    let near = squared_distance(&t_edge, w, h);
    let r = band_radius(w, h) as f64;
    let in_band = |i: usize| near.get(i).copied().unwrap_or(f64::INFINITY) <= r * r;
    let band_iou = self::iou(&p, &t, &|i| ignored(i) || !in_band(i));
    let gaps_kept = gaps_kept(&p, &t, w, h, &ignored);
    let unknown: Vec<bool> = truth.values.iter().enumerate().map(|(i, &v)| unknown_value(v) || in_band(i)).collect();
    let (sad, mse) = sad_mse(&pred.values, &truth.values, &unknown, &ignored);
    Some(Scores { iou, bg_iou, boundary_f, band_iou, gaps_kept, sad, mse })
}

fn invert(m: &[bool]) -> Vec<bool> {
    m.iter().map(|v| !v).collect()
}

/// Intersection over union of the pixels `skip` lets through; two empty selections agree (1).
fn iou(a: &[bool], b: &[bool], skip: &dyn Fn(usize) -> bool) -> f64 {
    let (mut both, mut either) = (0u64, 0u64);
    for (i, (&x, &y)) in a.iter().zip(b).enumerate() {
        if skip(i) {
            continue;
        }
        both += u64::from(x && y);
        either += u64::from(x || y);
    }
    if either == 0 { 1.0 } else { both as f64 / either as f64 }
}

/// The 4-neighbours of pixel `i` that exist.
fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
}

/// The selected pixels with an unselected neighbour. Pixels at or beside an ignored one are left out: an outline there
/// is where the labelling stops, not the object's edge. The image border is not an outline.
fn outline(m: &[bool], w: usize, h: usize, ignored: &dyn Fn(usize) -> bool) -> Vec<bool> {
    (0..m.len())
        .map(|i| m[i] && !ignored(i) && !neighbours(i, w, h).any(ignored) && neighbours(i, w, h).any(|n| !m.get(n).copied().unwrap_or(true)))
        .collect()
}

fn boundary_f(pred: &[bool], truth: &[bool], w: usize, h: usize, tol: usize) -> f64 {
    let count = |m: &[bool]| m.iter().filter(|&&v| v).count();
    let (np, nt) = (count(pred), count(truth));
    if np == 0 || nt == 0 {
        return if np == nt { 1.0 } else { 0.0 };
    }
    let limit = (tol * tol) as f64;
    let within = |from: &[bool], to: &[bool]| {
        let d = squared_distance(to, w, h);
        from.iter().zip(&d).filter(|&(&on, &d)| on && d <= limit).count()
    };
    let precision = within(pred, truth) as f64 / np as f64;
    let recall = within(truth, pred) as f64 / nt as f64;
    if precision + recall == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) }
}

fn unknown_value(v: f32) -> bool {
    v > 0.02 && v < 0.98
}

/// Where a hair edge is judged: the truth's soft pixels and everything within [`band_radius`] of its outline.
pub fn unknown_band(truth: &Mask) -> Vec<bool> {
    let (w, h) = (truth.width, truth.height);
    let near = squared_distance(&outline(&truth.binary(), w, h, &|_| false), w, h);
    let r = band_radius(w, h) as f64;
    truth.values.iter().zip(&near).map(|(&v, &d)| unknown_value(v) || d <= r * r).collect()
}

/// (SAD / 1000, MSE) over `band`.
fn sad_mse(pred: &[f32], truth: &[f32], band: &[bool], ignored: &dyn Fn(usize) -> bool) -> (f64, f64) {
    let (mut sad, mut sse, mut n) = (0.0f64, 0.0f64, 0u64);
    for (i, ((&p, &t), &b)) in pred.iter().zip(truth).zip(band).enumerate() {
        if !b || ignored(i) {
            continue;
        }
        let d = f64::from(p - t).abs();
        sad += d;
        sse += d * d;
        n += 1;
    }
    (sad / 1000.0, if n == 0 { 0.0 } else { sse / n as f64 })
}

/// The 4-connected areas of `on`, each as its pixel indices.
fn areas(on: &[bool], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; on.len()];
    let mut out = Vec::new();
    for start in 0..on.len() {
        if !on[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut area = vec![start];
        let mut k = 0;
        while let Some(&i) = area.get(k) {
            k += 1;
            for n in neighbours(i, w, h) {
                if on.get(n).copied().unwrap_or(false) && !seen[n] {
                    seen[n] = true;
                    area.push(n);
                }
            }
        }
        out.push(area);
    }
    out
}

/// Fewer pixels than this is labelling noise, not a hole.
const MIN_GAP: usize = 8;

fn gaps_kept(pred: &[bool], truth: &[bool], w: usize, h: usize, ignored: &dyn Fn(usize) -> bool) -> Option<f64> {
    let out: Vec<bool> = truth.iter().enumerate().map(|(i, &t)| !t && !ignored(i)).collect();
    let touches_border = |i: usize| {
        let (x, y) = (i % w, i / w);
        x == 0 || y == 0 || x + 1 == w || y + 1 == h
    };
    let (mut kept, mut all) = (0usize, 0usize);
    for gap in areas(&out, w, h) {
        // (a hole: off the border, not beside unlabelled pixels, big enough to matter)
        if gap.len() < MIN_GAP || gap.iter().any(|&i| touches_border(i) || neighbours(i, w, h).any(ignored)) {
            continue;
        }
        all += gap.len();
        kept += gap.iter().filter(|&&i| !pred.get(i).copied().unwrap_or(false)).count();
    }
    (all > 0).then(|| kept as f64 / all as f64)
}

/// The squared Euclidean distance of every pixel to the nearest `target` pixel (infinite when there is none). Exact,
/// in two linear passes (Felzenszwalb and Huttenlocher's lower envelope of parabolas).
pub fn squared_distance(target: &[bool], w: usize, h: usize) -> Vec<f64> {
    let mut d: Vec<f64> = target.iter().map(|&t| if t { 0.0 } else { f64::INFINITY }).collect();
    if w == 0 || h == 0 || d.len() != w * h {
        return d;
    }
    let mut line = vec![0.0; w.max(h)];
    for x in 0..w {
        for y in 0..h {
            line[y] = d[y * w + x];
        }
        for (y, v) in envelope(&line[..h]).into_iter().enumerate() {
            d[y * w + x] = v;
        }
    }
    for row in d.chunks_mut(w) {
        let out = envelope(row);
        row.copy_from_slice(&out);
    }
    d
}

/// `min over q of (p − q)² + f(q)` for every p.
fn envelope(f: &[f64]) -> Vec<f64> {
    // the parabolas of the lower envelope: (apex, from where it is the lowest)
    let mut hull: Vec<(usize, f64)> = Vec::new();
    for (q, &fq) in f.iter().enumerate() {
        if !fq.is_finite() {
            continue;
        }
        let from = loop {
            let Some(&(v, from)) = hull.last() else { break f64::NEG_INFINITY };
            let (qf, vf) = (q as f64, v as f64);
            let s = ((fq + qf * qf) - (f[v] + vf * vf)) / (2.0 * (qf - vf));
            if s > from {
                break s;
            }
            hull.pop();
        };
        hull.push((q, from));
    }
    let mut k = 0;
    (0..f.len())
        .map(|p| {
            while hull.get(k + 1).is_some_and(|&(_, from)| from <= p as f64) {
                k += 1;
            }
            hull.get(k).map_or(f64::INFINITY, |&(v, _)| {
                let d = p as f64 - v as f64;
                d * d + f[v]
            })
        })
        .collect()
}

/// A click at pixel `(x, y)`: include what is there, or exclude it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Click {
    pub x: usize,
    pub y: usize,
    pub include: bool,
}

/// The click the rule puts next (see the module notes). `None` when the mask matches the truth everywhere it is
/// labelled, or the sizes differ.
pub fn next_click(pred: &Mask, truth: &Mask, ignore: Option<&[bool]>) -> Option<Click> {
    let (w, h) = (truth.width, truth.height);
    if (pred.width, pred.height) != (w, h) {
        return None;
    }
    let (p, t) = (pred.binary(), truth.binary());
    let wrong = |include: bool| -> Vec<bool> {
        (0..w * h).map(|i| t[i] == include && p[i] != include && !ignore.is_some_and(|m| m.get(i).copied().unwrap_or(false))).collect()
    };
    let mut best: Option<(Vec<usize>, bool)> = None;
    for include in [true, false] {
        for area in areas(&wrong(include), w, h) {
            if best.as_ref().is_none_or(|(b, _)| area.len() > b.len()) {
                best = Some((area, include));
            }
        }
    }
    let (area, include) = best?;
    // the deepest point of the area: farthest from anything outside it, the image border included
    let mut outside = vec![true; (w + 2) * (h + 2)];
    for &i in &area {
        outside[(i / w + 1) * (w + 2) + i % w + 1] = false;
    }
    let depth = squared_distance(&outside, w + 2, h + 2);
    let at = |i: usize| depth.get((i / w + 1) * (w + 2) + i % w + 1).copied().unwrap_or(0.0);
    let deepest = area.iter().copied().fold(None, |b: Option<usize>, i| match b {
        Some(b) if at(b) > at(i) || (at(b) == at(i) && b < i) => Some(b),
        _ => Some(i),
    })?;
    Some(Click { x: deepest % w, y: deepest / w, include })
}

#[cfg(test)]
mod tests;
