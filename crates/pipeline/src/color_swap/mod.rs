//! Fork (issue #460, `fork/color-swap.md`): Recolor, which replaces one colour with another across the whole photo.
//!
//! It works on display-linear colour in OkLab, after the colour stage and the masks' Recolor and before the vignette
//! (the stage `crate::finish` runs a pixel through), and the GPU kernel (`color_swap.wgsl`) repeats it step by step.
//! Entries run in order: each sees what the one before it made, so a chain (blue to red, then red to green) works as
//! written.
//!
//! **Which pixels** ([`SwapK::weight`], 0..1): the distance of a pixel from the From colour in three terms, each over
//! its own half-width: the hue (an angle, ignored when the From colour is nearly grey), the saturation as
//! chroma over lightness, and the lightness. Chroma over lightness stays the same when light falls on a surface more
//! or less strongly (OkLab scales with the cube root of light), so a shirt's shadows and highlights are as much "the
//! shirt" as its middle, which is what lets a swap work in light and shadow. The three terms join as one distance, and
//! the weight is 1 up to [`SwapK::edge`] (a share of the distance) and falls to 0 at the distance 1 with a smoothstep:
//! no step anywhere in the colour space, so no banding in a gradient and no halo at an edge. Range scales all three
//! half-widths, the per-term ranges scale one each, and Edge Softness moves where the fade starts.
//!
//! **What changes**: [`crate::recolor::shade`], the part of mask Recolor that keeps shading: the pixel's chroma
//! relative to the From colour's becomes relative to the To colour's, its hue offset from the From hue (a little)
//! carries over, and its lightness is kept (or, with Keep Lightness off, follows the To colour's, in proportion). The
//! change is mixed in by weight × Intensity.
//!
//! **The range view** ([`ColorSwaps::view`]): one entry's weight drawn instead of its change, what Option-hover shows.

use lightcraft_color::perceptual::{lab_to_lch, oklab_from_2020, oklab_to_2020};
use lightcraft_develop::{ColorSwap, DevelopSettings};

use crate::recolor::{self, Reference, Style, srgb_to_oklab, wrap};

/// Words per entry in the GPU parameter block ([`SwapK::words`]).
pub const SWAP_WORDS: usize = 16;

/// Hue half-width (radians) at Range 50 and a hue range of 0, and what the hue range adds at 100.
pub const HUE_WIDTH: [f32; 2] = [0.12, 0.7];
/// Saturation (chroma over lightness) half-width, the same two numbers.
pub const SAT_WIDTH: [f32; 2] = [0.05, 0.40];
/// Lightness half-width, the same two numbers.
pub const LUM_WIDTH: [f32; 2] = [0.15, 1.05];
/// A From colour with less chroma than the first value has no hue to select by; from the second it has all of it.
pub const FROM_COLOURED: [f32; 2] = [0.015, 0.05];
/// A pixel with less chroma than the first value is not of a coloured From colour whatever its hue says; from the
/// second its hue counts fully.
pub const PIXEL_COLOURED: [f32; 2] = [0.004, 0.03];
/// The weakly coloured pixels that take the new hue alone (see [`SwapK::membership`]): from none at the first chroma to
/// all at the second, and fading out again between the last two (a clearly coloured pixel either belongs or keeps its colour).
pub const WEAK_CHROMA: [f32; 4] = [0.008, 0.016, 0.03, 0.05];
/// A To colour with less chroma than the first value is neutral (white, grey); from the second it is not.
pub const NEUTRAL_CHROMA: [f32; 2] = [0.01, 0.03];
/// The range view dims what is outside to this share of its lightness and drops its colour.
pub const VIEW_DIM: f32 = 0.38;
/// The range view's edge glow: how far a pixel at the middle of the fade moves to the To colour.
pub const VIEW_GLOW: f32 = 0.3;
/// The colour of the range view's edge glow for an entry with no To colour yet (sRGB).
pub const VIEW_AMBER: [f64; 3] = [1.0, 0.72, 0.1];

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One entry resolved for the per-pixel stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwapK {
    /// The From colour: OkLab lightness, chroma, hue (radians), saturation (chroma over lightness).
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub sat: f32,
    /// Half-widths of the range: hue (radians), saturation, lightness.
    pub wh: f32,
    pub ws: f32,
    pub wl: f32,
    /// Where the fade starts, as a share of the distance (the weight is 1 inside it): `1 − (0.1 + 0.9 · softness)`.
    pub edge: f32,
    /// The To colour in OkLab.
    pub target: [f32; 3],
    /// Intensity, 0..1.
    pub k: f32,
    /// The pixel keeps its own lightness.
    pub keep: bool,
    /// How neutral the To colour is, 0..1 ([`NEUTRAL_CHROMA`]): a weakly coloured edge pixel then takes the To colour's
    /// chroma along with its hue, instead of keeping its own under the residue hue of white.
    pub neutral: f32,
}

