//! Recolor: a mask's area takes a new colour and keeps its own shading (issue #83).
//!
//! The maths works in OkLab on display-linear colour (after the tone map and the colour stage, where the colour
//! overlay also works). It needs the area's own colour, its [`Reference`]: the alpha-weighted mean lightness and
//! chroma and the mean hue of the masked pixels, measured once per render on a fixed-size proxy of the whole frame
//! ([`references`]), so a preview, an export and the GPU all recolor from the same numbers.
//!
//! Per pixel ([`apply`]):
//! - **Lightness** keeps its proportion to the area's mean: below the mean `L' = Lt · L / Lr` (a fold half as light
//!   as the area stays half as light as the target); above it the rest of the range up to white is squeezed so
//!   nothing passes white. A target as light as the area leaves lightness exactly as it was.
//! - **Chroma** keeps its proportion to the area's mean chroma (softly limited above it), so a pixel greyer than the
//!   area stays greyer, and one that is grey for its lightness (a specular highlight, a white sole) stays grey. A near-grey area (a black shirt) has no chroma to keep: its chroma
//!   follows its shading instead, and fades out towards white.
//! - **Hue** is the target's, plus the pixel's own hue offset from the area's (limited, and only where both are
//!   clearly coloured), so a warmer light on one side stays warmer.
//!
//! The GPU kernel (`finish.wgsl`, `recolor`) repeats these steps and constants.

use lightcraft_color::perceptual::{lab_to_lch, oklab_from_2020};
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_color::{REC2020, SRGB};
use lightcraft_develop::LocalAdjustments;

/// Below this mean chroma an area counts as grey: its chroma comes from its shading, not its own chroma.
pub const GREY_CHROMA: [f32; 2] = [0.02, 0.06];
/// Most a pixel's chroma may exceed the area's mean, as a factor: a ratio above 1 approaches it softly
/// ([`chroma_ratio`]), so a bright streak doesn't turn neon.
pub const MAX_CHROMA_RATIO: f32 = 1.75;
/// Chroma over lightness (a saturation) below which a pixel of a coloured area counts as grey and stays grey (a
/// specular highlight, a white sole, a grey toe cap): from none at the first value to all of its ratio at the second.
pub const NEUTRAL_SAT: [f32; 2] = [0.03, 0.09];
/// Most a pixel's hue may keep of its offset from the area's mean hue (radians).
pub const MAX_HUE_OFFSET: f32 = 0.5;
/// The lightness range over which a grey area's new chroma fades out towards white.
pub const WHITE_FADE: [f32; 2] = [0.92, 1.0];
/// Smallest reference lightness (an all-black area still has a defined proportion).
pub const MIN_REF_L: f32 = 0.02;
/// Number of terms a mask's recolor carries in the per-mask term block ([`crate::finish::mask_terms`]): strength,
/// target OkLab (3), reference lightness, chroma, and the cosine and sine of its hue.
pub const TERMS: usize = 8;
/// Long edge of the proxy [`references`] measures the areas on: fixed, so every render size gets the same numbers.
pub const PROXY_EDGE: usize = 384;
/// Least total alpha (in proxy pixels) an area needs for a reference: a mask with less recolors nothing.
pub const MIN_WEIGHT: f32 = 4.0;

/// The colour of a masked area before recoloring, in OkLCh terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reference {
    /// Mean OkLab lightness.
    pub l: f32,
    /// Mean OkLab chroma.
    pub c: f32,
    /// Cosine and sine of the mean hue (the direction of the mean `a, b`).
    pub h: [f32; 2],
}

/// Is the recolor of `j` on (an amount above 0)?
pub fn is_on(j: &LocalAdjustments) -> bool {
    j.recolor.is_finite() && j.recolor > 0.0
}

/// The target colour of `j` (sRGB-encoded 0..1 channels) as OkLab of display-linear Rec.2020.
pub fn target_lab(j: &LocalAdjustments) -> [f32; 3] {
    srgb_to_oklab([j.recolor_r, j.recolor_g, j.recolor_b])
}

