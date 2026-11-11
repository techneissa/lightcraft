//! Whites: the sclera brighter and cleaner, its shading kept.
//!
//! Three things, all in proportion to the slider:
//! - **Lift**: the whites get brighter by a gain (in stops), so their shading (the corners, the shadow under the upper
//!   lid) keeps its ratio to the rest and the eye stays round. The gain depends on how the whites sit against the
//!   face's lit skin: a white in shadow (deep-set eyes, a low-key photo) gets more, one already bright less. A soft
//!   shoulder keeps the brightest parts below paper white, and nothing ever gets darker.
//! - **Clean**: veins and blotches (darker than the white around them, at a small scale) are lifted toward it, and
//!   most of the colour goes, so a red or yellow cast turns a clean neutral white and veins stay as faint grey lines.
//! - **Where**: the Eye Whites mask, cut to the eye's opening, faded toward the corners (the inner one has the pink
//!   caruncle), and only where a pixel is light enough to be white (lashes and the lid's edge are left alone).

use lightcraft_raster::{Image, Rgb32f};

use super::super::geom::{Eye, eye_scale, smoothstep, window};
use super::super::grid::{blur, luma, sample};
use super::{Patch, opening, quantile, response, weighted_blur};
use crate::{Part, Scene};

/// The grid (across, down) the whites' level is measured on, over the Eye Whites mask's area.
const LEVEL_GRID: (usize, usize) = (96, 32);
/// The whites' level: this share of their pixels is darker. Read where the mask is at least `LEVEL_MASK`: its soft
/// edge is part lid or skin.
const WHITES_LEVEL: f32 = 0.9;
const LEVEL_MASK: f32 = 0.8;
/// The grid the face's lit skin is measured on, over the Facial Skin mask's area, and the share of it darker than
/// "lit skin".
const SKIN_GRID: usize = 48;
const SKIN_LEVEL: f32 = 0.9;
/// The whites aim for this share of the lit skin's level.
const TARGET: f32 = 0.85;
/// The lift at 100, in stops: `LIFT_BASE` plus `LIFT_PULL` of the way (in stops) to the aim, within `LIFT_RANGE`.
const LIFT_BASE: f32 = 0.85;
const LIFT_PULL: f32 = 0.5;
const LIFT_RANGE: (f32, f32) = (0.85, 1.75);
/// The lift of a face without Facial Skin, in stops at 100.
const LIFT_NO_SKIN: f32 = 0.85;
/// Above this linear level the lifted whites roll off toward `CEILING` (about 238 of 255): bright, never paper white.
const KNEE: f32 = 0.45;
const CEILING: f32 = 0.86;
/// A pixel this share of the whites' level (a lash, the lid's edge) is left alone; one at the upper share is wholly
/// changed. The shadowed white under the lid is well above the lower one.
const GATE: (f64, f64) = (0.12, 0.3);
/// The scale veins are measured at (the white around a vein), in half eye widths.
const VEIN_SIGMA: f64 = 0.05;
/// How much of a vein's depth goes at 100 (as a power: 0.6 takes a vein at 70 % of its surround to 87 %).
const EVEN: f32 = 0.6;
/// A spot darker than this share of its surround is not a vein (a lash, the iris's edge): not evened.
const VEIN_DEPTH: (f32, f32) = (0.35, 0.6);
/// How much of the whites' colour goes at 100.
const DESAT: f32 = 0.85;
/// Across the eye (in half widths, −1 the outer corner, +1 the inner), where the change is whole and where it has
/// faded out: the corners stay as they were, the caruncle at the inner one too.
const ACROSS: (f64, f64, f64, f64) = (-1.08, -0.72, 0.62, 0.95);
/// The opening's edge for the whites ([`opening::Fade`]), at the upper lid and at the lower one: the lower lid's wet
/// rim is pale and lies just inside the mesh's outline, so the change starts further in there.
const FADE: opening::Fade = (0.03, 0.04);
const FADE_LOWER: opening::Fade = (0.0, 0.1);
/// The scale (in half eye widths, at least a pixel) the gate reads the luminance at: grain in a dark, noisy white
/// gets the same change as the white around it instead of staying behind as dark specks.
const GATE_SIGMA: f64 = 0.02;
/// Within this many half eye widths of the opening's outline (fading out at the second), a pixel whose saturation is
/// this share of the lit skin's is starting to be the lid's rim (the mesh's outline can sit a little inside the lid);
/// at the upper share it is the rim and left alone. Only there: a red or yellow white can be as saturated as skin.
const LID_ZONE: (f64, f64) = (0.04, 0.12);
const SKIN_LIKE: (f64, f64) = (0.75, 1.0);
/// How much of the face's light (the lit skin's colour) a cleaned white keeps: a white in warm light stays a little
/// warm instead of turning grey-blue beside the skin.
const TINT: f32 = 0.15;

