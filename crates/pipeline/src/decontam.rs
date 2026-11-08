//! Fork feature (`fork/decontaminate.md`, issue #115): colour decontamination at a mask's soft edge.
//!
//! A pixel a mask half-selects (a strand of hair against the sky) is a mix of two colours: `I = a·S + (1 − a)·U`, `S`
//! the colour of what the mask selects (the hair), `U` the colour of what it leaves (the sky). Editing the pixel
//! as one colour keeps the sky's blue in the strand whatever is done to the background. [`estimate`] recovers `S` and
//! `U` from the photo and the mask's alpha by blur-fusion foreground estimation (Forte and Pitié, "Approximate Fast
//! Foreground Colour Estimation", ICIP 2021): local averages of the selected and the unselected colours, corrected so
//! that they mix back to the pixel exactly. Two passes: a wide window for colours far from the edge, a narrow one for
//! the edge itself. Windows are fractions of the frame's long side, so a preview and an export agree.
//!
//! No model, linear time (box blurs by running sums).

use lightcraft_raster::{Plane, Rgb32f};

/// A decontaminated mask's edge in one render: its index among the render's masks' ids, the Amount (0..1), and every
/// pixel's selected and unselected colour (scene linear, the render's size).
pub struct Split {
    pub id: u32,
    pub amount: f32,
    pub selected: Rgb32f,
    pub unselected: Rgb32f,
}

/// Below and above these alphas a pixel is wholly in one layer: it is drawn as before.
const EDGE: (f32, f32) = (0.002, 0.998);

/// The splits of the render's visible masks whose Decontaminate amount is above 0 (`masks` are the settings' masks;
/// the render's evaluated masks give the alphas).
pub(crate) fn splits(p: &crate::Prepared, masks: &[lightcraft_develop::Mask]) -> Vec<Split> {
    masks
        .iter()
        .filter(|m| m.decontaminate > 0.0 && m.decontaminate.is_finite())
        .filter_map(|m| {
            let e = p.masks.iter().find(|e| e.id == m.id)?;
            if e.alpha.is_zero() {
                return None;
            }
            let (selected, unselected) = estimate(&p.img, &e.alpha.to_plane(), p.px_per_long);
            Some(Split { id: m.id, amount: (m.decontaminate / 100.0).clamp(0.0, 1.0) as f32, selected, unselected })
        })
        .collect()
}

/// For each split, the index of its mask among the render's masks.
pub(crate) fn index(p: &crate::Prepared) -> Vec<(usize, usize)> {
    p.split.iter().enumerate().filter_map(|(si, s)| p.masks.iter().position(|e| e.id == s.id).map(|k| (k, si))).collect()
}

/// At pixel `(x, y)` (index `i`): the first decontaminated mask whose edge it is on, its alpha there, its Amount, and
/// the pixel's selected and unselected colours.
pub(crate) fn split_at(p: &crate::Prepared, idx: &[(usize, usize)], x: usize, y: usize, i: usize) -> Option<(usize, f32, f32, [f32; 3], [f32; 3])> {
    idx.iter().find_map(|&(k, si)| {
        let a = p.masks.get(k)?.alpha.at(x, y);
        let s = p.split.get(si)?;
        (a > EDGE.0 && a < EDGE.1).then(|| Some((k, a, s.amount, *s.selected.data.get(i)?, *s.unselected.data.get(i)?))).flatten()
    })
}

/// The wide and narrow windows, as fractions of the long side (the paper's 90 and 6 px at about 2000 px).
pub const WIDE: f64 = 0.045;
pub const NARROW: f64 = 0.003;

/// A box mean of radius `r` over a `w × h` plane, the window clamped at the borders (each pixel divided by how many
/// values its window holds).
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    let mut out = vec![0.0f32; w * h];
    // rows
    for y in 0..h {
        let row = v.get(y * w..(y + 1) * w).unwrap_or(&[]);
        let mut prefix = Vec::with_capacity(w + 1);
        prefix.push(0.0f64);
        for x in row {
            prefix.push(prefix.last().copied().unwrap_or(0.0) + f64::from(*x));
        }
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = prefix.get(b).copied().unwrap_or(0.0) - prefix.get(a).copied().unwrap_or(0.0);
            if let Some(t) = tmp.get_mut(y * w + x) {
                *t = (s / (b - a).max(1) as f64) as f32;
            }
        }
    }
    // columns
    let mut prefix = vec![0.0f64; h + 1];
    for x in 0..w {
        for y in 0..h {
            let p = prefix.get(y).copied().unwrap_or(0.0) + f64::from(tmp.get(y * w + x).copied().unwrap_or(0.0));
            if let Some(q) = prefix.get_mut(y + 1) {
                *q = p;
            }
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
            let s = prefix.get(b).copied().unwrap_or(0.0) - prefix.get(a).copied().unwrap_or(0.0);
            if let Some(o) = out.get_mut(y * w + x) {
                *o = (s / (b - a).max(1) as f64) as f32;
            }
        }
    }
    out
}

