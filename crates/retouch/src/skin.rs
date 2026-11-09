//! Skin: Smoothness, Texture and Shine. Evens the skin's surface while keeping its pores.
//!
//! Frequency separation on the face's Facial Skin mask. The skin inside the mask is split into four bands by
//! Gaussian blurs whose sizes are fractions of the face ([`FINE`], [`MID`], [`TONE`]):
//!
//! - **fine** (pores, fine hairs, grain): the picture less its [`FINE`] blur. Smoothness never touches it, so the
//!   skin keeps its texture; Texture strengthens it (up to [`TEXTURE_GAIN`] more).
//! - **mid** (bumps, small marks, fine lines): [`FINE`] blur less [`MID`] blur, taken away by Smoothness (up to
//!   [`MID_CUT`] of it).
//! - **tone** (blotches, uneven colour): [`MID`] blur less [`TONE`] blur, taken away by Smoothness (up to
//!   [`TONE_CUT`]).
//! - **base** (the face's shape and light): the [`TONE`] blur, never changed by Smoothness.
//!
//! Each band's change fades out where its contrast is high (an edge such as a nostril or a lash line is not
//! unevenness), so the face keeps its shape. Shine lowers oily highlights: where the smoothed skin is brighter
//! than the skin around it ([`SHINE_BASE`] blur) by more than [`SHINE_START`], and paler (light on top of the skin's
//! colour, unlike the light on one side of a face), the excess is compressed by taking away neutral light, which
//! gives the skin its colour back; the texture on top is scaled with it.
//!
//! Every blur reads only the skin (a blur of the picture weighted by the mask, divided by the blur of the mask), so
//! no colour from hair, eyes, lips or the background bleeds in: no halo at the mask's edge. The change is weighted
//! by the mask, so a pixel outside it never changes, and the bands are measured on pixels in the mask's box only.
//! The blurs run on the region reduced so that each is at least `PER_PIXEL` reduced pixels wide, and are read back
//! bilinearly, which keeps a slider drag within the draft budget.
//!
//! A later Refine brush (#99) paints over the mask: `skin_mask` is where it will be applied.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

/// The fine band's blur, a fraction of the face's size: what is finer than it is texture.
pub const FINE: f64 = 0.008;
/// The mid band's blur, a fraction of the face's size.
pub const MID: f64 = 0.03;
/// The tone band's blur, a fraction of the face's size: what is coarser is the face's shape and light.
pub const TONE: f64 = 0.07;
/// The blur Shine measures highlights against, a fraction of the face's size.
pub const SHINE_BASE: f64 = 0.1;

/// The share of the mid band Smoothness 100 takes away.
pub const MID_CUT: f32 = 0.9;
/// The share of the tone band Smoothness 100 takes away.
pub const TONE_CUT: f32 = 0.6;
/// How much stronger Texture 100 makes the fine band (0.6: 60 % more).
pub const TEXTURE_GAIN: f32 = 0.6;
/// A band's change is halved where its contrast (to the skin around it) is this; the mid, tone and fine bands.
const EDGE: [f32; 3] = [0.10, 0.15, 0.12];
/// Shine: skin brighter than the skin around it ([`SHINE_BASE`]) by more than this (natural log: 1.1×) is a
/// highlight.
pub const SHINE_START: f32 = 0.1;
/// Shine: a highlight as saturated as the skin around it counts this much (the light falling on one side of the
/// face is not shine); one that has lost a quarter of the skin's saturation or more counts fully.
const SHINE_SAME_COLOUR: f32 = 0.25;
/// Skin around a highlight with less saturation than this is taken as grey (a black-and-white photo): there,
/// brightness alone finds the shine.
const GREY: f32 = 0.04;
/// Shine 100 keeps this share of a highlight's excess (in log brightness).
const SHINE_KEEP: f32 = 0.2;
/// Shine never takes more than this share of a channel away.
const SHINE_MOST: f32 = 0.6;

/// Most pixels of the mask's box the step works on (2⁶ M: a face filling a 100 MP export).
const MAX_BOX: usize = 1 << 26;

/// The Skin settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skin {
    /// 0…100: how much of the skin's unevenness (bumps, blotches) is taken away; 0 is none.
    pub smoothness: f64,
    /// 0…100: how much the skin's fine texture (pores) is strengthened; 0 is as it is.
    pub texture: f64,
    /// 0…100: how much oily highlights are lowered; 0 is none.
    pub shine: f64,
}