/// Whites, `amount` 0…1.
pub fn whites(img: &mut Rgb32f, scene: &Scene<'_>, face: usize, eyes: &[Eye], amount: f32) {
    let Some(ctx) = scene.faces.get(face) else { return };
    let Some(mask) = ctx.face.mask(Part::EyeWhites) else { return };
    let Some(bx) = scene.mask_box(face, Part::EyeWhites) else { return };
    let openings = opening::of(ctx, eyes);
    let view = scene.view();
    // the whites' level, from a fixed grid over the whole mask on the photo: the same whatever part of the face a
    // render (a zoomed loupe) holds, and at every size
    let a = mask.area;
    let mut ys = Vec::with_capacity(LEVEL_GRID.0 * LEVEL_GRID.1);
    for j in 0..LEVEL_GRID.1 {
        for i in 0..LEVEL_GRID.0 {
            let (nx, ny) = (a.x0 + (i as f64 + 0.5) / LEVEL_GRID.0 as f64 * a.width(), a.y0 + (j as f64 + 0.5) / LEVEL_GRID.1 as f64 * a.height());
            if mask.sample(nx, ny) < LEVEL_MASK {
                continue;
            }
            let Some((x, y)) = view.to_px(nx, ny) else { continue };
            if opening::fade(opening::depth(&openings, x, y), FADE, FADE_LOWER) < 0.5 {
                continue;
            }
            if let Some(v) = sample(img, x, y).map(luma).filter(|v| v.is_finite()) {
                ys.push(v);
            }
        }
    }
    let Some(level) = quantile(&mut ys, WHITES_LEVEL).filter(|l| *l > 1e-6) else { return };
    let k = response(amount);
    let light = lit_skin(img, scene, face);
    let stops = light.map_or(LIFT_NO_SKIN, |(l, _)| lift(level, l));
    let gain = (k * stops).exp2();
    let even = k * EVEN;
    let keep = 1.0 - DESAT * k;
    // the clean white's colour: neutral, with a little of the face's light
    let tint = light.map_or([1.0; 3], |(_, c)| c.map(|v| 1.0 + TINT * (v - 1.0)));
    let half = eye_scale(ctx, eyes);

    // the patch: the mask's box and the veins' blur around it
    let pad = (3.0 * VEIN_SIGMA * half).ceil() as usize + 1;
    let Some(patch) = Patch::grown(bx, pad, img.width, img.height) else { return };
    let n = patch.w * patch.h;
    let lum: Vec<f32> =
        (0..n).map(|j| img.data.get(patch.image_index(j, img.width)).map(|p| luma(*p)).filter(|l| l.is_finite()).unwrap_or(0.0)).collect();
    let rgb = Image::<[f32; 3]>::from_fn(patch.w, patch.h, |x, y| {
        img.data.get(patch.image_index(y * patch.w + x, img.width)).copied().filter(|p| p.iter().all(|v| v.is_finite())).unwrap_or([0.0; 3])
    });
    let soft = blur(&rgb, (GATE_SIGMA * half).max(1.0)).data;
    let skin_sat = light.map(|(_, c)| spread(c));
    let wgt: Vec<f32> = (0..n)
        .map(|j| {
            let (x, y) = patch.centre(j);
            let (nx, ny) = view.to_norm(x, y);
            let m = mask.sample(nx, ny);
            if m <= 0.0 {
                return 0.0;
            }
            let q = soft.get(j).copied().unwrap_or([0.0; 3]);
            let l = luma(q);
            let mut gate = smoothstep(GATE.0 * level as f64, GATE.1 * level as f64, l as f64) as f32;
            // by the lids, a pixel as saturated as the skin is the lid's rim, not white
            let depth = opening::depth(&openings, x, y);
            if let (Some(ss), Some(depth)) = (skin_sat.filter(|v| *v > 1e-3), depth.map(|d| d.d))
                && l > 1e-6
            {
                let near = 1.0 - smoothstep(LID_ZONE.0, LID_ZONE.1, depth);
                let sat = spread(q.map(|v| v / l)) / ss;
                gate *= 1.0 - (near * smoothstep(SKIN_LIKE.0, SKIN_LIKE.1, sat as f64)) as f32;
            }
            let w = m * gate * opening::fade(depth, FADE, FADE_LOWER) * across(eyes, x, y);
            if w.is_finite() { w.clamp(0.0, 1.0) } else { 0.0 }
        })
        .collect();
    let base = weighted_blur(&lum, &wgt, patch.w, patch.h, VEIN_SIGMA * half);
    for j in 0..n {
        let (Some(&w), Some(&l), Some(&b)) = (wgt.get(j), lum.get(j), base.get(j)) else { continue };
        if w <= 0.0 || l <= 0.0 {
            continue;
        }
        let Some(p) = img.data.get_mut(patch.image_index(j, img.width)) else { continue };
        // a vein's depth against the white around it, partly lifted
        let r = if b > 1e-6 { (l / b).min(1.0) } else { 1.0 };
        let vein = if r < 1.0 { r.powf(-even * smoothstep(VEIN_DEPTH.0 as f64, VEIN_DEPTH.1 as f64, r as f64) as f32) } else { 1.0 };
        let s = shoulder(l * gain * vein).max(l) / l;
        let lit = p.map(|c| c * s);
        let ly = luma(lit);
        let out: [f32; 3] = std::array::from_fn(|c| {
            let white = ly * tint[c];
            let clean = white + (lit[c] - white) * keep;
            p[c] + w * (clean - p[c])
        });
        if out.iter().all(|v| v.is_finite()) {
            *p = out;
        }
    }
}

