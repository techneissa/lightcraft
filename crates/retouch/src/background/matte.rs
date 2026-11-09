//! The person's matte and the two colours of its soft edge.
//!
//! The matte is the largest Entire Person mask of every face at each pixel (the people parse's, with the hair edge
//! Settings ▸ Faces ▸ Hair Edges chooses, `fork/hair-edges.md`), with the small gaps of the hair filled
//! ([`super::holes`]). At its soft edge a pixel is a mix, `I = a·F + (1 − a)·U`, of the person's colour `F` and the
//! background behind it `U`. Both are estimated on the analysis image by blur-fusion foreground estimation (Forte and
//! Pitié 2021), as a mask's Decontaminate does (`fork/decontaminate.md`): box means of the person's and the
//! background's colours in a wide and a narrow window (fractions of the image's long side). A render reads them
//! bilinearly and corrects them so that they mix back to its own pixel exactly ([`mix_back`]). The person is then
//! drawn over a new background with `F`, so no old background colour stays between strands.

use std::sync::Arc;

use lightcraft_raster::{Plane, Rgb32f, par_rows};

use crate::{Part, PartMask, Scene};

/// Below and above these values of the matte a pixel is wholly background or wholly person.
pub(crate) const EDGE: (f32, f32) = (0.002, 0.998);
/// The wide and narrow windows, fractions of the image's long side (as Decontaminate's).
pub(crate) const WIDE: f64 = 0.045;
pub(crate) const NARROW: f64 = 0.003;

/// Every face's Entire Person mask with the small gaps of its hair filled (kept: [`super::holes::filled_kept`]);
/// `None` when no face has one with a person in it.
pub(crate) fn masks(scene: &Scene<'_>) -> Option<Vec<Arc<PartMask>>> {
    let out: Vec<Arc<PartMask>> = scene
        .faces
        .iter()
        .filter_map(|f| {
            let m = f.face.mask(Part::EntirePerson)?;
            let ok =
                m.area.is_valid() && m.plane.data.len() == m.plane.width.saturating_mul(m.plane.height) && m.plane.data.iter().any(|v| *v > EDGE.0);
            ok.then(|| super::holes::filled_kept(m, f.face.mask(Part::Hair)))
        })
        .collect();
    (!out.is_empty()).then_some(out)
}

/// The matte at normalised point (`nx`, `ny`).
pub(crate) fn at(masks: &[Arc<PartMask>], nx: f64, ny: f64) -> f32 {
    masks.iter().map(|m| m.sample(nx, ny)).fold(0.0, f32::max)
}

/// The matte over a `w × h` image whose pixel centre (`x + 0.5`, `y + 0.5`) is at normalised `to_norm(x + 0.5, y + 0.5)`.
pub(crate) fn plane(masks: &[Arc<PartMask>], w: usize, h: usize, to_norm: &(impl Fn(f64, f64) -> (f64, f64) + Sync)) -> Plane {
    let mut a = Plane::new(w, h);
    par_rows(&mut a.data, w.max(1), |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let (nx, ny) = to_norm(x as f64 + 0.5, y as f64 + 0.5);
            *v = at(masks, nx, ny);
        }
    });
    a
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
}

/// Colours over a rectangle.
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

/// Box means of radius `r` of `v` (read over `inp`) at every pixel of `out`, each window clamped to `inp`.
fn box_mean(v: &(impl Fn(usize, usize) -> f32 + Sync), inp: Rect, out: Rect, r: usize) -> Vec<f32> {
    let (iw, ow) = (inp.w(), out.w());
    let mut rows = vec![0f64; inp.h() * ow];
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
    for ry in 1..inp.h() {
        let (done, rest) = rows.split_at_mut(ry * ow);
        if let (Some(prev), Some(row)) = (done.get((ry - 1) * ow..), rest.get_mut(..ow)) {
            row.iter_mut().zip(prev).for_each(|(v, p)| *v += *p);
        }
    }
    let mut o = vec![0f32; out.h() * ow];
    par_rows(&mut o, ow.max(1), |oy, row| {
        let y = out.y0 + oy;
        let (a, b) = (y.saturating_sub(r).max(inp.y0), y.saturating_add(r + 1).min(inp.y1));
        let n = b.saturating_sub(a).max(1) as f64;
        let sum_to = |end: usize, ox: usize| if end <= inp.y0 { 0.0 } else { rows.get((end - inp.y0 - 1) * ow + ox).copied().unwrap_or(0.0) };
        for (ox, v) in row.iter_mut().enumerate() {
            *v = ((sum_to(b, ox) - sum_to(a, ox)) / n) as f32;
        }
    });
    o
}