impl Feature for Skin {
    const KEY: &'static str = "skin";
    const LABEL: &'static str = "Skin";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.skin.smoothness", "Smoothness"), get: |s| s.smoothness, set: |s, v| s.smoothness = v },
        Control { spec: Spec::amount("retouch.skin.texture", "Texture"), get: |s| s.texture, set: |s, v| s.texture = v },
        Control { spec: Spec::amount("retouch.skin.shine", "Shine"), get: |s| s.shine, set: |s, v| s.shine = v },
    ];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        let Some(face) = face else { return };
        let k = Strength::of(self);
        if k.is_none() || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(size) = scene.faces.get(face).map(|f| f.size) else { return };
        let Some(b) = scene.mask_box(face, Part::FacialSkin).filter(|b| (b[2] - b[0]).saturating_mul(b[3] - b[1]) <= MAX_BOX) else { return };
        let Some(m) = skin_mask(scene, face, b) else { return };
        let out = retouch(crop(img, b), &m, size, k);
        paste(img, b, &out, &m);
    }
}

/// The sliders as 0…1 factors (a value that is not a number is 0).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Strength {
    smooth: f32,
    texture: f32,
    shine: f32,
}

impl Strength {
    fn of(s: &Skin) -> Strength {
        let f = |v: f64| if v.is_finite() { (v.clamp(0.0, 100.0) / 100.0) as f32 } else { 0.0 };
        Strength { smooth: f(s.smoothness), texture: f(s.texture), shine: f(s.shine) }
    }

    fn is_none(&self) -> bool {
        self.smooth <= 0.0 && self.texture <= 0.0 && self.shine <= 0.0
    }
}

/// The skin mask of `face` over the box `b` (0…1); `None` when it is 0 everywhere there. The Refine brush (#99)
/// will add and take away its strokes here.
fn skin_mask(scene: &Scene<'_>, face: usize, b: [usize; 4]) -> Option<Plane> {
    let mask = scene.faces.get(face)?.face.mask(Part::FacialSkin)?;
    let view = scene.view();
    let [x0, y0, x1, y1] = b;
    let w = x1 - x0;
    let mut m = Plane::new(w, y1 - y0);
    par_rows(&mut m.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let (nx, ny) = view.to_norm((x0 + x) as f64 + 0.5, (y0 + y) as f64 + 0.5);
            *v = mask.sample(nx, ny).clamp(0.0, 1.0);
        }
    });
    m.data.iter().any(|v| *v > 0.0).then_some(m)
}

fn crop(img: &Rgb32f, b: [usize; 4]) -> Rgb32f {
    let [x0, y0, x1, y1] = b;
    let w = x1 - x0;
    let mut c = Rgb32f::new(w, y1 - y0);
    par_rows(&mut c.data, w, |y, row| {
        let start = (y0 + y) * img.width + x0;
        if let Some(src) = img.data.get(start..start + w) {
            row.copy_from_slice(src);
        }
    });
    c
}

/// Write `out` into `img` over the box `b`, only where the mask is above 0.
fn paste(img: &mut Rgb32f, b: [usize; 4], out: &Rgb32f, m: &Plane) {
    let [x0, y0, x1, _] = b;
    let (w, iw) = (x1 - x0, img.width);
    for (y, (orow, mrow)) in out.data.chunks(w.max(1)).zip(m.data.chunks(w.max(1))).enumerate() {
        let start = (y0 + y) * iw + x0;
        let Some(dst) = img.data.get_mut(start..start + w) else { return };
        for ((d, o), mv) in dst.iter_mut().zip(orow).zip(mrow) {
            if *mv > 0.0 {
                *d = *o;
            }
        }
    }
}

/// Luminance weights (Rec.2020; close enough for the sRGB of the examples: only contrast is measured).
const Y: [f32; 3] = [0.2627, 0.678, 0.0593];

fn luma(p: [f32; 3]) -> f32 {
    p[0] * Y[0] + p[1] * Y[1] + p[2] * Y[2]
}

/// The smallest luminance contrast is measured against (scene-linear; keeps black skin from dividing by 0).
const DARK: f32 = 1e-4;

/// How much of a band's change is kept at relative contrast `c` for edge `e`: 1 for small contrast, ½ at `e`.
fn soft(c: f32, e: f32) -> f32 {
    let r = c / e;
    1.0 / (1.0 + r * r)
}

