//! Skin: Smoothness, Texture and Shine. Evens the skin's surface while keeping its pores.
//!
//! Frequency separation on the face's Facial Skin mask, in log luminance (a blotch is a change of brightness by a
//! factor, the same on dark and light skin) and colour ratios (`split`), with sizes that are fractions of the
//! face:
//!
//! - **pores** (pores, fine hairs, grain): the skin less its blur at [`PORES`]. Smoothness keeps them as they are;
//!   Texture strengthens them (up to [`TEXTURE_GAIN`] more) or softens them (up to [`TEXTURE_SOFTEN`] less).
//! - **the rest** (bumps, small marks, fine lines, blotches, and the face's shape and light): Smoothness evens it
//!   with a bilateral grid (`grid::Grid`) over [`SMOOTH`] of the face: a pixel is averaged with the skin around it of
//!   about its own light ([`EDGE`]), so bumps, marks and blotches go while a nostril, a lash line, a fold or the
//!   face's shading, darker or brighter than the skin beside it, stays sharp with no halo. [`TONE_CUT`] of a wider
//!   grid ([`TONE`]) takes larger blotches down a little; a change of colour past [`COLOUR_EDGE`] is held back
//!   (the grid tells edges by light only); and the change fades in over [`FEATHER`] from the mask's edge.
//!
//! Shine lowers oily highlights: where the smoothed skin is brighter than the skin around it ([`SHINE_BASE`]
//! blur) by more than [`SHINE_START`], and paler (light on top of the skin's colour, unlike the light on one side
//! of a face), the excess is compressed by taking away neutral light, which gives the skin its colour back; the
//! pores on top keep their contrast.
//!
//! Every blur reads only the skin (a blur of the picture weighted by the mask, divided by the blur of the mask), so
//! no colour from hair, eyes, lips or the background bleeds in: no halo at the mask's edge. The change is weighted
//! by the mask, so a pixel outside it never changes. The wide blurs run on the region reduced so that each is at
//! least `PER_PIXEL` reduced pixels wide, and are read back bilinearly, which keeps a slider drag within the draft
//! budget.
//!
//! A later Refine brush (#99) paints over the mask: `skin_mask` is where it will be applied.

use lightcraft_raster::{Image, Plane, Rgb32f, par_rows};
use serde::{Deserialize, Serialize};

use crate::{Control, Feature, Part, Scene, Scope, Spec, Stage};

mod grid;

use grid::{Grid, Reduced};

/// The pores' size, a fraction of the face: what is finer (pores, fine hairs, grain) is the skin's texture, which
/// Smoothness keeps and Texture scales.
pub const PORES: f64 = 0.002;
/// The window Smoothness evens the skin over (a Gaussian's sigma), a fraction of the face: bumps, small marks,
/// fine lines and blotches up to about this size go.
pub const SMOOTH: f64 = 0.04;
/// A change of colour (a channel's share of the light, relative) Smoothness keeps half of: blotchy redness, a few
/// percent, is evened; the colour of a neighbouring area of the same light (a lit nose wing beside a cheek) is not
/// carried over.
pub const COLOUR_EDGE: f32 = 0.08;
/// How far in from the mask's edge Smoothness takes to come to full strength (a Gaussian's sigma, a fraction of
/// the face): no seam where smoothed skin meets the hair, beard or background beyond the mask.
pub const FEATHER: f64 = 0.01;
/// The tone band's coarse end, a fraction of the face's size: what is coarser is the face's shape and light.
pub const TONE: f64 = 0.07;
/// The blur Shine measures highlights against, a fraction of the face's size.
pub const SHINE_BASE: f64 = 0.1;

/// The share of the grid at [`TONE`] mixed into the grid at [`SMOOTH`]: how much of the blotches between the two
/// sizes Smoothness 100 takes away.
pub const TONE_CUT: f32 = 0.15;
/// The difference of light (natural log) Smoothness tells an edge by: skin within about this of a pixel's own
/// light is averaged with it, and skin far brighter or darker (the other side of a nostril, a lash line or a
/// shadow) is not.
pub const EDGE: f32 = 0.4;
/// How much stronger Texture +100 makes the pores (1.0: twice their contrast).
pub const TEXTURE_GAIN: f32 = 1.0;
/// How much of the pores' contrast Texture −100 takes away.
pub const TEXTURE_SOFTEN: f32 = 0.8;
/// Texture changes pores less where their own contrast reaches this (natural log; half at it): an edge's fine
/// detail, such as a nostril's rim, is not texture.
const TEXTURE_EDGE: f32 = 0.3;
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

/// Most pixels of the mask's box the step works on at full size (2²⁶, about 67 M: a face filling a 100 MP export).
/// A bigger box is worked on reduced, as a draft is, and the change carried back ([`Skin::apply_capped`]).
pub const MAX_BOX: usize = 1 << 26;

