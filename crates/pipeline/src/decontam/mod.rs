//! Fork feature (`fork/decontaminate.md`, issue #115): colour decontamination at a mask's soft edge.
//!
//! A pixel a mask half-selects (a strand of hair against the sky) is a mix of two colours: `I = a·S + (1 − a)·U`, `S`
//! the colour of what the mask selects (the hair), `U` the colour of what it leaves (the sky). Editing the pixel
//! as one colour keeps the sky's blue in the strand whatever is done to the background. [`estimate_edge`] recovers `S`
//! and `U` from the photo and the mask's alpha by blur-fusion foreground estimation (Forte and Pitié, "Approximate Fast
//! Foreground Colour Estimation", ICIP 2021): local averages of the selected and the unselected colours, corrected so
//! that they mix back to the pixel exactly. Two passes: a wide window for colours far from the edge, a narrow one for
//! the edge itself. Windows are fractions of the frame's long side, so a preview and an export agree.
//!
//! Only the edge is estimated and kept (the pixels whose alpha is strictly between [`EDGE`]'s bounds), band by band
//! of rows, each band reading its windows' reach around it. No model, linear time (box means by running sums).
//! The finish stage edits the two colours apart ([`pixel`]).

use std::sync::Arc;

use lightcraft_develop::{LocalAdjustments, Mask};
use lightcraft_raster::{Rgb32f, par_rows};

use crate::masks::MaskAlpha;

pub(crate) mod pixel;

/// A decontaminated mask's edge in one render: its id, the Amount (0..1), and its edge's colours.
pub struct Split {
    pub id: u32,
    pub amount: f32,
    pub edge: Arc<Edge>,
}

/// One edge pixel: its column and its selected and unselected colours (scene linear, before exposure).
#[derive(Clone, Copy, Debug, PartialEq)]
struct EdgePx {
    x: u32,
    selected: [f32; 3],
    unselected: [f32; 3],
}

/// The selected and unselected colours of a mask's edge pixels, row by row (columns increasing).
#[derive(Debug, Default, PartialEq)]
pub struct Edge {
    /// Where each row's pixels start in `px` (`height + 1` entries; empty when nothing is kept).
    rows: Vec<usize>,
    px: Vec<EdgePx>,
}

impl Edge {
    /// The selected and unselected colours at `(x, y)`, when it is an edge pixel.
    pub fn at(&self, x: usize, y: usize) -> Option<([f32; 3], [f32; 3])> {
        let (&a, &b) = (self.rows.get(y)?, self.rows.get(y.checked_add(1)?)?);
        let row = self.px.get(a..b)?;
        let x = u32::try_from(x).ok()?;
        let k = row.binary_search_by_key(&x, |p| p.x).ok()?;
        row.get(k).map(|p| (p.selected, p.unselected))
    }

    /// Number of edge pixels.
    pub fn len(&self) -> usize {
        self.px.len()
    }

    pub fn is_empty(&self) -> bool {
        self.px.is_empty()
    }

    /// Memory held (bytes).
    pub fn bytes(&self) -> usize {
        self.px.len() * std::mem::size_of::<EdgePx>() + self.rows.len() * std::mem::size_of::<usize>()
    }

    /// Which `cell` × `cell` squares of a frame `width` pixels wide hold an edge pixel: `width.div_ceil(cell)` columns by
    /// `height.div_ceil(cell)` rows, row-major (fork, `fork/perf-decontam-gpu.md`: where a retouched region can reach
    /// the edge's estimate).
    pub fn occupied(&self, cell: usize, width: usize) -> Vec<bool> {
        let (cell, rows) = (cell.max(1), self.rows.len().saturating_sub(1));
        let cols = width.div_ceil(cell);
        let mut hit = vec![false; cols.saturating_mul(rows.div_ceil(cell))];
        for y in 0..rows {
            let (Some(&a), Some(&b)) = (self.rows.get(y), self.rows.get(y + 1)) else { continue };
            for p in self.px.get(a..b).unwrap_or(&[]) {
                if let Some(c) = hit.get_mut((y / cell).saturating_mul(cols).saturating_add(p.x as usize / cell)) {
                    *c = true;
                }
            }
        }
        hit
    }

