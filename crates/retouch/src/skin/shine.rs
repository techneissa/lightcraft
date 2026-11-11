//! Shine: oily highlights brought down toward the skin around them.
//!
//! A highlight is skin brighter than the skin right around it ([`AROUND`] of the face) and paler than it: the
//! light's own colour on top of the skin's. The skin around a pixel is measured without the shine, the beard and
//! the shadows: a first blur gives each place's usual light and how much it varies, and a second blur counts only
//! the skin within about that variation of the usual light. So a lit cheek, a lit side of the face or the nose's
//! lit bridge (broad, and in the skin's own colour) stays, while a spot on the forehead, the nose or a cheekbone
//! comes down.
//!
//! The change moves the spot's light and its colour toward that skin: a highlight is the light's colour, often a
//! little blue from a window or the sky, so taking grey light away would leave it lilac. [`KEEP`] of its excess
//! stays at 100, so the skin does not go matte-flat, and the pores in it are calmed by up to [`PORES_CALM`] (the
//! glints on them are shine too).
//!
//! Issue #209 replaced a version that took grey light away where the skin was brighter than a blur a tenth of the
//! face wide: lit cheeks and noses went dark and pink or purple, and the real shine kept a lilac cast.

use lightcraft_raster::{Image, par_rows};

use super::grid::Reduced;
use super::{YB, YR};

/// The skin a highlight is measured against, a Gaussian's sigma in fractions of the face: wider than a spot of
/// shine, narrower than the light falling on one side of the face.
pub const AROUND: f64 = 0.05;
/// Skin brighter than the skin around it by this (natural log: 1.08×) starts to count as shine ...
const START: f32 = 0.08;
/// ... and from this (1.5×) counts fully.
const FULL: f32 = 0.4;
/// The share of a highlight's excess Shine 100 keeps: a little sheen.
pub const KEEP: f32 = 0.25;
/// How much of the pores' contrast a highlight taken away fully loses.
const PORES_CALM: f32 = 0.4;
/// A highlight as saturated as the skin around it counts this much (light in the skin's own colour is rarely
/// shine); one that has lost a quarter of the skin's saturation or more counts fully.
const SAME_COLOUR: f32 = 0.25;
/// Skin around a highlight with less saturation than this is taken as grey (a black-and-white photo): there,
/// brightness alone finds the shine.
const GREY: f32 = 0.04;
/// How far the shine's own edge is softened, a Gaussian's sigma in fractions of the face.
pub const SOFT: f64 = 0.008;
/// The least spread of light (natural log) the skin around a pixel is taken to have: skin within about this of
/// its usual light is counted in the second blur even where it is very even.
const SPREAD: f32 = 0.04;

/// The skin around each pixel without its shine, where the shine is, and the slider's strength.
pub(super) struct Shine {
    /// `[log luminance, red, blue]` of the skin around (weighted, read with [`Reduced::read`]).
    around: Reduced,
    /// How much each pixel is shine (0…1, in the first channel), blurred by [`SOFT`] so its edge follows the
    /// highlight's without drawing a line where a pale highlight meets skin of a deeper colour.
    shine: Reduced,
    /// How much of a full highlight's excess is taken away (0…1 − [`KEEP`]).
    take: f32,
}

/// What Shine does to one pixel: the change of its pore-free skin (`[log luminance, red, blue]`) and the factor its
/// pores' contrast is scaled by.
pub(super) struct Change {
    pub(super) shift: [f32; 3],
    pub(super) pores: f32,
}