/// OkLab of an sRGB-encoded colour (channels clamped to 0..1, NaN as 0).
pub fn srgb_to_oklab(rgb: [f64; 3]) -> [f32; 3] {
    let lin = rgb.map(|v| srgb_to_linear(if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 }));
    oklab_from_2020(SRGB.to_space(&REC2020).apply_f32(lin))
}

/// `"#7a3fb0"` (or `"7a3fb0"`, `"#73b"`) as sRGB-encoded 0..1 channels.
pub fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() {
        return None;
    }
    let digits: Vec<u8> = match h.len() {
        3 => h.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8)).collect::<Option<_>>()?,
        6 => (0..3).map(|i| h.get(2 * i..2 * i + 2).and_then(|p| u8::from_str_radix(p, 16).ok())).collect::<Option<_>>()?,
        _ => return None,
    };
    let [r, g, b] = digits.as_slice() else { return None };
    Some([*r, *g, *b].map(|v| f64::from(v) / 255.0))
}

/// sRGB-encoded 0..1 channels as `"#rrggbb"` (out-of-range and NaN channels clamped).
pub fn to_hex(rgb: [f64; 3]) -> String {
    let [r, g, b] = rgb.map(|v| (if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 } * 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `a` wrapped to −π..π.
#[inline]
fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(t) - std::f32::consts::PI
}

/// The new lightness of a pixel of lightness `l` in an area of mean `lr`, recolored to lightness `lt`.
#[inline]
pub fn lightness(l: f32, lr: f32, lt: f32) -> f32 {
    let lr = lr.max(MIN_REF_L);
    let lt = lt.max(0.0);
    if l <= lr {
        return lt * l / lr;
    }
    // above the mean: proportional, but never so steep that white passes white
    let s = (lt / lr).min((1.0 - lt).max(0.0) / (1.0 - lr).max(1e-3));
    lt + (l - lr) * s
}

/// A pixel's chroma `c` (at lightness `l`) as a proportion of its area's mean chroma `rc`: softly limited to
/// [`MAX_CHROMA_RATIO`] above 1, and 0 for a pixel that is grey for its lightness ([`NEUTRAL_SAT`]).
#[inline]
pub fn chroma_ratio(c: f32, rc: f32, l: f32) -> f32 {
    if rc <= 1e-6 {
        return 0.0;
    }
    let r = (c / rc).max(0.0);
    let span = MAX_CHROMA_RATIO - 1.0;
    let r = if r > 1.0 { 1.0 + span * (1.0 - (-(r - 1.0) / span).exp()) } else { r };
    r * smooth(NEUTRAL_SAT[0], NEUTRAL_SAT[1], c / l.max(0.05))
}

/// The pixel `lab` (OkLab) of an area with reference `r`, recolored to `target` (OkLab) at strength `k` (0..1;
/// 0 or less returns `lab` unchanged, exactly).
pub fn apply(lab: [f32; 3], target: [f32; 3], r: &Reference, k: f32) -> [f32; 3] {
    if k.is_nan() || k <= 0.0 {
        return lab;
    }
    let k = k.min(1.0);
    let [l, c, h] = lab_to_lch(lab);
    let [lt, ct, ht] = lab_to_lch(target);
    let l1 = lightness(l, r.l, lt);
    // chroma: the pixel's proportion of the area's, or (a grey area) its lightness proportion
    let coloured = smooth(GREY_CHROMA[0], GREY_CHROMA[1], r.c);
    let by_chroma = chroma_ratio(c, r.c, l);
    let by_light = if lt > 1e-6 { (l1 / lt).clamp(0.0, 1.5) } else { 1.0 };
    let grey = by_light * (1.0 - smooth(WHITE_FADE[0], WHITE_FADE[1], l1));
    let c1 = ct * (grey + (by_chroma - grey) * coloured);
    // hue: the target's, keeping the pixel's own offset from the area's where both are clearly coloured
    let hr = r.h[1].atan2(r.h[0]);
    let keep = coloured * smooth(GREY_CHROMA[0], GREY_CHROMA[1], c);
    let h1 = ht + (wrap(h - hr) * keep).clamp(-MAX_HUE_OFFSET, MAX_HUE_OFFSET);
    let (s, co) = h1.sin_cos();
    let new = [l1, c1 * co, c1 * s];
    let out = [lab[0] + (new[0] - lab[0]) * k, lab[1] + (new[1] - lab[1]) * k, lab[2] + (new[2] - lab[2]) * k];
    if out.iter().all(|v| v.is_finite()) { out } else { lab }
}

/// Running sums of an area's colour, for [`Reference`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Sums {
    w: f64,
    l: f64,
    c: f64,
    a: f64,
    b: f64,
}