/// The Skin settings. A field added here is read as its default from settings saved without it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skin {
    /// 0…100: how much of the skin's unevenness (bumps, blotches) is taken away; 0 is none.
    pub smoothness: f64,
    /// −100…100: how much the skin's pores are softened (below 0) or strengthened (above 0); 0 is as they are.
    pub texture: f64,
    /// 0…100: how much oily highlights are lowered; 0 is none.
    pub shine: f64,
}

impl Feature for Skin {
    const KEY: &'static str = "skin";
    const LABEL: &'static str = "Skin";
    const STAGE: Stage = Stage::Retouch;
    const SCOPE: Scope = Scope::Face;
    const PARTS: &'static [Part] = &[Part::FacialSkin];
    /// Skin works inside its Facial Skin mask's box ([`Scene::mask_box`], a pixel past the mask's area) and its
    /// blurs stop at the box's edges: it reads nothing farther (a hair of margin for the box's own pixel).
    const REACH: f64 = 0.02;
    const CONTROLS: &'static [Control<Self>] = &[
        Control { spec: Spec::amount("retouch.skin.smoothness", "Smoothness"), get: |s| s.smoothness, set: |s, v| s.smoothness = v },
        Control { spec: Spec::signed("retouch.skin.texture", "Texture"), get: |s| s.texture, set: |s, v| s.texture = v },
        Control { spec: Spec::amount("retouch.skin.shine", "Shine"), get: |s| s.shine, set: |s, v| s.shine = v },
    ];

    fn apply(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>) {
        self.apply_capped(img, scene, face, MAX_BOX);
    }
}

impl Skin {
    /// [`Feature::apply`] with `max_box` the most pixels of the mask's box worked on at full size ([`MAX_BOX`] in
    /// the step): a bigger box is worked on reduced by a whole factor, as a draft of it is, and the change is
    /// carried back to the full-size skin as a gain, so a huge face is retouched as its draft shows, never skipped.
    pub fn apply_capped(&self, img: &mut Rgb32f, scene: &Scene<'_>, face: Option<usize>, max_box: usize) {
        let Some(face) = face else { return };
        let k = Strength::of(self);
        if k.is_none() || img.data.len() != img.width.saturating_mul(img.height) {
            return;
        }
        let Some(size) = scene.faces.get(face).map(|f| f.size) else { return };
        let Some(b) = scene.mask_box(face, Part::FacialSkin) else { return };
        if let Some(f) = crate::reduced::factor(b, max_box) {
            let keep = |x, y| scene.mask_at(face, Part::FacialSkin, x, y) > 0.0;
            crate::reduced::retouch(img, scene, face, b, f, keep, |small, sub, at| self.apply_capped(small, sub, Some(at), usize::MAX));
            return;
        }
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
        let f = |v: f64, lo: f64| if v.is_finite() { (v.clamp(lo, 100.0) / 100.0) as f32 } else { 0.0 };
        Strength { smooth: f(s.smoothness, 0.0), texture: f(s.texture, -100.0), shine: f(s.shine, 0.0) }
    }

    fn is_none(&self) -> bool {
        self.smooth <= 0.0 && self.texture == 0.0 && self.shine <= 0.0
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
    // the skin (split) weighted by the mask, then without its pores: [log luminance, red, blue, mask]
    let mut base = Image::<[f32; 4]>::new(w, c.height);
    par_rows(&mut base.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (p, mv) = (c.data.get(i).copied().unwrap_or_default(), m.data.get(i).copied().unwrap_or(0.0));
            let l = split(p);
            *o = [l[0] * mv, l[1] * mv, l[2] * mv, mv];
        }
    });
    let pores = Reduced::blurred(&base, px(PORES));
    par_rows(&mut base.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            if o[3] <= 0.0 {
                continue;
            }
            let own = [o[0] / o[3], o[1] / o[3], o[2] / o[3]];
            let p = pores.read(&pores.taps(x, y), own);
            *o = [p[0], p[1], p[2], o[3]];
        }
    });
    drop(pores);
    let even = if k.smooth > 0.0 { Even::new(&base, px(SMOOTH), px(TONE), px(FEATHER)) } else { None };
    let around = if k.shine > 0.0 { Some(Reduced::of_skin(&base, px(SMOOTH)).blur(px(SHINE_BASE))) } else { None };
    let wide = |x: usize, y: usize, own: [f32; 3]| around.as_ref().map_or(own, |r| r.read(&r.taps(x, y), own));
    par_rows(&mut c.data, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let Some(b) = base.data.get(y * w + x) else { continue };
            if b[3] <= 0.0 {
                continue;
            }
            let (src, smooth) = (*o, [b[0], b[1], b[2]]);
            let even = even.as_ref().map(|e| e.at(x, y, smooth));
            let new = pixel(src, smooth, even, wide(x, y, smooth), k);
            *o = std::array::from_fn(|c| src[c] + b[3] * (new[c] - src[c]));
        }
    });
    c
}

