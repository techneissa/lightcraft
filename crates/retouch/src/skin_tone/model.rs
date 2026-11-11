//! The tone model behind Even Tone and Redness, on the cells of one skin mask ([`super::grid`]).
//!
//! A face's skin varies for two reasons. Its **shading** follows the face's form and the light: smooth over the
//! cheeks and the forehead, and long and narrow where the form turns (the nose's sides, the folds from the nose to
//! the mouth, the eyes' rims, the nostrils). Its **blotches** (age and sun spots, darker or lighter patches,
//! redness) are pigment: roundish, a few hundredths of the face across, and not tied to the form. In a processed
//! photo both darken the skin along the same colour direction (a camera's tone curve turns darker skin redder too),
//! so colour alone cannot tell them apart; their shape and place can. The model:
//!
//! 1. The skin's log colour per cell, the blotch scale ([`EVEN_FINE`]) read from it with the skin's own weights.
//! 2. The **shading**: a local plane fitted to the log luminance over [`EVEN_SHADING`] of the face, robustly (twice
//!    re-weighted, a cell [`ROBUST`] off the plane counting for nothing), so it follows the light's gradients and
//!    the face's slopes but not a blotch.
//! 3. The face's **shading direction**: how this photo's skin colour changes with its brightness over large scales
//!    ([`SHADING_DIRECTION`]). A lightened blotch moves along it, so it takes the colour the skin around it has at
//!    that brightness, not a greyer or pinker one.
//! 4. The luminance left over is a blotch where it is round; where it is long and narrow ([`FORM_TENSOR`],
//!    [`FORM_COHERENCE`]), near the eyes, lips and nostrils or along the nose-to-mouth folds ([`super::zones`]), or
//!    deeper than [`EVEN_KEEP`], it is the face's form and stays. Stubble and other dense fine texture
//!    ([`TEXTURE_KEEP`]) stay too, and Even Tone takes only part of a lighter patch ([`EVEN_LIGHT`]) and almost none
//!    of a highlight ([`EVEN_HIGHLIGHT`]).
//! 5. **Colour** (with the shading's colour taken out) moves towards the skin's colour over [`EVEN_COLOUR`] of the
//!    face.
//! 6. **Redness**: colour with a quarter of the shading's colour taken out ([`RED_SHADING`]). A cell redder than
//!    the skin around it ([`RED_AROUND`]) takes that skin's colour (a red spot); then, where it is still redder than
//!    the face's own calm skin ([`RED_REFERENCE`]), it moves [`RED_MOST`] of the way to it (flushed cheeks, a red
//!    nose); fully from [`RED_FULL`] redder. Its brightness stays.
//!
//! The result is a gain per cell and channel, read back bilinearly and multiplied in by the caller: texture finer
//! than a cell (pores, freckles, stubble) keeps its contrast.

use lightcraft_raster::Image;
use lightcraft_raster::blur::gaussian;

use super::LUMA;
use super::grid::smoothstep;