/// One blur-fusion pass with window `r`: `s` and `u` (the selected and unselected colours so far) refined.
fn pass(img: &Rgb32f, a: &[f32], s: &mut Rgb32f, u: &mut Rgb32f, r: usize) {
    const EPS: f32 = 1e-5;
    let (w, h) = (img.width, img.height);
    let ba = box_mean(a, w, h, r);
    let mut bs = [Vec::new(), Vec::new(), Vec::new()];
    let mut bu = [Vec::new(), Vec::new(), Vec::new()];
    for c in 0..3 {
        let sa: Vec<f32> = s.data.iter().zip(a).map(|(p, a)| p[c] * a).collect();
        let ua: Vec<f32> = u.data.iter().zip(a).map(|(p, a)| p[c] * (1.0 - a)).collect();
        bs[c] = box_mean(&sa, w, h, r);
        bu[c] = box_mean(&ua, w, h, r);
    }
    for i in 0..w * h {
        let (Some(&al), Some(&m)) = (a.get(i), ba.get(i)) else { continue };
        let i_px = img.data.get(i).copied().unwrap_or([0.0; 3]);
        let mut sp = [0.0f32; 3];
        let mut up = [0.0f32; 3];
        for c in 0..3 {
            let fs = bs[c].get(i).copied().unwrap_or(0.0) / (m + EPS);
            let fu = bu[c].get(i).copied().unwrap_or(0.0) / (1.0 - m + EPS);
            let res = i_px[c] - al * fs - (1.0 - al) * fu;
            sp[c] = (fs + al * res).max(0.0);
            up[c] = (fu + (1.0 - al) * res).max(0.0);
        }
        if let Some(p) = s.data.get_mut(i) {
            *p = sp;
        }
        if let Some(p) = u.data.get_mut(i) {
            *p = up;
        }
    }
}

/// The selected and unselected colours of every pixel of `img` under `alpha` (same size; values 0..1), with windows
/// for a frame whose long side is `long_px` output pixels. Pixels the mask selects wholly or not at all keep their own
/// colour in the layer they belong to. A size mismatch gives the photo back for both.
pub fn estimate(img: &Rgb32f, alpha: &Plane, long_px: f64) -> (Rgb32f, Rgb32f) {
    let (w, h) = (img.width, img.height);
    let mut s = img.clone();
    let mut u = img.clone();
    if alpha.width != w || alpha.height != h || w == 0 || h == 0 {
        return (s, u);
    }
    let a: Vec<f32> = alpha.data.iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }).collect();
    let radius = |f: f64| ((f * long_px).round() as usize).clamp(1, w.max(h));
    pass(img, &a, &mut s, &mut u, radius(WIDE));
    pass(img, &a, &mut s, &mut u, radius(NARROW));
    // the update mixes back to the pixel only approximately: the remainder is shared by the two colours in proportion
    // to their weights (the smallest change that makes `a·S + (1 − a)·U` the pixel), so an edge the masks leave alone
    // renders as before
    for ((sp, up), (i_px, al)) in s.data.iter_mut().zip(u.data.iter_mut()).zip(img.data.iter().zip(&a)) {
        let norm = al * al + (1.0 - al) * (1.0 - al);
        for c in 0..3 {
            let r = (i_px[c] - (al * sp[c] + (1.0 - al) * up[c])) / norm;
            sp[c] = (sp[c] + al * r).max(0.0);
            up[c] = (up[c] + (1.0 - al) * r).max(0.0);
        }
    }
    (s, u)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Red hair strands over a blue sky: a column of pixels each `k`/10 hair.
    fn strands(w: usize, h: usize) -> (Rgb32f, Plane) {
        let hair = [0.6f32, 0.25, 0.1];
        let sky = [0.1f32, 0.3, 0.8];
        let alpha_at = |x: usize| {
            if x < w * 45 / 100 {
                1.0
            } else if x < w * 55 / 100 {
                ((x * 7) % 10) as f32 / 10.0
            } else {
                0.0
            }
        };
        let img = Rgb32f::from_fn(w, h, |x, _| {
            let a = alpha_at(x);
            std::array::from_fn(|c| a * hair[c] + (1.0 - a) * sky[c])
        });
        let mut alpha = Plane::new(w, h);
        for (i, v) in alpha.data.iter_mut().enumerate() {
            *v = alpha_at(i % w);
        }
        (img, alpha)
    }

    #[test]
    fn a_mixed_strand_splits_into_hair_and_sky() {
        let (w, h) = (300, 40);
        let (img, alpha) = strands(w, h);
        let (s, u) = estimate(&img, &alpha, w as f64);
        for x in (w * 45 / 100..w * 55 / 100).filter(|x| (x * 7) % 10 >= 3 && (x * 7) % 10 <= 7) {
            let (sp, up) = (s.get(x, h / 2), u.get(x, h / 2));
            // (an approximation: at least 40 % of the sky's blue in the pixel leaves the strand's colour)
            let i = img.get(x, h / 2);
            assert!(i[2] - sp[2] >= 0.4 * (i[2] - 0.1) && sp[0] > i[0], "x {x}: the strand keeps the sky's blue: {sp:?} from {i:?}");
            assert!(up[2] > 0.55 && up[2] > 2.0 * up[0], "x {x}: the sky behind it stays blue: {up:?}");
            // they mix back to the pixel
            let a = alpha.data[h / 2 * w + x];
            for c in 0..3 {
                assert!((a * sp[c] + (1.0 - a) * up[c] - i[c]).abs() < 0.02, "x {x}");
            }
        }
    }

    #[test]
    fn hostile_input_gives_the_photo_back() {
        let img = Rgb32f::from_fn(4, 4, |_, _| [0.5; 3]);
        let (s, u) = estimate(&img, &Plane::new(3, 3), 4.0);
        assert_eq!(s.data, img.data);
        assert_eq!(u.data, img.data);
        let mut nan = Plane::new(4, 4);
        nan.data.iter_mut().for_each(|v| *v = f32::NAN);
        let (s, _) = estimate(&img, &nan, 4.0);
        assert!(s.data.iter().flatten().all(|v| v.is_finite()));
        let empty = Rgb32f::new(0, 0);
        let _ = estimate(&empty, &Plane::new(0, 0), 0.0);
    }
}