/// The face's lit skin: its level (luminance) and its colour (each channel over the luminance), read before any
/// change on a fixed grid over the Facial Skin mask; `None` without that mask.
fn lit_skin(img: &Rgb32f, scene: &Scene<'_>, face: usize) -> Option<(f32, [f32; 3])> {
    let skin = scene.faces.get(face)?.face.mask(Part::FacialSkin)?;
    let a = skin.area;
    let mut px = Vec::with_capacity(SKIN_GRID * SKIN_GRID);
    for j in 0..SKIN_GRID {
        for i in 0..SKIN_GRID {
            let (nx, ny) = (a.x0 + (i as f64 + 0.5) / SKIN_GRID as f64 * a.width(), a.y0 + (j as f64 + 0.5) / SKIN_GRID as f64 * a.height());
            if skin.sample(nx, ny) < 0.5 {
                continue;
            }
            if let Some(p) = scene.view().to_px(nx, ny).and_then(|(x, y)| sample(img, x, y)).filter(|p| p.iter().all(|v| v.is_finite())) {
                px.push(p);
            }
        }
    }
    let mut ys: Vec<f32> = px.iter().map(|p| luma(*p)).collect();
    let level = quantile(&mut ys, SKIN_LEVEL).filter(|l| *l > 1e-6)?;
    // the colour of the lit half
    let (mut sum, mut count) = ([0.0f32; 3], 0usize);
    for p in px.iter().filter(|p| luma(**p) >= 0.5 * level) {
        sum = std::array::from_fn(|c| sum[c] + p[c]);
        count += 1;
    }
    let y = luma(sum);
    let colour = if count > 0 && y > 1e-6 { sum.map(|v| (v / y).clamp(0.0, 4.0)) } else { [1.0; 3] };
    Some((level, colour))
}

/// The lift at 100, in stops, for whites at `level` in a face whose lit skin is at `skin`: more for whites in shadow.
fn lift(level: f32, skin: f32) -> f32 {
    let s = LIFT_BASE + LIFT_PULL * (TARGET * skin / level).log2();
    if s.is_finite() { s.clamp(LIFT_RANGE.0, LIFT_RANGE.1) } else { LIFT_NO_SKIN }
}

/// The weight across the eye at pixel (`x`, `y`): 1 in the middle, fading out at the corners (by the nearer eye).
fn across(eyes: &[Eye], x: f64, y: f64) -> f32 {
    let nearest = eyes.iter().map(|e| e.local(x, y)).min_by(|p, q| (p.0.abs() + p.1.abs()).total_cmp(&(q.0.abs() + q.1.abs())));
    match nearest {
        Some((a, _)) => window(a, ACROSS.0, ACROSS.1, ACROSS.2, ACROSS.3) as f32,
        None => 1.0,
    }
}

/// The spread of a colour's channels (its saturation, for a colour over its luminance).
fn spread(c: [f32; 3]) -> f32 {
    c.iter().fold(f32::MIN, |a, v| a.max(*v)) - c.iter().fold(f32::MAX, |a, v| a.min(*v))
}

/// A soft roll-off above [`KNEE`] toward [`CEILING`].
fn shoulder(v: f32) -> f32 {
    if v <= KNEE {
        return v;
    }
    let room = CEILING - KNEE;
    KNEE + room * (1.0 - (-(v - KNEE) / room).exp())
}