impl SwapK {
    /// The entry's numbers for the per-pixel stage; `None` without a From colour. A missing To colour is the glow
    /// colour of the range view and an Intensity of 0.
    pub fn new(e: &ColorSwap) -> Option<SwapK> {
        let e = e.sanitized();
        let [l, c, h] = lab_to_lch(srgb_to_oklab(e.from?));
        let f = |v: f64| (v / 100.0) as f32;
        let reach = (f(e.range) * 2.0).max(0.05);
        let width = |[lo, span]: [f32; 2], share: f64| (lo + span * f(share)) * reach;
        let target = srgb_to_oklab(e.to.unwrap_or(VIEW_AMBER));
        Some(SwapK {
            l,
            c,
            h,
            sat: c / l.max(0.05),
            wh: width(HUE_WIDTH, e.hue_range),
            ws: width(SAT_WIDTH, e.sat_range),
            wl: width(LUM_WIDTH, e.lum_range),
            edge: 1.0 - (0.1 + 0.9 * f(e.softness)),
            target,
            k: if e.to.is_some() { f(e.intensity) } else { 0.0 },
            keep: e.keep_lightness,
            neutral: 1.0 - smooth(NEUTRAL_CHROMA[0], NEUTRAL_CHROMA[1], lab_to_lch(target)[1]),
        })
    }

    /// The GPU parameter words: from L, C, hue cosine, hue sine, saturation; the three half-widths, the fade's start;
    /// the To colour; Intensity, Keep Lightness, how neutral the To colour is; one spare.
    pub fn words(&self) -> [f32; SWAP_WORDS] {
        let (s, c) = self.h.sin_cos();
        let [tl, ta, tb] = self.target;
        let mut w = [0.0; SWAP_WORDS];
        w[..15].copy_from_slice(&[
            self.l,
            self.c,
            c,
            s,
            self.sat,
            self.wh,
            self.ws,
            self.wl,
            self.edge,
            tl,
            ta,
            tb,
            self.k,
            f32::from(u8::from(self.keep)),
            self.neutral,
        ]);
        w
    }

    /// How much a pixel of OkLCh `lch` belongs to this entry's range: `[full, hue]` (0..1, smooth), the weight of the
    /// whole change and the weight of a change of hue alone (never less than `full`). The hue alone is for the
    /// weakly coloured pixels between the From colour and something else (the grey-purple where a purple stripe
    /// meets an olive one): they take the new hue at their own chroma, so no thin line of the old hue is left
    /// along an edge ([`crate::recolor::membership`] does the same for a mask).
    pub fn membership(&self, lch: [f32; 3]) -> [f32; 2] {
        let [l, c, h] = lch;
        let coloured = smooth(FROM_COLOURED[0], FROM_COLOURED[1], self.c);
        let dh_angle = wrap(h - self.h).abs();
        let dh = dh_angle / self.wh * coloured;
        let ds = (c / l.max(0.05) - self.sat) / self.ws;
        let dl = (l - self.l) / self.wl;
        let d = (dh * dh + ds * ds + dl * dl).sqrt();
        // a nearly grey pixel has no hue to match a coloured From colour with
        let gate = 1.0 - coloured * (1.0 - smooth(PIXEL_COLOURED[0], PIXEL_COLOURED[1], c));
        let full = (1.0 - smooth(self.edge, 1.0, d)) * gate;
        let [c0, c1, c2, c3] = WEAK_CHROMA;
        let loose = 1.0 - smooth(self.wh * 1.2, self.wh * 2.0 + 0.1, dh_angle);
        let weak = smooth(c0, c1, c) * (1.0 - smooth(c2, c3, c));
        let in_light = 1.0 - smooth(0.5, 1.0, dl.abs());
        [full, full.max(loose * weak * coloured * in_light)]
    }