    /// The edge as the GPU's finish kernel reads it (fork, `fork/perf-decontam-gpu.md`): `height + 1` row starts
    /// (pixel indices, as `u32` bits), then [`PX_WORDS`] words a pixel: its column (`u32` bits), its selected and its
    /// unselected colour. `None` when a count doesn't fit in 32 bits or the edge was estimated for another height.
    pub fn device_words(&self, height: usize) -> Option<Vec<f32>> {
        if self.rows.len() != height.checked_add(1)? {
            return None;
        }
        let total = self.rows.len().checked_add(self.px.len().checked_mul(PX_WORDS)?)?;
        u32::try_from(total).ok()?;
        let mut w = Vec::with_capacity(total);
        w.extend(self.rows.iter().map(|&r| f32::from_bits(r as u32)));
        for p in &self.px {
            w.push(f32::from_bits(p.x));
            w.extend_from_slice(&p.selected);
            w.extend_from_slice(&p.unselected);
        }
        Some(w)
    }
}

/// Words of one edge pixel on the GPU ([`Edge::device_words`]).
pub const PX_WORDS: usize = 7;

/// What a decontaminated mask's edge is keyed by in a stage cache (CPU and GPU alike): the image it is estimated
/// from (`lin_key`), the mask's alpha (`masks::alpha_key`) and the frame's scale; never its Amount or sliders.
pub fn split_key(lin_key: u64, alpha_key: u64, px_per_long: f64) -> u64 {
    crate::hash_of((lin_key, alpha_key, px_per_long.to_bits()))
}

/// Which parts of the finish stage a decontaminated mask with effect terms `t` edits apart: its scene edits, its
/// colour edits (`crate::finish::effect_terms_with`'s block). The GPU kernel reads them from its descriptor.
pub fn layers_of(t: &[f32]) -> (bool, bool) {
    (scene_terms(t), colour_terms(t))
}

/// [`estimate_edge`], counted in [`estimates`] (the GPU render estimates through this, as the CPU's [`splits`] does).
pub fn estimate_counted(img: &Rgb32f, alpha: &MaskAlpha, long_px: f64) -> Edge {
    ESTIMATES.with(|c| c.set(c.get() + 1));
    estimate_edge(img, alpha, long_px)
}

/// Below and above these alphas a pixel is wholly in one layer: it is drawn as before.
pub const EDGE: (f32, f32) = (0.002, 0.998);

/// The wide and narrow windows, as fractions of the long side (the paper's 90 and 6 px at about 2000 px).
pub const WIDE: f64 = 0.045;
pub const NARROW: f64 = 0.003;

/// Rows estimated together: a band reads its windows' reach above and below it, so taller bands read less twice.
const BAND: usize = 512;

/// Whether the adjustments `j` hold an edit the split changes: one made in scene linear before the tone stage
/// (Exposure, Temp, Tint, Dehaze, Noise, Moiré, Defringe) or a colour edit after it (Saturation, Hue, Color, Recolor).
/// Tone edits (Contrast, Highlights, Shadows, Whites, Blacks, Texture, Clarity, Sharpness) read the mixed pixel's
/// neighbourhood and run once on the composite.
pub fn edits_apart(j: &LocalAdjustments) -> bool {
    let t = crate::finish::mask_terms(j);
    scene_terms(&t) || colour_terms(&t) || crate::recolor::is_on(j)
}

/// Whether mask `m` is decontaminated in a render: visible, Decontaminate above 0, an Amount, and an edit the split
/// changes ([`edits_apart`]). The GPU render falls back to the CPU for it; a windowed render reads around itself.
pub fn applies(m: &Mask) -> bool {
    m.visible && !m.is_waiting() && m.decontaminate.is_finite() && m.decontaminate > 0.0 && m.adjust.amount > 0.0 && edits_apart(&m.adjust)
}