/// The retouched skin over the region, weighted by the mask `m` (pixels where it is 0 are left as they are).
fn retouch(mut c: Rgb32f, m: &Plane, size: f64, k: Strength) -> Rgb32f {
    let px = |f: f64| {
        let v = (f * size) as f32;
        if v.is_finite() { v.max(0.0) } else { 0.0 }
    };
    let w = c.width;
    let mut weighted = Image::<[f32; 4]>::new(w, c.height);
    par_rows(&mut weighted.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (p, mv) = (c.data.get(i).copied().unwrap_or_default(), m.data.get(i).copied().unwrap_or(0.0));
            *o = [p[0] * mv, p[1] * mv, p[2] * mv, mv];
        }
    });
    let fine = Reduced::blurred(&weighted, px(FINE));
    let wide = Reduced::new(&weighted, px(MID));
    let (mid, tone, base) = (wide.blur(px(MID)), wide.blur(px(TONE)), wide.blur(px(SHINE_BASE)));
    par_rows(&mut c.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let mv = m.data.get(y * w + x).copied().unwrap_or(0.0);
            if mv <= 0.0 {
                continue;
            }
            let src = *o;
            let g1 = fine.read(&fine.taps(x, y), src);
            let t = mid.taps(x, y);
            let (g2, g3, g4) = (mid.read(&t, src), tone.read(&t, src), base.read(&t, src));
            let new = pixel(src, g1, g2, g3, g4, k);
            *o = std::array::from_fn(|c| src[c] + mv * (new[c] - src[c]));
        }
    });
    c
}

/// One pixel: `src` and its blurs at [`FINE`], [`MID`], [`TONE`] and [`SHINE_BASE`].
fn pixel(src: [f32; 3], g1: [f32; 3], g2: [f32; 3], g3: [f32; 3], g4: [f32; 3], k: Strength) -> [f32; 3] {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let (fine, mid, tone) = (sub(src, g1), sub(g1, g2), sub(g2, g3));
    // each band's contrast against the skin around it (its luminance, or half its strongest channel for a change
    // of colour)
    let inv = 1.0 / luma(g2).max(DARK);
    let rel = |d: [f32; 3]| luma(d).abs().max(d[0].abs().max(d[1].abs()).max(d[2].abs()) * 0.5) * inv;
    let cut_mid = k.smooth * MID_CUT * soft(rel(mid), EDGE[0]);
    let cut_tone = k.smooth * TONE_CUT * soft(rel(tone), EDGE[1]);
    let gain = k.texture * TEXTURE_GAIN * soft(rel(fine), EDGE[2]);
    // the smoothed skin without its fine and mid bands, then its highlights lowered
    let low: [f32; 3] = std::array::from_fn(|c| g3[c] + tone[c] * (1.0 - cut_tone));
    let (lit, scale) = deshine(low, g4, k.shine);
    std::array::from_fn(|c| {
        let v = lit[c] + scale * (mid[c] * (1.0 - cut_mid) + fine[c] * (1.0 + gain));
        // never below black, unless the source already was
        if v.is_finite() { v.max(src[c].min(0.0)) } else { src[c] }
    })
}

/// `low` with its shine lowered by `amount` (0…1), against the face's wide average `base`, and the factor its
/// luminance changed by (the texture on top is scaled by it).
fn deshine(low: [f32; 3], base: [f32; 3], amount: f32) -> ([f32; 3], f32) {
    let (y, yb) = (luma(low), luma(base));
    if amount <= 0.0 || y <= DARK || yb <= DARK {
        return (low, 1.0);
    }
    let over = (y / yb).ln() - SHINE_START;
    if over <= 0.0 || !over.is_finite() {
        return (low, 1.0);
    }
    // oily shine is paler than the skin around it (white light on top of the skin's colour)
    let sat = |p: [f32; 3]| {
        let (hi, lo) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
        if hi > DARK { (hi - lo) / hi } else { 0.0 }
    };
    let around = sat(base);
    let pale = if around < GREY { 1.0 } else { (SHINE_SAME_COLOUR + (1.0 - sat(low) / around) * 3.0).clamp(SHINE_SAME_COLOUR, 1.0) };
    let target = yb * (SHINE_START + over * (1.0 - amount * pale * (1.0 - SHINE_KEEP))).exp();
    // take neutral light away (a specular highlight is the light's colour on the skin's), at most SHINE_MOST of a
    // channel
    let neutral = (y - target).max(0.0);
    let lit: [f32; 3] = std::array::from_fn(|c| (low[c] - neutral).max(low[c] * (1.0 - SHINE_MOST)));
    let scale = (luma(lit) / y).clamp(0.0, 1.0);
    (lit, scale)
}