/// Even Tone: the blotch scale, a fraction of the face (Gaussian sigma).
pub const EVEN_FINE: f64 = 0.008;
/// Even Tone: the scale of the shading's local plane, a fraction of the face.
pub const EVEN_SHADING: f64 = 0.04;
/// Even Tone: a cell this far (natural log of luminance) from the shading's plane counts for nothing in its fit.
pub const ROBUST: f32 = 0.12;
/// Even Tone: the share of a dark blotch's darkness taken away at 100, and of a light one's lightness.
pub const EVEN_DARK: f32 = 0.95;
pub const EVEN_LIGHT: f32 = 0.5;
/// Even Tone: a light patch this much lighter (natural log) fades out of the correction: a highlight, not a blotch.
pub const EVEN_HIGHLIGHT: f32 = 0.15;
/// Even Tone: a difference (natural log of luminance) from the first value is kept more and more, and from the
/// second fully, as structure (a nostril, a deep crease, a beard's edge): blotches up to about 45 % darker are evened.
pub const EVEN_KEEP: [f32; 2] = [0.6, 1.0];
/// Even Tone: the scale of the skin's own colour, a fraction of the face; the share of a colour difference taken
/// away at 100; and the difference (log chroma) that fades out of it (a beard, lips bleeding into the mask).
pub const EVEN_COLOUR: f64 = 0.15;
pub const EVEN_COLOUR_SHARE: f32 = 0.85;
pub const EVEN_COLOUR_KEEP: f32 = 0.5;
/// The scales (fractions of the face) whose difference measures the shading direction.
pub const SHADING_DIRECTION: [f64; 2] = [0.03, 0.15];
/// Form: the scale the luminance's direction is gathered over, a fraction of the face, and the coherence (0…1)
/// from which a difference counts as long and narrow (form), fully from the second.
pub const FORM_TENSOR: f64 = 0.015;
pub const FORM_COHERENCE: [f32; 2] = [0.25, 0.65];
/// Texture: the scale it is gathered over, a fraction of the face, and how many times the face's typical fine
/// texture starts to keep the skin as it is (stubble), fully from the second.
pub const TEXTURE_SCALE: f64 = 0.02;
pub const TEXTURE_KEEP: [f32; 2] = [2.5, 4.5];
/// Redness: the share of the shading's colour that counts as redness (the rest is the photo's tone curve, and
/// keeps shadows warm).
pub const RED_SHADING: f32 = 0.75;
/// Redness: the skin a spot is compared with, a fraction of the face.
pub const RED_AROUND: f64 = 0.12;
/// Redness: the face's calm skin, the cells between these percentiles of redness (its less red skin).
pub const RED_REFERENCE: [f32; 2] = [10.0, 30.0];
/// Redness: how much redder (natural log of red over green) than its reference a cell moves fully at 100.
pub const RED_FULL: f32 = 0.03;
/// Redness: how far towards its reference a red cell moves at 100 (all the way greys the skin: the least red skin
/// is also the least saturated).
pub const RED_MOST: f32 = 0.6;
/// The largest change per channel (natural log), so a cell never jumps.
const MAX_LOG: f32 = 0.7;

/// The cells of one skin mask: each one's mask-weighted sums (red, green, blue, the mask, luminance, luminance²).
pub(super) struct Cells {
    pub gw: usize,
    pub gh: usize,
    pub sums: Vec<[f32; 6]>,
    /// A cell's area in pixels (a full cell's mask sum).
    pub area: f32,
}

/// What the model needs besides the cells: the sliders, scales in cells, and the zones per cell.
pub(super) struct Inputs<'a> {
    pub even: f32,
    pub red: f32,
    /// Cells per face (a fraction of the face times this is in cells).
    pub per_face: f32,
    pub even_zone: &'a [f32],
    pub red_zone: &'a [f32],
}

