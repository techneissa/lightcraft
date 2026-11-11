//! Fork feature (`fork/portrait-effect-map.md`, issue #208): what a Portrait slider changes on this photo, the map its
//! ⌥-drag shows ([`crate::Overlay::RetouchEffect`]).
//!
//! The map is measured, never drawn by hand: the retouch step runs on the image before it twice, with the settings as
//! they are and with the dragged slider back at its default (for every face, or for the picked face alone), and each
//! pixel's value is how far apart the two results look (OkLab ΔE at the photo's exposure) on a fixed scale. Face
//! Reshape moves pixels instead: its map is how far the warp moves each point, as a share of its face's size. So the
//! map shows what the feature does on this face as its algorithm is now, grows with the value, and is 0 everywhere
//! with the slider at its default.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_color::perceptual::oklab_from_2020;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::{FEATURES, Input, MAX_FACES, Scope, Stage};
use lightcraft_geom::Point;
use lightcraft_raster::{Plane, Rgb32f};

use crate::kept_budget::{HOST, Kept};
use crate::retouch_step::{self, FrameKeyMap};
use crate::{Overlay, Plan, SourceInfo};

/// OkLab ΔE at which the map reaches 63 % (`1 − e⁻¹`). A just-noticeable difference is about 0.01: a change you can
/// barely see shows faintly, a plain retouch (0.04 and up) brightly.
const DE_SCALE: f32 = 0.03;
/// Face Reshape: the move, as a share of the face's size, at which the map reaches 63 %. The curve starts flat
/// (`1 − e^(−(d/s)²)`), so the warp's far tails, a fraction of a pixel, do not draw its grid's edge.
const MOVE_SCALE: f64 = 0.025;
/// The ΔE is averaged over this share of the photo's long edge (a Gaussian's sigma), so texture work (smoothing a
/// skin's pores) reads as the area it covers rather than as speckle, at every render size alike.
const BLUR: f64 = 0.0012;

/// The map of what control `control` (a place in feature `feature`'s sliders, `FEATURES[feature]`) changes on face
/// `face` (a place in the photo's faces; [`Overlay::ALL_FACES`]: the settings for every face) over the plan's output
/// pixels, 0 (nothing) to 1. `pre` is the image before the retouch step for this plan when the renderer kept it (else
/// it is made here). `None` for a feature or control that does not exist.
pub fn effect(src: &Rgb32f, pre: Option<&Rgb32f>, info: &SourceInfo, p: &Plan<'_>, feature: u8, control: u8, face: u8) -> Option<Plane> {
    let f = FEATURES.get(feature as usize)?;
    let spec = *(f.specs)().get(control as usize)?;
    let zero = || Plane::filled(p.w, p.h, 0.0);
    let Some(input) = info.retouch.as_deref() else { return Some(zero()) };
    let keys = FrameKeyMap::new(&p.frame, p.settings.orientation);
    // (a picture feature, Background, runs with the settings for every face whichever face is picked)
    let face = if f.scope == Scope::Picture { Overlay::ALL_FACES } else { face };
    let Some(without) = without(&p.settings, spec.id, spec.default, input, face, &keys) else { return Some(zero()) };
    if without.retouch == p.settings.retouch {
        return Some(zero());
    }
    Some(crate::timed("retouch effect map", || match f.stage {
        Stage::Geometry => moves(info, p, &without, face),
        Stage::Retouch | Stage::Background => {
            let made;
            let pre = match pre.filter(|i| (i.width, i.height) == (p.w, p.h)) {
                Some(i) => i,
                None => {
                    made = before_step(src, info, p);
                    &*made
                }
            };
            let mut with = pre.clone();
            retouch_step::apply_with(&mut with, info, p, &p.settings);
            let gone = without_image(pre, info, p, &without);
            let gain = (p.settings.light.exposure as f32).exp2();
            let sigma = (BLUR * p.px_per_long) as f32;
            looks_apart(&with, &gone, if gain.is_finite() { gain } else { 1.0 }, sigma)
        }
    }))
}

/// `s` with control `id` at `default`: in the settings for every face ([`Overlay::ALL_FACES`]), else in face `face`'s
/// own (made from those for every face when it has none, so only that face changes). `None` when the face cannot have
/// its own settings.
fn without(s: &DevelopSettings, id: &str, default: f64, input: &Input, face: u8, keys: &FrameKeyMap) -> Option<DevelopSettings> {
    let mut out = s.clone();
    if face == Overlay::ALL_FACES {
        out.retouch.global.set(id, default);
    } else {
        let faces = input.faces.get(..input.faces.len().min(MAX_FACES))?;
        out.retouch.override_mut_in(faces, face as usize, Some(keys))?.set(id, default);
    }
    Some(out)
}

