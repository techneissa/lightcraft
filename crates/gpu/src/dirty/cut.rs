//! The spatial stages on a cut of the image (issue #111): what a rectangle of a stage's output needs from its input,
//! and the same kernels run on just that much. Every filter here is local: box passes (the vertical one summing
//! directly or starting its runs on the whole image's rows, see [`box_chunk`]), medians, resampling taps and
//! pointwise kernels. So a cut with the margin this module computes gives the whole image's bits in the rectangle.

use lightcraft_pipeline::nrc_joint::{self, JointNoise};
use lightcraft_pipeline::{Plan, local};
use lightcraft_raster::resample::{Filter, weights};

use crate::ctx::{Buf, groups2};
use crate::render::{
    BOX_DIRECT_MAX, Cx, box_chunk, gaussian_at, guided_at, guided_coeffs_at, joint_band, joint_halo, map, median_hv, nrcj, pack_taps, resize_with,
};

/// `[x0, y0, x1, y1)` in pixels of the whole image.
pub(crate) type Rect = [usize; 4];

/// How far a change spreads through a stage (and the margin it reads around what it makes), across and down.
pub(crate) type Reach = (usize, usize);

pub(crate) fn width(r: &Rect) -> usize {
    r[2].saturating_sub(r[0])
}

pub(crate) fn height(r: &Rect) -> usize {
    r[3].saturating_sub(r[1])
}

pub(crate) fn area(r: &Rect) -> usize {
    width(r) * height(r)
}

/// `r` grown by `m` on every side, inside a `w × h` image.
pub(crate) fn grow(r: &Rect, m: Reach, (w, h): (usize, usize)) -> Rect {
    [r[0].saturating_sub(m.0), r[1].saturating_sub(m.1), r[2].saturating_add(m.0).min(w), r[3].saturating_add(m.1).min(h)]
}

pub(crate) fn add(a: Reach, b: Reach) -> Reach {
    (a.0.saturating_add(b.0), a.1.saturating_add(b.1))
}

fn twice(a: Reach) -> Reach {
    add(a, a)
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    outer[0] <= inner[0] && outer[1] <= inner[1] && inner[2] <= outer[2] && inner[3] <= outer[3]
}

/// An image of `nc` channels holding the rectangle `at` of a whole image (rows of `width(at)` pixels).
#[derive(Clone, Copy)]
pub(crate) struct Part<'a> {
    pub buf: &'a Buf,
    pub at: Rect,
    pub nc: usize,
}