type V3 = [f32; 3];

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The gain per cell and channel.
pub(super) fn gains(c: &Cells, inp: &Inputs<'_>) -> Image<V3> {
    let n = c.gw * c.gh;
    let mut out = Image::<V3>::new(c.gw, c.gh);
    out.data.iter_mut().for_each(|g| *g = [1.0; 3]);
    if n == 0 || c.sums.len() != n || c.area.is_nan() || c.area <= 0.0 {
        return out;
    }
    let sigma = |fraction: f64| (fraction * f64::from(inp.per_face)) as f32;
    // each cell's coverage and log colour
    let w: Vec<f32> = c.sums.iter().map(|s| (s[3] / c.area).clamp(0.0, 1.0)).collect();
    let logc: Vec<V3> = c.sums.iter().map(|s| if s[3] > 1e-9 { [0, 1, 2].map(|k| (s[k] / s[3]).max(1e-6).ln()) } else { [0.0; 3] }).collect();
    let fine = blur3(&logc, &w, c.gw, c.gh, sigma(EVEN_FINE));
    let wide = blur3(&logc, &w, c.gw, c.gh, sigma(EVEN_COLOUR));
    let sd = shading_direction(&blur3(&logc, &w, c.gw, c.gh, sigma(SHADING_DIRECTION[0])), &wide, &w);
    let mut corr = vec![[0.0f32; 3]; n];
    if inp.even > 0.0 {
        let lum: Vec<f32> = fine.iter().map(|f| dot(*f, LUMA)).collect();
        let base = shading(&lum, &w, c.gw, c.gh, sigma(EVEN_SHADING));
        let t: Vec<f32> = lum.iter().zip(&base).map(|(l, b)| l - b).collect();
        let form = form(&t, &w, c.gw, c.gh, sigma(FORM_TENSOR));
        let keep = texture(c, &w, sigma(TEXTURE_SCALE));
        for (i, k) in corr.iter_mut().enumerate() {
            let (Some(t), Some(f), Some(wd)) = (t.get(i), fine.get(i), wide.get(i)) else { continue };
            let z = inp.even_zone.get(i).copied().unwrap_or(0.0);
            let share = if *t < 0.0 { EVEN_DARK } else { EVEN_LIGHT * (-(t / EVEN_HIGHLIGHT).powi(2)).exp() };
            let share = share * ((EVEN_KEEP[1] - t.abs()) / (EVEN_KEEP[1] - EVEN_KEEP[0])).clamp(0.0, 1.0);
            let structure = form.get(i).copied().unwrap_or(1.0).max(z);
            let tl = t * share * (1.0 - structure);
            // colour against the skin's own, without the shading's colour
            let d: V3 = std::array::from_fn(|k| f[k] - wd[k]);
            let dl = dot(d, LUMA);
            let r: V3 = std::array::from_fn(|k| d[k] - dl * sd[k]);
            let rn = dot(r, r).sqrt();
            let rs = EVEN_COLOUR_SHARE * (-(rn / EVEN_COLOUR_KEEP).powi(2)).exp() * (1.0 - z);
            let g = inp.even * keep.get(i).copied().unwrap_or(0.0);
            *k = std::array::from_fn(|ch| -g * (r[ch] * rs + tl * sd[ch]));
        }
    }
    if inp.red > 0.0 {
        redness(c, &w, &fine, &corr.clone(), sd, inp, sigma(RED_AROUND), &mut corr);
    }
    // the gains, scaled so each cell's (linear) luminance changes by exactly the luminance part of its change:
    // colour moves keep the brightness
    for ((g, k), f) in out.data.iter_mut().zip(&corr).zip(&fine) {
        let k: V3 = k.map(|v| v.clamp(-MAX_LOG, MAX_LOG));
        let c: V3 = f.map(f32::exp);
        let v: V3 = std::array::from_fn(|ch| k[ch].exp());
        let (before, after) = (dot(c, LUMA), dot(std::array::from_fn(|ch| c[ch] * v[ch]), LUMA));
        let keep = if before > 1e-9 && after > 1e-9 { before * dot(k, LUMA).exp() / after } else { 1.0 };
        let v: V3 = v.map(|x| x * keep);
        if v.iter().all(|x| x.is_finite()) {
            *g = v;
        }
    }
    out
}