fn sane(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// One blur-fusion pass with window `r`, reading `inp` and estimating the colours at `out`.
fn pass(img: &Rgb32f, alpha: &Plane, prev: Option<(&Region, &Region)>, inp: Rect, out: Rect, r: usize) -> (Region, Region) {
    const EPS: f32 = 1e-5;
    let w = img.width;
    let photo = |x: usize, y: usize| img.data.get(y * w + x).copied().unwrap_or([0.0; 3]);
    let f_at = |x: usize, y: usize| prev.map_or_else(|| photo(x, y), |(f, _)| f.get(x, y));
    let u_at = |x: usize, y: usize| prev.map_or_else(|| photo(x, y), |(_, u)| u.get(x, y));
    let a = |x: usize, y: usize| sane(alpha.data.get(y * w + x).copied().unwrap_or(0.0));
    let ba = box_mean(&a, inp, out, r);
    let bf: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&|x, y| f_at(x, y)[c] * a(x, y), inp, out, r));
    let bu: [Vec<f32>; 3] = std::array::from_fn(|c| box_mean(&|x, y| u_at(x, y)[c] * (1.0 - a(x, y)), inp, out, r));
    let n = out.w() * out.h();
    let (mut f, mut u) = (vec![[0.0f32; 3]; n], vec![[0.0f32; 3]; n]);
    for (i, (fp, up)) in f.iter_mut().zip(u.iter_mut()).enumerate() {
        let (x, y) = (out.x0 + i % out.w().max(1), out.y0 + i / out.w().max(1));
        let (al, m, px) = (a(x, y), ba.get(i).copied().unwrap_or(0.0), photo(x, y));
        for c in 0..3 {
            let ff = bf[c].get(i).copied().unwrap_or(0.0) / (m + EPS);
            let fu = bu[c].get(i).copied().unwrap_or(0.0) / (1.0 - m + EPS);
            let res = px[c] - al * ff - (1.0 - al) * fu;
            fp[c] = (ff + al * res).max(0.0);
            up[c] = (fu + (1.0 - al) * res).max(0.0);
        }
    }
    (Region { at: out, data: f }, Region { at: out, data: u })
}

/// The person and background colours of every pixel of `img` under `alpha` (same size), with windows for an image
/// whose long side is `long_px` pixels: estimated at the edge, the pixel itself where it is wholly one or the other,
/// and the windows' extrapolation of the other there (so a render between two pixels reads sensible colours).
pub(crate) fn colours(img: &Rgb32f, alpha: &Plane, long_px: f64) -> (Rgb32f, Rgb32f) {
    let (w, h) = (img.width, img.height);
    if alpha.width != w || alpha.height != h || w == 0 || h == 0 {
        return (img.clone(), img.clone());
    }
    let radius = |f: f64| ((f * long_px).round().max(1.0) as usize).min(w.max(h));
    let all = Rect { x0: 0, y0: 0, x1: w, y1: h };
    let (f1, u1) = pass(img, alpha, None, all, all, radius(WIDE));
    let (f, u) = pass(img, alpha, Some((&f1, &u1)), all, all, radius(NARROW));
    drop((f1, u1));
    let (mut fo, mut uo) = (Rgb32f::new(w, h), Rgb32f::new(w, h));
    for (i, ((fp, up), px)) in fo.data.iter_mut().zip(uo.data.iter_mut()).zip(&img.data).enumerate() {
        let a = sane(alpha.data.get(i).copied().unwrap_or(0.0));
        let (fe, ue) = (f.data.get(i).copied().unwrap_or(*px), u.data.get(i).copied().unwrap_or(*px));
        (*fp, *up) = if a >= EDGE.1 {
            (*px, ue)
        } else if a <= EDGE.0 {
            (fe, *px)
        } else {
            mix_back(a, *px, fe, ue)
        };
    }
    (fo, uo)
}

/// Person and background colours `f`, `u` corrected so that `a·F + (1 − a)·U` is pixel `px`: the smallest change,
/// shared by the two in proportion to their weights.
pub(crate) fn mix_back(a: f32, px: [f32; 3], f: [f32; 3], u: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let a = sane(a);
    let norm = (a * a + (1.0 - a) * (1.0 - a)).max(1e-6);
    let (mut fo, mut uo) = (f, u);
    for c in 0..3 {
        let r = (px[c] - (a * f[c] + (1.0 - a) * u[c])) / norm;
        fo[c] = (f[c] + a * r).max(0.0);
        uo[c] = (u[c] + (1.0 - a) * r).max(0.0);
    }
    (fo, uo)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A strand half over a blue sky: the person's colour comes out without the blue, the background's without the
    /// strand, and the two mix back to the pixel.
    #[test]
    fn an_edge_pixel_splits_into_the_person_and_what_is_behind_and_mixes_back() {
        let (w, h) = (120, 80);
        let hair = [0.30, 0.18, 0.08];
        let sky = [0.10, 0.30, 0.80];
        let alpha = Plane::from_fn(w, h, |x, _| ((x as f32 - 50.0) / 20.0).clamp(0.0, 1.0));
        let img = Rgb32f::from_fn(w, h, |x, y| {
            let a = alpha.get(x, y);
            std::array::from_fn(|c| a * hair[c] + (1.0 - a) * sky[c])
        });
        let (f, u) = colours(&img, &alpha, 1000.0);
        let mut edges = 0;
        for x in 0..w {
            let i = 40 * w + x;
            let (a, px) = (alpha.data[i], img.data[i]);
            for c in 0..3 {
                assert!((a * f.data[i][c] + (1.0 - a) * u.data[i][c] - px[c]).abs() < 1e-4, "mixes back at {x}");
            }
            if (0.3..0.7).contains(&a) {
                edges += 1;
                assert!((f.data[i][2] - hair[2]).abs() < 0.1, "no blue in the strand: {:?} at {a}", f.data[i]);
                assert!((u.data[i][0] - sky[0]).abs() < 0.1, "no hair in the sky: {:?}", u.data[i]);
            }
        }
        assert!(edges > 3);
        let (fo, uo) = mix_back(0.5, [0.4; 3], [0.0; 3], [0.0; 3]);
        assert!((0.5 * fo[0] + 0.5 * uo[0] - 0.4).abs() < 1e-6);
    }
}