/// How far (output pixels) a decontaminated edge reads around a pixel in a frame whose long side is `long_px`: the
/// wide and the narrow window, and a pixel each for rounding.
pub fn reach(long_px: f64) -> usize {
    let r = |f: f64| ((f * long_px).round().max(1.0)) as usize;
    r(WIDE).saturating_add(r(NARROW)).saturating_add(2)
}

/// Scene terms of a mask's term block ([`crate::finish::mask_terms`]): exposure, temp, tint, dehaze, noise, moiré,
/// defringe.
pub(crate) fn scene_terms(t: &[f32]) -> bool {
    [0, 1, 2, 10, 14, 15, 16].iter().any(|&k| t.get(k).is_some_and(|v| *v != 0.0))
}

/// Colour terms of a mask's term block: saturation, hue, the colour overlay, Recolor.
pub(crate) fn colour_terms(t: &[f32]) -> bool {
    let at = |k: usize| t.get(k).copied().unwrap_or(0.0);
    let overlay = at(crate::finish::MASK_SUMS) > 0.0 && at(crate::finish::MASK_SUMS + 3) > 0.0;
    at(11) != 0.0 || at(12) != 0.0 || overlay || at(crate::finish::MASK_RECOLOR) > 0.0
}

thread_local! {
    static ESTIMATES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many edges this thread has estimated (tests: a kept edge is not estimated again).
pub fn estimates() -> usize {
    ESTIMATES.with(std::cell::Cell::get)
}

/// The kept edges of the last render, by key ([`splits`]).
pub(crate) type Kept = Vec<(u64, Arc<Edge>)>;

/// The splits of the render's decontaminated masks ([`applies`]; `masks` are the settings' masks, the render's
/// evaluated masks give the alphas), and the edges to keep for the next render. `key` keys a mask's edge by what it is
/// estimated from (the image and the mask's alpha, not its Amount or sliders); an edge in `kept` under that key is
/// reused.
pub(crate) fn splits(p: &crate::Prepared, masks: &[Mask], key: impl Fn(&Mask) -> u64, kept: &Kept) -> (Vec<Split>, Kept) {
    let mut used: Kept = Vec::new();
    let split = masks
        .iter()
        .filter(|m| applies(m))
        .filter_map(|m| {
            let e = p.masks.iter().find(|e| e.id == m.id)?;
            if e.alpha.is_zero() {
                return None;
            }
            let k = key(m);
            let edge = match used.iter().chain(kept).find(|(o, _)| *o == k) {
                Some((_, edge)) => edge.clone(),
                None => Arc::new(estimate_counted(&p.img, &e.alpha, p.px_per_long)),
            };
            if !used.iter().any(|(o, _)| *o == k) {
                used.push((k, edge.clone()));
            }
            Some(Split { id: m.id, amount: (m.decontaminate / 100.0).clamp(0.0, 1.0) as f32, edge })
        })
        .collect();
    (split, used)
}

/// A rectangle of pixels, right and bottom exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Rect {
    fn w(self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    fn h(self) -> usize {
        self.y1.saturating_sub(self.y0)
    }

    /// Grown by `r` on every side, inside a `w × h` frame.
    fn grow(self, r: usize, w: usize, h: usize) -> Rect {
        Rect {
            x0: self.x0.saturating_sub(r),
            y0: self.y0.saturating_sub(r),
            x1: self.x1.saturating_add(r).min(w),
            y1: self.y1.saturating_add(r).min(h),
        }
    }
}

/// Colours over a rectangle of the frame.
struct Region {
    at: Rect,
    data: Vec<[f32; 3]>,
}

impl Region {
    fn get(&self, x: usize, y: usize) -> [f32; 3] {
        let (Some(rx), Some(ry)) = (x.checked_sub(self.at.x0), y.checked_sub(self.at.y0)) else { return [0.0; 3] };
        if rx >= self.at.w() {
            return [0.0; 3];
        }
        self.data.get(ry * self.at.w() + rx).copied().unwrap_or([0.0; 3])
    }
}

/// An alpha as the estimate reads it: 0..1, a non-finite value 0.
fn sane(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

fn is_edge(v: f32) -> bool {
    v > EDGE.0 && v < EDGE.1
}

/// Box means of radius `r` of `v` (read over `inp`) at every pixel of `out` (inside `inp`), each window clamped to
/// `inp` and divided by how many values it holds. Equal to the whole frame's box mean wherever a window lies inside
/// `inp` or reaches only frame borders that `inp` shares.
fn box_mean(v: &(impl Fn(usize, usize) -> f32 + Sync), inp: Rect, out: Rect, r: usize) -> Vec<f32> {
    let (iw, ow) = (inp.w(), out.w());
    let mut rows = vec![0f64; inp.h() * ow];
    // the row means of every input row, at the output columns
    par_rows(&mut rows, ow, |ry, t| {
        let y = inp.y0 + ry;
        let mut prefix = Vec::with_capacity(iw + 1);
        let mut sum = 0f64;
        prefix.push(sum);
        for x in inp.x0..inp.x1 {
            sum += f64::from(v(x, y));
            prefix.push(sum);
        }
        for (ox, t) in t.iter_mut().enumerate() {
            let x = out.x0 + ox;
            let (a, b) = (x.saturating_sub(r).max(inp.x0), x.saturating_add(r + 1).min(inp.x1));
            let s = prefix.get(b - inp.x0).copied().unwrap_or(0.0) - prefix.get(a.min(b) - inp.x0).copied().unwrap_or(0.0);
            *t = s / b.saturating_sub(a).max(1) as f64;
        }
    });
    // running sums down the columns, in place
    for ry in 1..inp.h() {
        let (done, rest) = rows.split_at_mut(ry * ow);
        if let (Some(prev), Some(row)) = (done.get((ry - 1) * ow..), rest.get_mut(..ow)) {
            row.iter_mut().zip(prev).for_each(|(v, p)| *v += *p);
        }
    }
    let mut o = vec![0f32; out.h() * ow];
    par_rows(&mut o, ow, |oy, row| {
        let y = out.y0 + oy;
        let (a, b) = (y.saturating_sub(r).max(inp.y0), y.saturating_add(r + 1).min(inp.y1));
        let n = b.saturating_sub(a).max(1) as f64;
        // (`rows[k]` sums input rows 0..=k)
        let sum_to = |end: usize, ox: usize| if end <= inp.y0 { 0.0 } else { rows.get((end - inp.y0 - 1) * ow + ox).copied().unwrap_or(0.0) };
        for (ox, v) in row.iter_mut().enumerate() {
            *v = ((sum_to(b, ox) - sum_to(a, ox)) / n) as f32;
        }
    });
    o
}

/// One blur-fusion pass with window `r`, reading `inp` and refining the selected and unselected colours at `out`:
/// from the photo (`prev` none) or from the previous pass (`prev`, over `inp`).
fn pass(img: &Rgb32f, alpha: &MaskAlpha, prev: Option<(&Region, &Region)>, inp: Rect, out: Rect, r: usize) -> (Region, Region) {
    const EPS: f32 = 1e-5;
    let w = img.width;
    let photo = |x: usize, y: usize| img.data.get(y * w + x).copied().unwrap_or([0.0; 3]);
    let s_at = |x: usize, y: usize| prev.map_or_else(|| photo(x, y), |(s, _)| s.get(x, y));
    let u_at = |x: usize, y: usize| prev.map_or_else(|| photo(x, y), |(_, u)| u.get(x, y));
    let a = |x: usize, y: usize| sane(alpha.at(x, y));
    let ba = box_mean(&a, inp, out, r);
    let bs: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&|x, y| s_at(x, y)[c] * a(x, y), inp, out, r));
    let bu: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&|x, y| u_at(x, y)[c] * (1.0 - a(x, y)), inp, out, r));
    let n = out.w() * out.h();
    let (mut s, mut u) = (vec![[0.0f32; 3]; n], vec![[0.0f32; 3]; n]);
    for (i, (sp, up)) in s.iter_mut().zip(u.iter_mut()).enumerate() {
        let (x, y) = (out.x0 + i % out.w().max(1), out.y0 + i / out.w().max(1));
        let (al, m, i_px) = (a(x, y), ba.get(i).copied().unwrap_or(0.0), photo(x, y));
        for c in 0..3 {
            let fs = bs[c].get(i).copied().unwrap_or(0.0) / (m + EPS);
            let fu = bu[c].get(i).copied().unwrap_or(0.0) / (1.0 - m + EPS);
            let res = i_px[c] - al * fs - (1.0 - al) * fu;
            sp[c] = (fs + al * res).max(0.0);
            up[c] = (fu + (1.0 - al) * res).max(0.0);
        }
    }
    (Region { at: out, data: s }, Region { at: out, data: u })
}