/// Redness on top of Even Tone's change `even` (log, per cell), added to `corr`.
#[allow(clippy::too_many_arguments)]
fn redness(c: &Cells, w: &[f32], fine: &[V3], even: &[V3], sd: V3, inp: &Inputs<'_>, around: f32, corr: &mut [V3]) {
    // the colour after Even Tone, less part of the shading's colour (it keeps its brightness: v · LUMA = 1)
    let v: V3 = std::array::from_fn(|k| RED_SHADING + (1.0 - RED_SHADING) * sd[k]);
    let q: Vec<V3> = fine
        .iter()
        .zip(even)
        .map(|(f, e)| {
            let a: V3 = std::array::from_fn(|k| f[k] + e[k]);
            let l = dot(a, LUMA);
            std::array::from_fn(|k| a[k] - l * v[k])
        })
        .collect();
    let redness = |q: &V3| q[0] - q[1];
    // the face's calm skin
    let mut ranked: Vec<(f32, usize)> =
        q.iter().enumerate().filter(|(i, _)| w.get(*i).is_some_and(|w| *w > 0.9)).map(|(i, q)| (redness(q), i)).collect();
    if ranked.is_empty() {
        return;
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let at = |p: f32| ((p / 100.0 * (ranked.len() - 1) as f32).round() as usize).min(ranked.len() - 1);
    let calm = ranked.get(at(RED_REFERENCE[0])..=at(RED_REFERENCE[1])).unwrap_or(&[]);
    let mut face = [0.0f32; 3];
    for (_, i) in calm {
        if let Some(q) = q.get(*i) {
            face = std::array::from_fn(|k| face[k] + q[k] / calm.len() as f32);
        }
    }
    let skin = blur3(&q, w, c.gw, c.gh, around);
    for (i, k) in corr.iter_mut().enumerate() {
        let (Some(q), Some(s)) = (q.get(i), skin.get(i)) else { continue };
        let open = inp.red * (1.0 - inp.red_zone.get(i).copied().unwrap_or(0.0));
        // a red spot: all the way to the skin around it
        let g = open * smoothstep(0.0, RED_FULL, redness(q) - redness(s));
        let near: V3 = std::array::from_fn(|ch| q[ch] - g * (q[ch] - s[ch]));
        // red skin: part of the way to the face's calm skin
        let g = open * RED_MOST * smoothstep(0.0, RED_FULL, redness(&near) - redness(&face));
        let to: V3 = std::array::from_fn(|ch| near[ch] - g * (near[ch] - face[ch]));
        *k = std::array::from_fn(|ch| k[ch] + to[ch] - q[ch]);
    }
}

/// `v` blurred by `sigma` cells with the cells' weights `w` (a cell without skin borrows from its neighbours). A
/// wide blur runs on cells up to four times larger ([`coarser`]) and is read back bilinearly: it is smooth at that
/// size, and a slider move stays within the draft's budget.
fn blur3(v: &[V3], w: &[f32], gw: usize, gh: usize, sigma: f32) -> Vec<V3> {
    let f = coarser(sigma);
    let img = Image::<[f32; 4]> { width: gw, height: gh, data: v.iter().zip(w).map(|(v, w)| [v[0] * w, v[1] * w, v[2] * w, *w]).collect() };
    let small = gaussian(&reduce(&img, f), sigma / f as f32);
    let norm = |s: [f32; 4]| if s[3] > 1e-6 { [s[0] / s[3], s[1] / s[3], s[2] / s[3]] } else { [0.0; 3] };
    if f == 1 {
        return small.data.into_iter().map(norm).collect();
    }
    (0..gw * gh).map(|i| norm(enlarged(&small, f, i % gw.max(1), i / gw.max(1)))).collect()
}

/// How many cells across one coarse cell a blur of `sigma` cells runs on: at least two coarse cells per sigma.
fn coarser(sigma: f32) -> usize {
    if sigma.is_finite() { ((sigma / 2.0).floor() as usize).clamp(1, 4) } else { 1 }
}

/// `g` with each `f × f` block of cells summed into one.
fn reduce(g: &Image<[f32; 4]>, f: usize) -> Image<[f32; 4]> {
    if f <= 1 {
        return g.clone();
    }
    let (w, h) = (g.width.div_ceil(f).max(1), g.height.div_ceil(f).max(1));
    let mut out = Image::<[f32; 4]>::new(w, h);
    for (i, v) in g.data.iter().enumerate() {
        let (x, y) = (i % g.width.max(1) / f, i / g.width.max(1) / f);
        if let Some(o) = out.data.get_mut(y * w + x) {
            *o = std::array::from_fn(|k| o[k] + v[k]);
        }
    }
    out
}

/// Coarse cells `g` (each `f` fine cells across) read at fine cell (`x`, `y`), bilinear between coarse centres.
fn enlarged(g: &Image<[f32; 4]>, f: usize, x: usize, y: usize) -> [f32; 4] {
    let axis = |p: usize, n: usize| {
        let c = ((p as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, n.saturating_sub(1) as f32);
        let i = c.floor() as usize;
        (i, (i + 1).min(n.saturating_sub(1)), c - i as f32)
    };
    let ((x0, x1, tx), (y0, y1, ty)) = (axis(x, g.width), axis(y, g.height));
    let v = |x: usize, y: usize| g.data.get(y * g.width + x).copied().unwrap_or([0.0; 4]);
    let w = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
    let p = [v(x0, y0), v(x1, y0), v(x0, y1), v(x1, y1)];
    std::array::from_fn(|k| p.iter().zip(w).map(|(p, w)| p[k] * w).sum())
}

/// `v` (one value per cell) blurred by `sigma` cells with the cells' weights `w`.
fn blur1(v: &[f32], w: &[f32], gw: usize, gh: usize, sigma: f32) -> Vec<f32> {
    let img = Image::<[f32; 4]> { width: gw, height: gh, data: v.iter().zip(w).map(|(v, w)| [v * w, *w, 0.0, 0.0]).collect() };
    gaussian(&img, sigma).data.iter().map(|s| if s[1] > 1e-6 { s[0] / s[1] } else { 0.0 }).collect()
}

/// How this photo's skin colour changes with its brightness: the regression of the colour difference between two
/// scales on its luminance, scaled so that its luminance is 1. Without a usable answer, grey (1, 1, 1).
fn shading_direction(near: &[V3], far: &[V3], w: &[f32]) -> V3 {
    let (mut num, mut den) = ([0.0f64; 3], 0.0f64);
    for ((a, b), w) in near.iter().zip(far).zip(w) {
        if *w < 0.9 {
            continue;
        }
        let d: V3 = std::array::from_fn(|k| a[k] - b[k]);
        let l = f64::from(dot(d, LUMA));
        for (n, dk) in num.iter_mut().zip(d) {
            *n += f64::from(dk) * l;
        }
        den += l * l;
    }
    if den < 1e-9 {
        return [1.0; 3];
    }
    let s: V3 = num.map(|n| (n / den) as f32);
    let l = dot(s, LUMA);
    let s: V3 = s.map(|v| v / l);
    if l.is_finite() && l > 0.1 && s.iter().all(|v| v.is_finite() && (0.1..=4.0).contains(v)) { s } else { [1.0; 3] }
}

/// The shading: a plane fitted to `lum` round each cell over `sigma` cells, robustly. It is fitted on cells up to
/// twice as large ([`coarser`]) and read back bilinearly.
fn shading(lum: &[f32], w: &[f32], gw: usize, gh: usize, sigma: f32) -> Vec<f32> {
    let f = coarser(sigma).min(2);
    if f == 1 {
        return shading_on(lum, w, gw, gh, sigma);
    }
    let img = Image::<[f32; 4]> { width: gw, height: gh, data: lum.iter().zip(w).map(|(l, w)| [l * w, *w, 0.0, 0.0]).collect() };
    let small = reduce(&img, f);
    let cw: Vec<f32> = small.data.iter().map(|s| s[1] / (f * f) as f32).collect();
    let cl: Vec<f32> = small.data.iter().map(|s| if s[1] > 1e-9 { s[0] / s[1] } else { 0.0 }).collect();
    let base = shading_on(&cl, &cw, small.width, small.height, sigma / f as f32);
    let up = Image::<[f32; 4]> { width: small.width, height: small.height, data: base.iter().map(|b| [*b, 0.0, 0.0, 0.0]).collect() };
    (0..gw * gh).map(|i| enlarged(&up, f, i % gw.max(1), i / gw.max(1))[0]).collect()
}

/// [`shading`] on cells as they are.
fn shading_on(lum: &[f32], w: &[f32], gw: usize, gh: usize, sigma: f32) -> Vec<f32> {
    let n = gw * gh;
    // coordinates from the grid's middle, so the moments stay small
    let (cx, cy) = (gw as f32 / 2.0, gh as f32 / 2.0);
    let xy = |i: usize| ((i % gw.max(1)) as f32 - cx, (i / gw.max(1)) as f32 - cy);
    let mut rw = vec![1.0f32; n];
    let mut base = lum.to_vec();
    let reg = 0.05 * sigma * sigma;
    for pass in 0..3 {
        let mut a = Image::<[f32; 4]>::new(gw, gh);
        let mut b = Image::<[f32; 4]>::new(gw, gh);
        for i in 0..n {
            let (x, y) = xy(i);
            let (ww, f) = (w.get(i).copied().unwrap_or(0.0) * rw.get(i).copied().unwrap_or(0.0), lum.get(i).copied().unwrap_or(0.0));
            if let (Some(a), Some(b)) = (a.data.get_mut(i), b.data.get_mut(i)) {
                *a = [ww, ww * x, ww * y, ww * f];
                *b = [ww * x * x, ww * x * y, ww * y * y, 0.0];
            }
        }
        let mut c = Image::<[f32; 4]>::new(gw, gh);
        for (i, cv) in c.data.iter_mut().enumerate() {
            let (x, y) = xy(i);
            let wf = a.data.get(i).map_or(0.0, |a| a[3]);
            *cv = [wf * x, wf * y, 0.0, 0.0];
        }
        let (a, b, c) = (gaussian(&a, sigma), gaussian(&b, sigma), gaussian(&c, sigma));
        for (i, out) in base.iter_mut().enumerate() {
            let (Some(a), Some(b), Some(c)) = (a.data.get(i), b.data.get(i), c.data.get(i)) else { continue };
            if a[0] <= 1e-6 {
                continue;
            }
            let (s0, (x, y)) = (a[0], xy(i));
            let (mx, my, mf) = (a[1] / s0, a[2] / s0, a[3] / s0);
            let sxx = b[0] / s0 - mx * mx + reg;
            let sxy = b[1] / s0 - mx * my;
            let syy = b[2] / s0 - my * my + reg;
            let sxf = c[0] / s0 - mx * mf;
            let syf = c[1] / s0 - my * mf;
            let det = sxx * syy - sxy * sxy;
            let v = if det > 1e-9 {
                let (gx, gy) = ((syy * sxf - sxy * syf) / det, (sxx * syf - sxy * sxf) / det);
                mf + gx * (x - mx) + gy * (y - my)
            } else {
                mf
            };
            if v.is_finite() {
                *out = v;
            }
        }
        if pass < 2 {
            for ((r, l), b) in rw.iter_mut().zip(lum).zip(&base) {
                let u = ((l - b) / ROBUST).abs();
                *r = if u < 1.0 { (1.0 - u * u).powi(2) } else { 0.0 };
            }
        }
    }
    base
}

/// How much each cell's difference `t` is long and narrow (0 round … 1 a line or an edge): the coherence of its
/// gradients gathered over `sigma` cells.
fn form(t: &[f32], w: &[f32], gw: usize, gh: usize, sigma: f32) -> Vec<f32> {
    let at = |x: usize, y: usize| t.get(y * gw + x).copied().unwrap_or(0.0);
    let mut tensor = Image::<[f32; 4]>::new(gw, gh);
    for (i, j) in tensor.data.iter_mut().enumerate() {
        let (x, y) = (i % gw.max(1), i / gw.max(1));
        let gx = (at((x + 1).min(gw.saturating_sub(1)), y) - at(x.saturating_sub(1), y)) * 0.5;
        let gy = (at(x, (y + 1).min(gh.saturating_sub(1))) - at(x, y.saturating_sub(1))) * 0.5;
        let ww = w.get(i).copied().unwrap_or(0.0);
        *j = [ww * gx * gx, ww * gx * gy, ww * gy * gy, ww];
    }
    gaussian(&tensor, sigma)
        .data
        .iter()
        .map(|s| {
            let tr = s[0] + s[2];
            if tr <= 1e-12 {
                return 0.0;
            }
            let coh = ((s[0] - s[2]).powi(2) + 4.0 * s[1] * s[1]) / (tr * tr);
            ((coh - FORM_COHERENCE[0]) / (FORM_COHERENCE[1] - FORM_COHERENCE[0])).clamp(0.0, 1.0)
        })
        .collect()
}

/// How much of the correction each cell takes (1 everywhere but dense fine texture: stubble, a beard's edge), from
/// the luminance variance inside each cell against the face's typical one.
fn texture(c: &Cells, w: &[f32], sigma: f32) -> Vec<f32> {
    let cv: Vec<f32> = c
        .sums
        .iter()
        .map(|s| {
            if s[3] <= 1e-9 || s[4] <= 1e-12 {
                return 0.0;
            }
            let (m, m2) = (s[4] / s[3], s[5] / s[3]);
            (m2 / (m * m) - 1.0).clamp(0.0, 4.0)
        })
        .collect();
    let cv = blur1(&cv, w, c.gw, c.gh, sigma);
    let mut typical: Vec<f32> = cv.iter().zip(w).filter(|(_, w)| **w > 0.9).map(|(v, _)| *v).collect();
    let mid = typical.len() / 2;
    let typical = if typical.is_empty() { 0.0 } else { *typical.select_nth_unstable_by(mid, f32::total_cmp).1 };
    if typical.is_nan() || typical <= 1e-6 {
        return vec![1.0; cv.len()];
    }
    cv.iter().map(|v| (1.0 - (v / typical - TEXTURE_KEEP[0]) / (TEXTURE_KEEP[1] - TEXTURE_KEEP[0])).clamp(0.0, 1.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shading_follows_a_ramp_and_ignores_a_blotch() {
        let (gw, gh) = (60, 50);
        let ramp = |i: usize| 0.01 * (i % gw) as f32 - 0.006 * (i / gw) as f32;
        let blotch = |i: usize| {
            let (x, y) = ((i % gw) as f32 - 30.0, (i / gw) as f32 - 25.0);
            if x * x + y * y < 9.0 { -0.3 } else { 0.0 }
        };
        let lum: Vec<f32> = (0..gw * gh).map(|i| ramp(i) + blotch(i)).collect();
        let w = vec![1.0; gw * gh];
        let base = shading_on(&lum, &w, gw, gh, 6.0);
        let coarse = shading(&lum, &w, gw, gh, 6.0);
        // away from the edges the plane is the ramp, the blotch left out of it
        for i in [10 * gw + 10, 25 * gw + 30, 40 * gw + 45] {
            assert!((base[i] - ramp(i)).abs() < 0.02, "{i}: {} vs {}", base[i], ramp(i));
            assert!((coarse[i] - ramp(i)).abs() < 0.03, "{i}: {} vs {}", coarse[i], ramp(i));
        }
    }

    #[test]
    fn a_line_is_form_and_a_disc_is_not() {
        let (gw, gh): (usize, usize) = (60, 60);
        let line: Vec<f32> = (0..gw * gh).map(|i| if (i % gw).abs_diff(20) < 2 { -0.3 } else { 0.0 }).collect();
        let disc: Vec<f32> = (0..gw * gh)
            .map(|i| {
                let (x, y) = ((i % gw) as f32 - 40.0, (i / gw) as f32 - 30.0);
                -0.3 * (-(x * x + y * y) / 18.0).exp()
            })
            .collect();
        let w = vec![1.0; gw * gh];
        assert!(form(&line, &w, gw, gh, 2.4)[30 * gw + 21] > 0.9);
        assert!(form(&disc, &w, gw, gh, 2.4)[30 * gw + 40] < 0.2);
    }

    #[test]
    fn the_shading_direction_of_grey_light_is_grey_and_nonsense_falls_back() {
        let near = vec![[0.1, 0.1, 0.1], [-0.2, -0.2, -0.2]];
        let far = vec![[0.0; 3]; 2];
        let s = shading_direction(&near, &far, &[1.0, 1.0]);
        assert!(s.iter().all(|v| (v - 1.0).abs() < 1e-5), "{s:?}");
        assert_eq!(shading_direction(&far, &far, &[1.0, 1.0]), [1.0; 3]);
        assert_eq!(shading_direction(&[[f32::NAN; 3]], &[[0.0; 3]], &[1.0]), [1.0; 3]);
    }
}