/// A mask-weighted blur back to colour; `fallback` where the mask's blur is 0.
fn unweight(p: [f32; 4], fallback: [f32; 3]) -> [f32; 3] {
    if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { fallback }
}

/// The mask-weighted region reduced by `factor` (whole pixels averaged), for blurs of at least about
/// [`PER_PIXEL`] times the factor; a blur of it is read back bilinearly.
#[derive(Clone)]
struct Reduced {
    img: Image<[f32; 4]>,
    factor: usize,
}

/// The smallest blur, in reduced pixels, a reduction is made for (a blur of 2.5 px or more read back bilinearly
/// differs from the full-size blur by well under a percent of the band).
const PER_PIXEL: f32 = 2.5;

impl Reduced {
    /// Reduce `weighted` for blurs of `smallest` full-size pixels or more.
    fn new(weighted: &Image<[f32; 4]>, smallest: f32) -> Reduced {
        let (fw, fh) = (weighted.width, weighted.height);
        let factor = ((smallest / PER_PIXEL).floor() as usize).clamp(1, fw.max(fh).max(1));
        if factor == 1 {
            return Reduced { img: weighted.clone(), factor };
        }
        let (w, h) = (fw.div_ceil(factor), fh.div_ceil(factor));
        let mut img = Image::<[f32; 4]>::new(w, h);
        par_rows(&mut img.data, w.max(1), |sy, row| {
            let ys = sy * factor..((sy + 1) * factor).min(fh);
            for (sx, o) in row.iter_mut().enumerate() {
                let xs = sx * factor..((sx + 1) * factor).min(fw);
                let mut acc = [0.0f32; 4];
                for y in ys.clone() {
                    for p in weighted.data.get(y * fw + xs.start..y * fw + xs.end).unwrap_or(&[]) {
                        acc = std::array::from_fn(|c| acc[c] + p[c]);
                    }
                }
                let n = (ys.len() * xs.len()).max(1) as f32;
                *o = acc.map(|v| v / n);
            }
        });
        Reduced { img, factor }
    }

    /// `weighted` blurred by `sigma` full-size pixels, reduced for it (not copied when it needs no reduction).
    fn blurred(weighted: &Image<[f32; 4]>, sigma: f32) -> Reduced {
        if ((sigma / PER_PIXEL).floor() as usize) < 2 {
            return Reduced { img: gaussian(weighted, sigma), factor: 1 };
        }
        Reduced::new(weighted, sigma).blur(sigma)
    }

    /// Blurred by `sigma` full-size pixels.
    fn blur(&self, sigma: f32) -> Reduced {
        Reduced { img: gaussian(&self.img, sigma / self.factor as f32), factor: self.factor }
    }

    /// Where full-size pixel (`x`, `y`) reads this reduction: four samples and their bilinear weights. Every blur
    /// of one reduction shares them.
    fn taps(&self, x: usize, y: usize) -> Taps {
        let (w, h) = (self.img.width, self.img.height);
        if w == 0 || h == 0 {
            return Taps { at: [usize::MAX; 4], weight: [0.0; 4] };
        }
        if self.factor == 1 {
            let i = y * w + x;
            return Taps { at: [i; 4], weight: [1.0, 0.0, 0.0, 0.0] };
        }
        let f = self.factor as f32;
        let fx = ((x as f32 + 0.5) / f - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = ((y as f32 + 0.5) / f - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        Taps {
            at: [y0 * w + x0, y0 * w + x1, y1 * w + x0, y1 * w + x1],
            weight: [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty],
        }
    }

    /// This blur at `taps`, back to colour; `fallback` where the mask's blur is 0.
    fn read(&self, t: &Taps, fallback: [f32; 3]) -> [f32; 3] {
        let mut p = [0.0f32; 4];
        for (i, wt) in t.at.iter().zip(t.weight) {
            if let Some(v) = self.img.data.get(*i) {
                p = std::array::from_fn(|c| p[c] + v[c] * wt);
            }
        }
        unweight(p, fallback)
    }
}

/// Four samples of a reduction and their weights ([`Reduced::taps`]).
struct Taps {
    at: [usize; 4],
    weight: [f32; 4],
}