impl Sums {
    /// Adds pixel `lab` (OkLab) with weight `w` (its alpha; non-positive and non-finite ones are skipped).
    pub fn add(&mut self, lab: [f32; 3], w: f32) {
        if w.is_nan() || w <= 0.0 || w.is_infinite() || !lab.iter().all(|v| v.is_finite()) {
            return;
        }
        let w = f64::from(w);
        let [l, a, b] = lab.map(f64::from);
        self.w += w;
        self.l += w * l;
        self.c += w * a.hypot(b);
        self.a += w * a;
        self.b += w * b;
    }

    /// The area's reference (`None`: less than `min_weight` of alpha in all).
    pub fn reference(&self, min_weight: f32) -> Option<Reference> {
        if self.w.is_nan() || self.w < f64::from(min_weight) || self.w <= 0.0 {
            return None;
        }
        let (a, b) = (self.a / self.w, self.b / self.w);
        let n = a.hypot(b);
        let h = if n > 1e-9 { [(a / n) as f32, (b / n) as f32] } else { [1.0, 0.0] };
        Some(Reference { l: (self.l / self.w) as f32, c: (self.c / self.w) as f32, h })
    }
}

/// The recolor terms of mask adjustments `j` at Amount `amount` (1 = 100 %) with reference `r`: zeros (off) without
/// a reference or with nothing to do. Layout: [`TERMS`].
pub fn terms(j: &LocalAdjustments, amount: f32, r: Option<&Reference>) -> [f32; TERMS] {
    let (Some(r), true) = (r, is_on(j) && amount > 0.0) else { return [0.0; TERMS] };
    let k = ((j.recolor / 100.0) as f32).clamp(0.0, 1.0) * amount;
    let t = target_lab(j);
    [k, t[0], t[1], t[2], r.l, r.c, r.h[0], r.h[1]]
}

/// Recolors `lab` with the terms `t` ([`terms`]) at mask alpha `alpha`.
#[inline]
pub fn apply_terms(lab: [f32; 3], t: &[f32], alpha: f32) -> [f32; 3] {
    let (Some(&k), Some(target), Some(rest)) = (t.first(), t.get(1..4), t.get(4..8)) else { return lab };
    if k.is_nan() || k <= 0.0 || alpha.is_nan() || alpha <= 0.0 {
        return lab;
    }
    let target = [target[0], target[1], target[2]];
    let r = Reference { l: rest[0], c: rest[1], h: [rest[2], rest[3]] };
    apply(lab, target, &r, k * alpha)
}

/// Recolors every pixel of `img` (sRGB-encoded 0..1, row-major RGB) under `alpha` (0..1 per pixel) to `target`
/// (sRGB-encoded 0..1) at `amount` (0..1), measuring the area's reference on the image itself. What the
/// `recolor` example runs on a photo and a mask, outside the app.
pub fn recolor_srgb(img: &mut [[f32; 3]], alpha: &[f32], target: [f64; 3], amount: f32) {
    let fwd = SRGB.to_space(&REC2020);
    let to_lab = |c: [f32; 3]| oklab_from_2020(fwd.apply_f32(c.map(srgb_to_linear)));
    let mut sums = Sums::default();
    for (px, a) in img.iter().zip(alpha) {
        sums.add(to_lab(*px), *a);
    }
    let Some(r) = sums.reference(MIN_WEIGHT) else { return };
    let t = srgb_to_oklab(target);
    let back = REC2020.to_space(&SRGB);
    for (px, a) in img.iter_mut().zip(alpha) {
        if a.is_nan() || *a <= 0.0 {
            continue;
        }
        let lab = apply(to_lab(*px), t, &r, amount * a);
        let lin = back.apply_f32(lightcraft_color::perceptual::oklab_to_2020(lab));
        *px = lin.map(|v| lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)));
    }
}