/// The selected and unselected colours of the edge pixels of `img` under `alpha` (same size), with windows for a frame
/// whose long side is `long_px` output pixels. A size mismatch keeps no pixel.
pub fn estimate_edge(img: &Rgb32f, alpha: &MaskAlpha, long_px: f64) -> Edge {
    estimate_in_bands(img, alpha, long_px, BAND)
}

/// [`estimate_edge`] with bands of `band` rows.
fn estimate_in_bands(img: &Rgb32f, alpha: &MaskAlpha, long_px: f64, band: usize) -> Edge {
    let (w, h) = (img.width, img.height);
    if alpha.width != w || alpha.height != h || w == 0 || h == 0 {
        return Edge::default();
    }
    let radius = |f: f64| ((f * long_px).round().max(1.0) as usize).min(w.max(h));
    let (rw, rn) = (radius(WIDE), radius(NARROW));
    // each row's first and last edge column
    let spans: Vec<Option<(usize, usize)>> = (0..h)
        .map(|y| {
            let (x0, row) = alpha.row(y)?;
            let first = row.iter().position(|v| is_edge(*v))?;
            let last = row.iter().rposition(|v| is_edge(*v)).unwrap_or(first);
            Some((x0 + first, x0 + last + 1))
        })
        .collect();
    let mut edge = Edge { rows: Vec::with_capacity(h + 1), px: Vec::new() };
    edge.rows.push(0);
    for y0 in (0..h).step_by(band.max(1)) {
        let y1 = y0.saturating_add(band.max(1)).min(h);
        let (x0, x1) = spans.get(y0..y1).unwrap_or(&[]).iter().flatten().fold((usize::MAX, 0), |(a, b), &(l, r)| (a.min(l), b.max(r)));
        if x1 > x0 {
            // the output, what the narrow pass reads, what the wide pass reads
            let o = Rect { x0, y0, x1, y1 };
            let p = o.grow(rn, w, h);
            let q = p.grow(rw, w, h);
            let (s1, u1) = pass(img, alpha, None, q, p, rw);
            let (s, u) = pass(img, alpha, Some((&s1, &u1)), p, o, rn);
            drop((s1, u1));
            for y in y0..y1 {
                for x in x0..x1 {
                    let raw = alpha.at(x, y);
                    if !is_edge(raw) {
                        continue;
                    }
                    let al = sane(raw);
                    let i_px = img.data.get(y * w + x).copied().unwrap_or([0.0; 3]);
                    let (mut sp, mut up) = (s.get(x, y), u.get(x, y));
                    // the update mixes back to the pixel only approximately: the remainder is shared by the two colours
                    // in proportion to their weights (the smallest change that makes `a·S + (1 − a)·U` the pixel), so an
                    // edge the masks leave alone renders as before
                    let norm = al * al + (1.0 - al) * (1.0 - al);
                    for c in 0..3 {
                        let r = (i_px[c] - (al * sp[c] + (1.0 - al) * up[c])) / norm;
                        sp[c] = (sp[c] + al * r).max(0.0);
                        up[c] = (up[c] + (1.0 - al) * r).max(0.0);
                    }
                    edge.px.push(EdgePx { x: u32::try_from(x).unwrap_or(u32::MAX), selected: sp, unselected: up });
                }
                edge.rows.push(edge.px.len());
            }
        } else {
            edge.rows.extend(std::iter::repeat_n(edge.px.len(), y1 - y0));
        }
    }
    edge
}

#[cfg(test)]
mod tests;