/// The image before the retouch step for `p`, made from the source when the renderer kept none (a render without a
/// stage cache, or the GPU's with the local tone map on), and kept for the next tick.
fn before_step(src: &Rgb32f, info: &SourceInfo, p: &Plan<'_>) -> Arc<Rgb32f> {
    let key = crate::hash_of((p.pre_retouch_key, (p.w, p.h), crate::optics::fingerprint(src), format!("{:?}", p.frame)));
    let kept = PRE.lock().unwrap_or_else(PoisonError::into_inner).as_ref().filter(|(k, _)| *k == key).and_then(|(_, kept)| kept.get());
    if let Some(img) = kept {
        return img;
    }
    let mut img = p.frame.sample(src, p.w, p.h);
    crate::lin_cpu_pre_retouch(&mut img, info, p);
    let img = Arc::new(img);
    let bytes = img.data.len() * std::mem::size_of::<[f32; 3]>();
    *PRE.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, Kept::new(img.clone(), &[(&HOST, bytes)])));
    img
}

/// The last image [`before_step`] made, by what it is made from.
static PRE: Mutex<Option<(u64, Kept<Rgb32f>)>> = Mutex::new(None);

/// The last image retouched with the slider at its default, by what it is made from: a drag of one slider moves only
/// the settings it is compared with, so each tick retouches once more, not twice.
static WITHOUT: Mutex<Option<(u64, Kept<Rgb32f>)>> = Mutex::new(None);

/// `pre` retouched with `without`, kept for the next tick.
fn without_image(pre: &Rgb32f, info: &SourceInfo, p: &Plan<'_>, without: &DevelopSettings) -> Arc<Rgb32f> {
    let key = crate::hash_of((
        p.pre_retouch_key,
        (p.w, p.h),
        format!("{:?}", without.retouch),
        retouch_step::active(without),
        info.retouch.as_ref().map(|i| i.fingerprint()),
        format!("{:?}", p.full.0),
        (p.full.1, p.full.2),
        p.retouch_analysis.as_ref().map(|a| a.key),
    ));
    let kept = WITHOUT.lock().unwrap_or_else(PoisonError::into_inner).as_ref().filter(|(k, _)| *k == key).and_then(|(_, kept)| kept.get());
    if let Some(img) = kept {
        return img;
    }
    let mut img = pre.clone();
    retouch_step::apply_with(&mut img, info, p, without);
    let img = Arc::new(img);
    let bytes = img.data.len() * std::mem::size_of::<[f32; 3]>();
    *WITHOUT.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, Kept::new(img.clone(), &[(&HOST, bytes)])));
    img
}

/// How far apart `a` and `b` (scene-linear Rec. 2020, the same size) look at each pixel, after a gain of `gain` (the
/// exposure): OkLab ΔE averaged by a Gaussian of `sigma` pixels, on [`DE_SCALE`]'s curve. Everything after the
/// comparison works only in the box round the pixels that differ.
fn looks_apart(a: &Rgb32f, b: &Rgb32f, gain: f32, sigma: f32) -> Plane {
    let (w, h) = (a.width, a.height);
    let mut out = Plane::filled(w, h, 0.0);
    if (b.width, b.height) != (w, h) || a.data.len() != w * h || b.data.len() != w * h || w == 0 {
        return out;
    }
    let lab = |c: [f32; 3]| oklab_from_2020(c.map(|v| if v.is_finite() { (v * gain).max(0.0) } else { 0.0 }));
    crate::for_rows(&mut out.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (Some(pa), Some(pb)) = (a.data.get(i), b.data.get(i)) else { continue };
            if pa != pb {
                let (la, lb) = (lab(*pa), lab(*pb));
                let d = ((la[0] - lb[0]).powi(2) + (la[1] - lb[1]).powi(2) + (la[2] - lb[2]).powi(2)).sqrt();
                *v = if d.is_finite() { d } else { 0.0 };
            }
        }
    });
    // (the box round the pixels that changed: a face's part of the frame; the rest stays 0)
    let Some([bx, by, bw, bh]) = lit_box(&out, (3.0 * sigma.max(0.0)).ceil() as usize + 1) else { return out };
    if sigma.is_finite() && sigma >= 0.3 {
        let part = lightcraft_raster::blur::gaussian(&out.crop(bx, by, bw, bh), sigma);
        crate::for_rows(&mut out.data, w, |y, row| {
            let Some(src) = y.checked_sub(by).filter(|r| *r < bh).and_then(|r| part.data.get(r * bw..(r + 1) * bw)) else { return };
            if let Some(dst) = row.get_mut(bx..bx + bw) {
                dst.copy_from_slice(src);
            }
        });
    }
    crate::for_rows(&mut out.data, w, |y, row| {
        if (by..by + bh).contains(&y) {
            row.iter_mut().skip(bx).take(bw).filter(|v| **v > 0.0).for_each(|v| *v = 1.0 - (-*v / DE_SCALE).exp());
        }
    });
    out
}