/// Scene-linear `c` (exposed) as the per-pixel stage has it where it recolors, leaving out the local tone (Contrast,
/// Shadows, … by area) and the global Clarity and Texture: calibration, the tone map with its highlight
/// desaturation, process V4's Contrast and Blacks and the colour stage (with the mask's own Saturation and Hue).
fn display_of(fp: &crate::finish::FinishParams, c: [f32; 3], l_sat: f32, l_hue: f32) -> [f32; 3] {
    let c = if fp.calib.is_some() || fp.shadow_tint != 0.0 { crate::colorops::calibrate(c, fp.calib.as_ref(), fp.shadow_tint) } else { c };
    let yl = lightcraft_color::luminance_2020(c);
    let o = fp.tone.apply(yl);
    let mut d = if yl > 1e-9 { c.map(|v| v * o / yl) } else { [0.0; 3] };
    let k = fp.tone.chroma_scale(o);
    if k != 1.0 {
        d = d.map(|v| o + (v - o) * k);
    }
    let mx = d[0].max(d[1]).max(d[2]);
    if mx > fp.peak {
        let t = ((mx - fp.peak) / (mx - o).max(1e-6)).clamp(0.0, 1.0);
        d = d.map(|v| v + (o - v) * t);
    }
    if let Some(lo) = &fp.light {
        d = lo.apply(d, &lightcraft_color::transfer::linear_to_srgb);
    }
    fp.ops.apply(d, l_sat, l_hue)
}

/// The reference of every visible mask whose recolor is on (by mask id), measured on a [`PROXY_EDGE`] render of the
/// whole `frame` of `src`: the white-balanced proxy through the per-pixel stage's exposure (global and the mask's
/// own), calibration, tone map and colour stage, as [`crate::finish`] sees it at the point it recolors. The proxy
/// size doesn't depend on the request, so every render of one edit (preview, export, CPU or GPU) gets the same.
pub(crate) fn references(
    src: &lightcraft_raster::Rgb32f,
    info: &crate::SourceInfo,
    s: &lightcraft_develop::DevelopSettings,
    frame: &crate::geometry::Frame,
    space: crate::output::OutputSpace,
) -> Vec<(u32, Reference)> {
    let on: Vec<lightcraft_develop::Mask> =
        s.masks.iter().filter(|m| m.visible && !m.components.is_empty() && is_on(&m.adjust) && m.adjust.amount > 0.0).cloned().collect();
    if on.is_empty() || src.width == 0 || src.height == 0 {
        return Vec::new();
    }
    let (pw, ph) = frame.fit(PROXY_EDGE, PROXY_EDGE);
    let mut img = lightcraft_raster::Rgb32f::clone(&crate::local::frame_proxy(src, frame, pw, ph));
    crate::local::white_balance(&mut img, info, s);
    let log_l = img.map(crate::local::log_lum);
    let ev = s.light.exposure as f32;
    let evaluated = crate::masks::evaluate(&on, frame, pw, ph, &img, &log_l, ev, info.mattes.as_deref());
    let fp = crate::finish::FinishParams::new(crate::finish::FinishParamsInput {
        settings: s,
        frame,
        info,
        image_size: (pw, ph),
        px_per_long: frame.px_per_long(pw),
        air_pre: 1.0,
        space,
    });
    evaluated
        .iter()
        .filter_map(|e| {
            let t = crate::finish::effect_terms(&e.adjust, e.amount);
            let gain = fp.gain * t[0].exp2();
            let mut sums = Sums::default();
            for y in 0..ph {
                let Some((x0, row)) = e.alpha.row(y) else { continue };
                for (x, a) in row.iter().enumerate() {
                    if *a > 0.0
                        && let Some(c) = img.data.get(y * pw + x0 + x)
                    {
                        sums.add(oklab_from_2020(display_of(&fp, c.map(|v| v * gain), t[11], t[12])), *a);
                    }
                }
            }
            sums.reference(MIN_WEIGHT).map(|r| (e.id, r))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests_recolor.rs"]
mod tests;
