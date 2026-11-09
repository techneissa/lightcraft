//! The finish stage at a decontaminated edge (`fork/decontaminate.md`). A pixel is drawn once, as the composite of
//! its two colours: each decontaminated mask's scene edits (Exposure, Temp, Tint, Dehaze, Noise, Moiré, Defringe) are
//! made to the selected colour with the mask wholly on and to the unselected one with it wholly off, and mixed back by
//! the alpha in scene linear; then the tone stage (Highlights, Shadows, Clarity, Texture, the tone map) runs once on
//! the composite, as on any pixel. The colour edits after it (Saturation, Hue, Color, Recolor) are made to each
//! colour's display colour apart, and mixed by the alpha.
//!
//! Every change is added as a difference, so a mask with nothing to edit apart changes no pixel, whatever the tone map
//! and the other masks do.

use lightcraft_color::luminance_2020;

use crate::Prepared;
use crate::finish::{FinishParams, MASK_TERMS, PixelLocalTerms, adjust_scene_pixel, pixel_local_terms};

/// For each split, the index of its mask among the render's masks, in mask order.
pub(crate) fn index(p: &Prepared) -> Vec<(usize, usize)> {
    let mut idx: Vec<(usize, usize)> =
        p.split.iter().enumerate().filter_map(|(si, s)| p.masks.iter().position(|e| e.id == s.id).map(|k| (k, si))).collect();
    idx.sort_unstable();
    idx
}

/// One decontaminated mask's edge at a pixel: the mask's index, its alpha there, the Amount (0..1), and the pixel's
/// selected and unselected colours.
pub(crate) struct Hit {
    k: usize,
    a: f32,
    t: f32,
    sel: [f32; 3],
    unsel: [f32; 3],
}

/// Every decontaminated mask whose edge holds pixel `(x, y)`, in mask order (`idx`: [`index`]).
pub(crate) fn hits(p: &Prepared, idx: &[(usize, usize)], x: usize, y: usize) -> Vec<Hit> {
    idx.iter()
        .filter_map(|&(k, si)| {
            let s = p.split.get(si)?;
            let (sel, unsel) = s.edge.at(x, y)?;
            let a = p.masks.get(k)?.alpha.at(x, y);
            (a > super::EDGE.0 && a < super::EDGE.1 && s.amount > 0.0).then_some(Hit { k, a, t: s.amount, sel, unsel })
        })
        .collect()
}

/// What the per-pixel stage knows of the pixel being drawn.
pub(crate) struct Ctx<'a> {
    pub p: &'a Prepared,
    pub fp: &'a FinishParams,
    pub terms: &'a [Option<[f32; MASK_TERMS]>],
    pub active: &'a [usize],
    pub x: usize,
    pub y: usize,
    pub i: usize,
}

/// A hit's two colours: the local terms with its mask wholly on and off, and the scene colours made with them.
pub(crate) struct Layers {
    on: PixelLocalTerms,
    off: PixelLocalTerms,
    s: [f32; 3],
    u: [f32; 3],
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| a[c] + b[c])
}

/// The pixel's scene colour `c` (every mask at its own alpha, `local`) with each hit's scene edits made apart, the
/// change of its log luminance, and each hit's layers (`None`: its mask edits nothing apart).
pub(crate) fn scene(cx: &Ctx<'_>, local: &PixelLocalTerms, c: [f32; 3], hits: &[Hit]) -> ([f32; 3], f32, Vec<Option<Layers>>) {
    let Ctx { p, fp, terms, active, x, y, i } = *cx;
    let mut out = c;
    let layers = hits
        .iter()
        .map(|h| {
            let t = terms.get(h.k).copied().flatten()?;
            let (scene, colour) = (super::scene_terms(&t), super::colour_terms(&t));
            if !scene && !colour {
                return None;
            }
            let on = pixel_local_terms(p, terms, active, x, y, Some((h.k, 1.0)));
            let off = pixel_local_terms(p, terms, active, x, y, Some((h.k, 0.0)));
            let (s, _) = adjust_scene_pixel(p, fp, i, h.sel, &on);
            let (u, _) = adjust_scene_pixel(p, fp, i, h.unsel, &off);
            if scene {
                // what the mask's edit does to each colour wholly on (off) beyond what it does at the pixel's alpha
                let (sa, _) = adjust_scene_pixel(p, fp, i, h.sel, local);
                let (ua, _) = adjust_scene_pixel(p, fp, i, h.unsel, local);
                let d: [f32; 3] = std::array::from_fn(|c| h.t * (h.a * (s[c] - sa[c]) + (1.0 - h.a) * (u[c] - ua[c])));
                out = add(out, d);
            }
            Some(Layers { on, off, s, u })
        })
        .collect();
    let lum = |c: [f32; 3]| luminance_2020(c).max(1e-7).log2();
    let shift = if out == c { 0.0 } else { lum(out) - lum(c) };
    (out, if shift.is_finite() { shift } else { 0.0 }, layers)
}

/// The pixel's colour edits with each hit's made apart: `pre` is the composite's display colour before them, `d` after
/// them (every mask at its own alpha), `g` the tone stage's gain. `to_display` takes a scene colour to `pre`'s stage;
/// `colour` makes the colour edits with the given local terms and one mask's alpha replaced.
pub(crate) fn colour(
    cx: &Ctx<'_>,
    hits: &[Hit],
    layers: &[Option<Layers>],
    (pre, d, g): ([f32; 3], [f32; 3], f32),
    to_display: &dyn Fn([f32; 3]) -> [f32; 3],
    colour: &dyn Fn([f32; 3], &PixelLocalTerms, Option<(usize, f32)>) -> [f32; 3],
) -> [f32; 3] {
    let mut out = d;
    for (h, l) in hits.iter().zip(layers) {
        let Some(l) = l else { continue };
        if !cx.terms.get(h.k).copied().flatten().is_some_and(|t| super::colour_terms(&t)) {
            continue;
        }
        // the two colours as the composite's tone stage draws them, corrected to mix back to it exactly
        let (mut ds, mut du) = (to_display(l.s.map(|v| v * g)), to_display(l.u.map(|v| v * g)));
        let a = h.a;
        let norm = a * a + (1.0 - a) * (1.0 - a);
        for c in 0..3 {
            let r = (pre[c] - (a * ds[c] + (1.0 - a) * du[c])) / norm;
            ds[c] = (ds[c] + a * r).max(0.0);
            du[c] = (du[c] + (1.0 - a) * r).max(0.0);
        }
        let (cs, cu) = (colour(ds, &l.on, Some((h.k, 1.0))), colour(du, &l.off, Some((h.k, 0.0))));
        out = add(out, std::array::from_fn(|c| h.t * (a * cs[c] + (1.0 - a) * cu[c] - d[c])));
    }
    out
}