    /// How much a pixel belongs to this entry's range at all ([`Self::membership`]'s `full`).
    pub fn weight(&self, lch: [f32; 3]) -> f32 {
        self.membership(lch)[0]
    }

    /// The pixel `lab` (OkLab) after this entry.
    pub fn apply(&self, lab: [f32; 3]) -> [f32; 3] {
        if self.k <= 0.0 {
            return lab;
        }
        let lch = lab_to_lch(lab);
        let member = self.membership(lch);
        if member[1] <= 0.0 {
            return lab;
        }
        let (s, c) = self.h.sin_cos();
        let r = Reference { l: self.l, c: self.c, h: [c, s], spread: 0.0 };
        recolor::shade(lab, lch, self.target, &r, member, self.k, Style { keep_lightness: self.keep, neutral: self.neutral })
    }

    /// The range view of display-linear `d`: the photo dimmed and drained of colour where the entry does not reach,
    /// as it is where it does, with a glow in the To colour on the fade between, so the edge of the range reads as a line.
    pub fn view(&self, d: [f32; 3]) -> [f32; 3] {
        let lab = oklab_from_2020(d);
        let w = self.membership(lab_to_lch(lab))[1];
        let dim = [lab[0] * VIEW_DIM, 0.0, 0.0];
        let mut out: [f32; 3] = std::array::from_fn(|i| dim[i] + (lab[i] - dim[i]) * w);
        let glow = 4.0 * w * (1.0 - w) * VIEW_GLOW;
        let accent = [self.target[0].clamp(0.7, 0.95), self.target[1], self.target[2]];
        out = std::array::from_fn(|i| out[i] + (accent[i] - out[i]) * glow);
        oklab_to_2020(out)
    }
}

/// The entries of a render, in order, resolved once ([`crate::finish::FinishParams::swaps`]).
#[derive(Clone, Debug, Default)]
pub struct ColorSwaps {
    /// The entries that change the photo, up to [`lightcraft_develop::MAX_SWAPS`].
    pub entries: Vec<SwapK>,
    /// The entry whose range is drawn instead of its change; `entries` then holds the ones before it.
    pub view: Option<SwapK>,
}

impl ColorSwaps {
    pub fn new(s: &DevelopSettings) -> ColorSwaps {
        let mut out = ColorSwaps::default();
        if !s.section_enabled(lightcraft_develop::color_swap::SECTION) || crate::is_bw(s) {
            return out;
        }
        for e in s.color_swaps.iter().take(lightcraft_develop::MAX_SWAPS) {
            if e.view {
                out.view = SwapK::new(e);
                break;
            }
            if e.is_active()
                && let Some(k) = SwapK::new(e)
            {
                out.entries.push(k);
            }
        }
        out
    }

    /// Whether the stage changes nothing.
    pub fn is_identity(&self) -> bool {
        self.entries.is_empty() && self.view.is_none()
    }

    /// The display-linear Rec.2020 colour `d` after the stage (exactly `d` when [`Self::is_identity`]).
    #[inline]
    pub fn apply(&self, d: [f32; 3]) -> [f32; 3] {
        if self.is_identity() {
            return d;
        }
        let lab0 = oklab_from_2020(d);
        let mut lab = lab0;
        for e in &self.entries {
            lab = e.apply(lab);
        }
        // (a pixel no entry reaches keeps its own value, exactly, not the round trip through OkLab)
        let d = if lab == lab0 { d } else { oklab_to_2020(lab) };
        match &self.view {
            Some(v) => v.view(d),
            None => d,
        }
    }
}

#[cfg(test)]
mod tests;