/// `[x, y, w, h]`: the box round the non-zero pixels of `plane`, grown by `pad` and kept inside it; `None` when every
/// pixel is 0.
fn lit_box(plane: &Plane, pad: usize) -> Option<[usize; 4]> {
    let (w, h) = (plane.width, plane.height);
    // each row's first and last non-zero pixel ([w, 0] for none)
    let mut rows = vec![[w, 0usize]; h];
    crate::for_rows(&mut rows, 1, |y, r| {
        let Some(row) = plane.data.get(y * w..(y + 1) * w) else { return };
        if let (Some(a), Some(b)) = (row.iter().position(|v| *v > 0.0), row.iter().rposition(|v| *v > 0.0)) {
            r[0] = [a, b];
        }
    });
    let lit: Vec<(usize, [usize; 2])> = rows.into_iter().enumerate().filter(|(_, r)| r[0] <= r[1]).collect();
    let (y0, y1) = (lit.first()?.0, lit.last()?.0);
    let x0 = lit.iter().map(|(_, r)| r[0]).min()?;
    let x1 = lit.iter().map(|(_, r)| r[1]).max()?;
    let (bx, by) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    Some([bx, by, x1.saturating_add(pad + 1).min(w) - bx, y1.saturating_add(pad + 1).min(h) - by])
}

/// Face Reshape: how far each output pixel's point of the photo moves between the warp of the plan's settings and
/// that of `without`, as a share of its face's size, on [`MOVE_SCALE`]'s curve; face by face (`face`, or every face).
fn moves(info: &SourceInfo, p: &Plan<'_>, without: &DevelopSettings, face: u8) -> Plane {
    let (w, h) = (p.w, p.h);
    let mut out = Plane::filled(w, h, 0.0);
    let Some(input) = info.retouch.as_deref() else { return out };
    // (the frame without the reshape: its output pixels → the reshaped photo, which the warp maps to the photo)
    let mut bare = p.frame.clone();
    if let Some(warp) = &mut bare.warp {
        warp.reshape = None;
    }
    let photo = bare.for_mask(true);
    let (to_photo, to_out) = (photo.out_to_mask(w, h), photo.mask_to_out(w, h));
    let (ow, oh) = (bare.ow, bare.oh);
    let long = ow.max(oh);
    if !(long.is_finite() && long > 0.0) {
        return out;
    }
    for (i, f) in input.faces.iter().enumerate().take(MAX_FACES) {
        if face != Overlay::ALL_FACES && i != face as usize {
            continue;
        }
        let now = crate::reshape_step::field_of(info, &p.settings, &bare, Some(i));
        let then = crate::reshape_step::field_of(info, without, &bare, Some(i));
        if now == then {
            continue;
        }
        // the face's size on the photo (the longer side of its box), in long edges
        let size = (f.bounds.width() * ow).max(f.bounds.height() * oh) / long;
        if !(size.is_finite() && size > 0.0) {
            continue;
        }
        // the output pixels the two warps can move
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for [x0, y0, x1, y1] in now.iter().chain(&then).flat_map(|f| f.areas()) {
            for k in 0..=16 {
                let t = f64::from(k) / 16.0;
                let (u, v) = (x0 + t * (x1 - x0), y0 + t * (y1 - y0));
                for q in [(u, y0), (u, y1), (x0, v), (x1, v)] {
                    if let Some(o) = to_out(Point::new(q.0, q.1)).filter(|o| o.x.is_finite() && o.y.is_finite()) {
                        b = [b[0].min(o.x), b[1].min(o.y), b[2].max(o.x), b[3].max(o.y)];
                    }
                }
            }
        }
        if !(b[0] < b[2] && b[1] < b[3]) {
            continue;
        }
        let (bx0, by0) = (b[0].floor().clamp(0.0, w as f64) as usize, b[1].floor().clamp(0.0, h as f64) as usize);
        let (bx1, by1) = ((b[2].ceil() + 1.0).clamp(0.0, w as f64) as usize, (b[3].ceil() + 1.0).clamp(0.0, h as f64) as usize);
        let source =
            |field: &Option<Arc<lightcraft_develop::retouch::reshape::Field>>, q: Point| field.as_ref().map_or((q.x, q.y), |f| f.source(q.x, q.y));
        crate::for_rows(&mut out.data, w, |y, row| {
            if !(by0..by1).contains(&y) {
                return;
            }
            for (x, v) in row.iter_mut().enumerate().take(bx1).skip(bx0) {
                let q = to_photo.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
                let (a, c) = (source(&now, q), source(&then, q));
                let d = (((a.0 - c.0) * ow).powi(2) + ((a.1 - c.1) * oh).powi(2)).sqrt() / long / size;
                if d.is_finite() && d > 0.0 {
                    *v = v.max((1.0 - (-(d / MOVE_SCALE).powi(2)).exp()) as f32);
                }
            }
        });
    }
    out
}

#[cfg(test)]
mod tests;