/// The even skin Smoothness 100 gives: the pore-free skin through the bilateral grid ([`Grid`]) at [`SMOOTH`],
/// with [`TONE_CUT`] of the grid at [`TONE`] mixed in (blotches up to that size).
struct Even {
    /// The grid at [`SMOOTH`] with [`TONE_CUT`] of the grid at [`TONE`] mixed in.
    grid: Grid,
    /// The mask blurred by [`FEATHER`]: Smoothness fades in from the mask's edge over about that much.
    fade: Reduced,
}

impl Even {
    fn new(base: &Image<[f32; 4]>, sigma: f32, tone: f32, feather: f32) -> Option<Even> {
        let range = EDGE;
        let grid = Grid::new(base, sigma, range)?;
        let fade = Reduced::of_skin(base, feather).blur(feather);
        Some(Even { grid: grid.blurred(sigma, range).mix(&grid.blurred(tone, range), TONE_CUT), fade })
    }

    /// The even skin at (`x`, `y`) and how much of it to take there (0 at the mask's edge, 1 a [`FEATHER`]
    /// inside it).
    fn at(&self, x: usize, y: usize, own: [f32; 3]) -> ([f32; 3], f32) {
        let even = self.grid.read(x, y, own);
        let inside = self.fade.weight(&self.fade.taps(x, y));
        let fade = ((inside - 0.5) / 0.4).clamp(0.0, 1.0);
        // colour: the grid tells edges by light only, so skin of another colour but the same light (a lit nose
        // wing beside a cheek) can be averaged in; a change of colour past COLOUR_EDGE is held back
        let ce = COLOUR_EDGE;
        let colour = |c: usize| {
            let d = even[c] - own[c];
            d * soft(d.abs() / own[c].abs().max(0.05), ce)
        };
        ([even[0], own[1] + colour(1), own[2] + colour(2)], fade * fade * (3.0 - 2.0 * fade))
    }
}

/// One pixel: `src`, its skin without pores `smooth` ([`split`]), the even skin there
/// (`None` with Smoothness off) and the skin around it at [`SHINE_BASE`].
fn pixel(src: [f32; 3], smooth: [f32; 3], even: Option<([f32; 3], f32)>, around: [f32; 3], k: Strength) -> [f32; 3] {
    if luma(src) <= DARK || src.iter().any(|v| !v.is_finite()) {
        return src;
    }
    let l = split(src);
    let pores: [f32; 3] = std::array::from_fn(|c| l[c] - smooth[c]);
    let mut low = smooth;
    if let Some((even, fade)) = even {
        low = std::array::from_fn(|c| smooth[c] + k.smooth * fade * (even[c] - smooth[c]));
    }
    if k.shine > 0.0 {
        let (lit, _) = deshine(join(low), join(around), k.shine);
        low = split(lit);
    }
    // Texture: the pores' brightness stronger or softer, less where it is an edge's own fine detail
    let gain = if k.texture > 0.0 { k.texture * TEXTURE_GAIN } else { k.texture * TEXTURE_SOFTEN } * soft(pores[0].abs(), TEXTURE_EDGE);
    let out = [(low[0] + pores[0] * (1.0 + gain)).min(l[0] + MOST_LOG), low[1] + pores[1], low[2] + pores[2]];
    let new = join(out);
    // never below black, unless the source already was
    std::array::from_fn(|c| if new[c].is_finite() { new[c].max(src[c].min(0.0)) } else { src[c] })
}

/// The most a pixel is brightened, in natural log (×20): only a broken input would ask for more.
const MOST_LOG: f32 = 3.0;

/// Red's and blue's luminance weights; green's is what is left ([`join`]).
const YR: f32 = Y[0];
const YB: f32 = Y[2];

/// `p` as the filter works on it: log luminance, and red and blue over luminance (its colour; green's
/// follows from them). Luminance at or below [`DARK`] reads as [`DARK`]. Log luminance makes a blotch a change by
/// a factor, the same on dark and light skin; colour as ratios stays calm where a channel is near 0 (the blue of
/// warm skin in low light), which a log per channel would blow up into noise.
fn split(p: [f32; 3]) -> [f32; 3] {
    let y = luma(p);
    let y = if y.is_finite() { y.max(DARK) } else { DARK };
    let ratio = |v: f32| if v.is_finite() { (v / y).clamp(-RATIO, RATIO) } else { 0.0 };
    [y.ln(), ratio(p[0]), ratio(p[2])]
}

/// Back from [`split`] to light.
fn join(v: [f32; 3]) -> [f32; 3] {
    let y = v[0].exp();
    let g = (1.0 - YR * v[1] - YB * v[2]) / Y[1];
    [y * v[1], y * g, y * v[2]]
}

/// The largest ratio of a channel to luminance [`split`] keeps (pure blue is about 17).
const RATIO: f32 = 32.0;

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