impl Shine {
    /// Shine `amount` (0…1) over `base`, the pore-free skin (`[log luminance, red, blue, mask]`, not weighted), with
    /// the skin around measured over `sigma` pixels and the shine's edge softened over `soft` pixels. `None` at 0.
    pub(super) fn new(base: &Image<[f32; 4]>, [sigma, soft]: [f32; 2], amount: f32) -> Option<Shine> {
        if amount.is_nan() || amount <= 0.0 || !sigma.is_finite() {
            return None;
        }
        let w = base.width.max(1);
        // the usual light at each place, and how much it varies
        let mut moments = Image::<[f32; 4]>::new(base.width, base.height);
        par_rows(&mut moments.data, w, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let p = base.data.get(y * w + x).copied().unwrap_or_default();
                *o = [p[0] * p[3], p[0] * p[0] * p[3], 0.0, p[3]];
            }
        });
        let usual = Reduced::new(&moments, sigma).blur(sigma);
        drop(moments);
        // the skin within about that variation of the usual light: no shine, beard or shadow
        let mut kept = Image::<[f32; 4]>::new(base.width, base.height);
        par_rows(&mut kept.data, w, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let p = base.data.get(y * w + x).copied().unwrap_or_default();
                if p[3] <= 0.0 {
                    continue;
                }
                let [m, m2, _] = usual.read(&usual.taps(x, y), [p[0], p[0] * p[0], 0.0]);
                let spread = (m2 - m * m).max(0.0).sqrt() + SPREAD;
                let z = (p[0] - m) / spread;
                let wt = p[3] * (-0.5 * z * z).exp();
                *o = [p[0] * wt, p[1] * wt, p[2] * wt, wt];
            }
        });
        let around = Reduced::new(&kept, sigma).blur(sigma);
        drop(kept);
        // how much each pixel is shine, softened
        let mut shine = Image::<[f32; 4]>::new(base.width, base.height);
        par_rows(&mut shine.data, w, |y, row| {
            for (x, o) in row.iter_mut().enumerate() {
                let p = base.data.get(y * w + x).copied().unwrap_or_default();
                if p[3] > 0.0 {
                    let own = [p[0], p[1], p[2]];
                    *o = [p[3] * how_much(own, around.read(&around.taps(x, y), own)), 0.0, 0.0, p[3]];
                }
            }
        });
        let shine = Reduced::blurred(&shine, soft);
        Some(Shine { around, shine, take: curve(amount) })
    }

    /// The change at pixel (`x`, `y`), whose pore-free skin is `own` (`[log luminance, red, blue]`).
    pub(super) fn at(&self, x: usize, y: usize, own: [f32; 3]) -> Change {
        let around = self.around.read(&self.around.taps(x, y), own);
        let over = own[0] - around[0];
        let k = self.take * self.shine.read(&self.shine.taps(x, y), [0.0; 3])[0].clamp(0.0, 1.0);
        if over.is_nan() || k.is_nan() || over <= 0.0 || k <= 0.0 {
            return Change { shift: [0.0; 3], pores: 1.0 };
        }
        Change { shift: [-k * over, k * (around[1] - own[1]), k * (around[2] - own[2])], pores: 1.0 - PORES_CALM * k / (1.0 - KEEP) }
    }
}

/// How much skin `own` (`[log luminance, red, blue]`) is shine against the skin `around` it: 0…1, rising from
/// [`START`] to [`FULL`] brighter, scaled down to [`SAME_COLOUR`] when it is as saturated as that skin.
fn how_much(own: [f32; 3], around: [f32; 3]) -> f32 {
    let over = own[0] - around[0];
    if over.is_nan() || over <= START {
        return 0.0;
    }
    let t = ((over - START) / (FULL - START)).clamp(0.0, 1.0);
    let bright = t * t * (3.0 - 2.0 * t);
    let skin = saturation(around);
    let pale = if skin < GREY { 1.0 } else { (SAME_COLOUR + (1.0 - saturation(own) / skin) * 3.0).clamp(SAME_COLOUR, 1.0) };
    bright * pale
}

/// The slider (0…1) as the share of a highlight's excess taken away: fast at first (`1 − (1 − s)^1.5`, 65 % of the
/// way at 50), up to all but [`KEEP`] at 100.
fn curve(s: f32) -> f32 {
    let s = s.clamp(0.0, 1.0);
    (1.0 - (1.0 - s).powf(1.5)) * (1.0 - KEEP)
}

/// The saturation of a colour given as red and blue over luminance (`[_, red, blue]`): its channels' spread over
/// the largest. 0 for grey or a colour that is not one.
fn saturation(p: [f32; 3]) -> f32 {
    let (r, b) = (p[1], p[2]);
    let g = (1.0 - YR * r - YB * b) / super::Y[1];
    let (hi, lo) = (r.max(g).max(b), r.min(g).min(b));
    if hi > 1e-3 && hi.is_finite() && lo.is_finite() { ((hi - lo) / hi).clamp(0.0, 1.0) } else { 0.0 }
}