impl<'a> Part<'a> {
    /// A whole `w × h` image.
    pub(crate) fn whole(buf: &'a Buf, (w, h): (usize, usize), nc: usize) -> Part<'a> {
        Part { buf, at: [0, 0, w, h], nc }
    }
}

/// Copy the `rw × rh` pixels at `from` of `src` (rows of `sw`) to `to` in `dst` (rows of `dw`), `nc` channels each.
pub(crate) fn copy_rect(
    cx: &mut Cx<'_>,
    nc: usize,
    (src, sw, from): (&Buf, usize, (usize, usize)),
    (dst, dw, to): (&Buf, usize, (usize, usize)),
    (rw, rh): (usize, usize),
) {
    if rw == 0 || rh == 0 {
        return;
    }
    let p = [nc, sw, from.0, from.1, dw, to.0, to.1, rw, rh].map(|v| v as u32);
    cx.run("rect_copy", &p, &[Some(src), Some(dst)], groups2(rw, rh, [16, 16]));
}

/// Clear the rectangle `r` of the whole image `dst` (rows of `w` pixels, `nc` channels) to zero words.
pub(crate) fn clear_rect(cx: &mut Cx<'_>, nc: usize, dst: &Buf, w: usize, r: &Rect) {
    let (rw, rh) = (width(r), height(r));
    if rw == 0 || rh == 0 {
        return;
    }
    let p = [nc, w, r[0], r[1], rw, rh].map(|v| v as u32);
    cx.run("rect_clear", &p, &[None, Some(dst)], groups2(rw, rh, [16, 16]));
}

/// The rectangle `r` of `part` as an image of its own; `None` when `part` does not hold all of it.
pub(crate) fn cut(cx: &mut Cx<'_>, part: &Part<'_>, r: &Rect) -> Option<Buf> {
    if !contains(&part.at, r) {
        return None;
    }
    let out = cx.gpu.buffer(area(r) * part.nc);
    copy_rect(cx, part.nc, (part.buf, width(&part.at), (r[0] - part.at[0], r[1] - part.at[1])), (&out, width(r), (0, 0)), (width(r), height(r)));
    Some(out)
}

/// `src` (the rectangle `r`, `nc` channels) put into the whole image `dst` of width `w`.
pub(crate) fn paste(cx: &mut Cx<'_>, src: &Buf, r: &Rect, nc: usize, dst: &Buf, w: usize) {
    copy_rect(cx, nc, (src, width(r), (0, 0)), (dst, w, (r[0], r[1])), (width(r), height(r)));
}

/// Reach of a [`gaussian_at`] of `sigma` over a `whole` image: its box radii across; down, each radius plus the run a
/// vertical pass slides (a changed row's rounding goes down the rest of its run, and a run starts up to a run above).
pub(crate) fn gauss_reach(sigma: f32, (w, h): (usize, usize)) -> Reach {
    let Some(radii) = lightcraft_raster::blur::radii_for(sigma, w, h) else { return (0, 0) };
    radii.iter().filter(|r| **r > 0).fold((0, 0), |(x, y), r| {
        let run = if *r <= BOX_DIRECT_MAX { 0 } else { box_chunk(*r) };
        (x + r, y + r + run)
    })
}

/// [`gaussian_at`] of `part` over the whole image `whole`, for the rectangle `out`.
pub(crate) fn gaussian(cx: &mut Cx<'_>, part: &Part<'_>, whole: (usize, usize), sigma: f32, out: &Rect) -> Option<Buf> {
    let i = grow(out, gauss_reach(sigma, whole), whole);
    let c = cut(cx, part, &i)?;
    let b = gaussian_at(cx, &c, (width(&i), height(&i)), part.nc, sigma, (whole, i[1]));
    cut(cx, &Part { buf: &b, at: i, nc: part.nc }, out)
}

/// Self-guided filter (`render::guided`) of the plane `part`, for `out`.
pub(crate) fn guided(cx: &mut Cx<'_>, part: &Part<'_>, whole: (usize, usize), sigma: f32, eps: f32, out: &Rect) -> Option<Buf> {
    let i = grow(out, twice(gauss_reach(sigma, whole)), whole);
    let c = cut(cx, part, &i)?;
    let q = guided_at(cx, &c, (width(&i), height(&i)), sigma, eps, (whole, i[1]));
    cut(cx, &Part { buf: &q, at: i, nc: 1 }, out)
}

/// The source indices `[lo, hi)` the taps of outputs `a..b` read.
fn tap_span(w: &[(usize, Vec<f32>)], a: usize, b: usize) -> (usize, usize) {
    let taps = w.get(a..b).unwrap_or_default();
    let lo = taps.iter().map(|t| t.0).min().unwrap_or(0);
    let hi = taps.iter().map(|t| t.0 + t.1.len()).max().unwrap_or(0);
    (lo, hi.max(lo))
}

/// The regions a fast guided filter (`render::guided_fast`) of a `whole` plane works on for the output `out`.
struct Fast {
    /// Subsampling step and the small grid's size.
    s: usize,
    small: (usize, usize),
    /// Taps of the subsampling (whole → small) and of the way back (small → whole), across and down.
    down: [Vec<(usize, Vec<f32>)>; 2],
    up: [Vec<(usize, Vec<f32>)>; 2],
    /// The small grid's coefficients made (with margin for their blurs), and the whole image's pixels they read.
    small_at: Rect,
    read: Rect,
}

fn fast(whole: (usize, usize), sigma: f32, out: &Rect) -> Option<Fast> {
    let s = local::guided_fast_step(sigma);
    if s <= 1 {
        return None;
    }
    let (w, h) = whole;
    let small = (w.div_ceil(s).max(1), h.div_ceil(s).max(1));
    let up = [weights(small.0, w, Filter::Bilinear), weights(small.1, h, Filter::Bilinear)];
    let (ux, uy) = (tap_span(&up[0], out[0], out[2]), tap_span(&up[1], out[1], out[3]));
    let small_at = grow(&[ux.0, uy.0, ux.1, uy.1], twice(gauss_reach(sigma / s as f32, small)), small);
    let down = [weights(w, small.0, Filter::Box), weights(h, small.1, Filter::Box)];
    let (dx, dy) = (tap_span(&down[0], small_at[0], small_at[2]), tap_span(&down[1], small_at[1], small_at[3]));
    Some(Fast { s, small, down, up, small_at, read: [dx.0, dy.0, dx.1, dy.1] })
}

/// Pixels of the plane a fast guided filter of `sigma` reads for `out`.
fn fast_reads(whole: (usize, usize), sigma: f32, out: &Rect) -> Rect {
    match fast(whole, sigma, out) {
        Some(f) => f.read,
        None => grow(out, twice(gauss_reach(sigma, whole)), whole),
    }
}

/// Reach of a fast guided filter of `sigma`.
pub(crate) fn fast_reach(whole: (usize, usize), sigma: f32) -> Reach {
    let s = local::guided_fast_step(sigma);
    if s <= 1 {
        return twice(gauss_reach(sigma, whole));
    }
    let small = (whole.0.div_ceil(s).max(1), whole.1.div_ceil(s).max(1));
    let (x, y) = twice(gauss_reach(sigma / s as f32, small));
    // (the subsampling box and the bilinear way back each reach about a cell further)
    ((x + 3) * s, (y + 3) * s)
}

/// Fast guided filter (`render::guided_fast`) of the plane `part`, for `out`.
pub(crate) fn guided_fast(cx: &mut Cx<'_>, part: &Part<'_>, whole: (usize, usize), sigma: f32, eps: f32, out: &Rect) -> Option<Buf> {
    let Some(f) = fast(whole, sigma, out) else { return guided(cx, part, whole, sigma, eps, out) };
    let (read, sa) = (f.read, f.small_at);
    let p = cut(cx, part, &read)?;
    let slice = |w: &[(usize, Vec<f32>)], a: usize, b: usize, shift: usize| pack_taps(w.get(a..b).unwrap_or_default(), shift);
    let lo = resize_with(
        cx,
        &p,
        (width(&read), height(&read)),
        (width(&sa), height(&sa)),
        1,
        (&slice(&f.down[0], sa[0], sa[2], read[0]), &slice(&f.down[1], sa[1], sa[3], read[1])),
    );
    let ab = guided_coeffs_at(cx, &lo, (width(&sa), height(&sa)), sigma / f.s as f32, eps, (f.small, sa[1]));
    let up = resize_with(
        cx,
        &ab,
        (width(&sa), height(&sa)),
        (width(out), height(out)),
        2,
        (&slice(&f.up[0], out[0], out[2], sa[0]), &slice(&f.up[1], out[1], out[3], sa[1])),
    );
    let pq = cut(cx, part, out)?;
    let q = cx.gpu.buffer(area(out));
    map(cx, "guided_apply", area(out), &[], [Some(&pq), Some(&up), None], &q);
    Some(q)
}

/// The highlights/shadows base (`render::tone_base`) of the plane `part`, for `out`.
pub(crate) fn tone_base(cx: &mut Cx<'_>, part: &Part<'_>, whole: (usize, usize), sigma: f32, detail: usize, out: &Rect) -> Option<Buf> {
    if detail == 0 {
        return guided_fast(cx, part, whole, sigma, local::BASE_EPS, out);
    }
    let m_at = fast_reads(whole, sigma, out);
    let i = grow(&m_at, (detail.min(local::BASE_DETAIL_MAX), detail.min(local::BASE_DETAIL_MAX)), whole);
    let c = cut(cx, part, &i)?;
    let m = median_hv(cx, &c, width(&i), height(&i), detail);
    let m = cut(cx, &Part { buf: &m, at: i, nc: 1 }, &m_at)?;
    guided_fast(cx, &Part { buf: &m, at: m_at, nc: 1 }, whole, sigma, local::BASE_EPS, out)
}

/// Reach of the highlights/shadows base.
pub(crate) fn tone_base_reach(whole: (usize, usize), sigma: f32, detail: usize) -> Reach {
    let d = detail.min(local::BASE_DETAIL_MAX);
    add(fast_reach(whole, sigma), (d, d))
}

/// Reach of noise reduction (`render::denoise`) in `plan`.
pub(crate) fn denoise_reach(plan: &Plan<'_>) -> Reach {
    let whole = (plan.w, plan.h);
    let (lum, col) = local::nr_params(&plan.settings, plan.detail_scale);
    let l = lum.map_or((0, 0), |nr| twice(gauss_reach(nr.sigma, whole)));
    let c = match joint_scales(plan) {
        Some(scales) => (joint_halo(&scales), joint_halo(&scales)),
        None => col.map_or((0, 0), |nr| gauss_reach(nr.sigma, whole)),
    };
    add(l, c)
}

/// The scales of the joint colour NR (`render::color_nr_joint`) in `plan`, when it takes the colour step (empty when
/// it then filters nothing, as `render::denoise` leaves the image).
fn joint_scales(plan: &Plan<'_>) -> Option<Vec<(usize, f32)>> {
    let (_, col) = local::nr_params(&plan.settings, plan.detail_scale);
    if col.is_none() || !local::joint_colour_nr(&plan.settings) {
        return None;
    }
    let Some(p) = local::nrc_params(&plan.settings, plan.detail_scale) else { return Some(Vec::new()) };
    let sigma = p.passes.last().map_or(0.0, |q| q.sigma);
    Some(nrc_joint::scale_plan(sigma, plan.w, plan.h))
}

/// The joint colour NR (`render::color_nr_joint`) of the image `part` (3 channels) for `out`: the filter run on `out`
/// with [`joint_halo`] around it, as one band, with the whole frame's noise model `noise`. `None` without one (the
/// noise is then measured on the whole image, which a spot changes), or when the cut is too small for the filter.
fn colour_joint(cx: &mut Cx<'_>, part: &Part<'_>, plan: &Plan<'_>, noise: Option<&JointNoise>, out: &Rect) -> Option<Buf> {
    let whole = (plan.w, plan.h);
    let d = &plan.settings.detail;
    let (detail, amount) = ((d.nr_color_detail / 100.0) as f32, (d.nr_color / 100.0) as f32);
    let scales = joint_scales(plan)?;
    let strength = nrc_joint::strength(amount);
    if scales.is_empty() || strength <= 0.0 {
        return cut(cx, part, out);
    }
    let mut noise = *noise?;
    noise.tolerance = nrc_joint::tolerance(detail);
    let halo = joint_halo(&scales);
    let r = grow(out, (halo, halo), whole);
    let (rw, rh) = (width(&r), height(&r));
    // (a cut too big for one buffer is denoised whole, in bands)
    if rw < 3 || rh < 3 || !cx.gpu.fits(3 * area(&r)) {
        return None;
    }
    let img = cut(cx, part, &r)?;
    let filtered = joint_band(cx, &img, rw, 0, rh, &noise, nrc_joint::speck_brake(amount), &scales);
    let n = area(&r);
    let o = cx.gpu.buffer(3 * n);
    nrcj(cx, "nrcj_recon", n, &[n as u32, strength.to_bits(), 0, 0], &img, Some(&filtered), &o);
    cut(cx, &Part { buf: &o, at: r, nc: 3 }, out)
}

/// Noise reduction (`render::denoise`) of the image `part` (3 channels), for `out`; `noise`: the whole frame's noise
/// model of the joint colour NR, as `render::denoise` takes it.
pub(crate) fn denoise(cx: &mut Cx<'_>, part: &Part<'_>, plan: &Plan<'_>, noise: Option<&JointNoise>, out: &Rect) -> Option<Buf> {
    let whole = (plan.w, plan.h);
    let (lum, col) = local::nr_params(&plan.settings, plan.detail_scale);
    let joint = joint_scales(plan);
    if joint.is_some() && noise.is_none() {
        return None; // the noise model is measured on the whole image, which a spot changes: denoise it whole
    }
    // what each step makes: the colour step `out`, the luminance step the colour filter's margin around it
    let r1 = match &joint {
        Some(scales) => grow(out, (joint_halo(scales), joint_halo(scales)), whole),
        None => col.map_or(*out, |nr| grow(out, gauss_reach(nr.sigma, whole), whole)),
    };
    let r0 = lum.map_or(r1, |nr| grow(&r1, twice(gauss_reach(nr.sigma, whole)), whole));
    let img0 = cut(cx, part, &r0)?;
    let img1 = match lum {
        Some(nr) => {
            let n0 = area(&r0);
            let l = cx.gpu.buffer(n0);
            map(cx, "log_lum_k", n0, &[], [Some(&img0), None, None], &l);
            let f = guided(cx, &Part { buf: &l, at: r0, nc: 1 }, whole, nr.sigma, nr.eps, &r1)?;
            let img = cut(cx, &Part { buf: &img0, at: r0, nc: 3 }, &r1)?;
            let l = cut(cx, &Part { buf: &l, at: r0, nc: 1 }, &r1)?;
            let o = cx.gpu.buffer(area(&r1) * 3);
            map(cx, "nr_lum", area(&r1), &[nr.k.to_bits(), nr.contrast.to_bits()], [Some(&img), Some(&l), Some(&f)], &o);
            o
        }
        None => img0,
    };
    if joint.is_some() {
        return colour_joint(cx, &Part { buf: &img1, at: r1, nc: 3 }, plan, noise, out);
    }
    match col {
        Some(nr) => {
            let n1 = area(&r1);
            let chroma = cx.gpu.buffer(n1 * 3);
            map(cx, "chroma_k", n1, &[], [Some(&img1), None, None], &chroma);
            let b = gaussian(cx, &Part { buf: &chroma, at: r1, nc: 3 }, whole, nr.sigma, out)?;
            let img = cut(cx, &Part { buf: &img1, at: r1, nc: 3 }, out)?;
            let chroma = cut(cx, &Part { buf: &chroma, at: r1, nc: 3 }, out)?;
            let o = cx.gpu.buffer(area(out) * 3);
            map(cx, "nr_col", area(out), &[nr.t.to_bits()], [Some(&img), Some(&chroma), Some(&b)], &o);
            Some(o)
        }
        None => cut(cx, &Part { buf: &img1, at: r1, nc: 3 }, out),
    }
}

/// A pointwise `map` kernel `k` (`nc_in` channels in, `nc_out` out) on the rectangle `r` of the whole image `src`.
pub(crate) fn pointwise(cx: &mut Cx<'_>, k: &str, src: &Part<'_>, nc_out: usize, r: &Rect) -> Option<Buf> {
    let c = cut(cx, src, r)?;
    let out = cx.gpu.buffer(area(r) * nc_out);
    map(cx, k, area(r), &[], [Some(&c), None, None], &out);
    Some(out)
}
