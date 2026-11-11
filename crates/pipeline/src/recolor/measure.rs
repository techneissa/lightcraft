//! Measuring a recolored area's main colour ([`super::Reference`]) on a proxy of the whole frame, through the same
//! per-pixel steps the finish stage runs before it recolors (exposure, dehaze, local tone, calibration, tone map,
//! the colour stage and the colour overlay), so the main colour lands on the picked one with Shadows, Contrast or
//! Clarity set too.

use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_color::perceptual::oklab_from_2020;
use lightcraft_develop::{DevelopSettings, Mask};
use lightcraft_raster::Rgb32f;

use super::{Reference, hue, is_on, saturation, source_rgb, srgb_to_oklab, wrap};
use crate::finish::{FinishParams, FinishParamsInput};
use crate::geometry::Frame;
use crate::output::OutputSpace;
use crate::{Quality, SourceInfo};

/// Long edge of the proxy the main colours are measured on: fixed, so every render size gets the same numbers.
pub const PROXY_EDGE: usize = 384;
/// Long edge of the second proxy for a mask too small to measure on the first ([`SMALL_WEIGHT`]).
pub const FINE_EDGE: usize = 1024;
/// A mask with less alpha than this (in proxy pixels) is measured again on the [`FINE_EDGE`] proxy.
pub const SMALL_WEIGHT: f32 = 64.0;
/// Least total alpha (in proxy pixels) an area needs for a main colour: a mask with less recolors nothing.
pub const MIN_WEIGHT: f32 = 1.0;
/// A pixel counts towards the main hue when its chroma and saturation pass these.
pub const CHROMATIC: [f32; 2] = [0.025, 0.08];
/// Hue histogram bins.
const BINS: usize = 72;
/// The peak's neighbourhood that sets the main hue, and the cluster around it (radians).
const PEAK_HALF: f32 = 0.45;
const CLUSTER_HALF: f32 = 0.6;
/// How many measured edits [`references`] keeps.
const KEPT: usize = 4;

/// The main colour of `samples` (OkLab, alpha): the peak of a hue histogram of its coloured pixels weighted by
/// chroma × alpha, refined to their weighted circular mean, with that cluster's lightness, chroma and spread. With
/// `from` (the Colour to Replace, OkLab) the cluster is the one around its hue. Mostly grey samples give a grey
/// reference (their mean). `None` with less than [`MIN_WEIGHT`] of alpha.
pub fn main_colour(samples: &[([f32; 3], f32)], from: Option<[f32; 3]>) -> Option<Reference> {
    let ok = |(lab, w): &&([f32; 3], f32)| w.is_finite() && *w > 0.0 && lab.iter().all(|v| v.is_finite());
    let lch = |lab: [f32; 3]| (lab[0], lab[1].hypot(lab[2]), hue(lab[1], lab[2]));
    let (mut total, mut sl, mut sc, mut sa, mut sb, mut chroma_w) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    let mut hist = [0f64; BINS];
    for (lab, w) in samples.iter().filter(ok) {
        let (l, c, h) = lch(*lab);
        let w64 = f64::from(*w);
        total += w64;
        sl += w64 * f64::from(l);
        sc += w64 * f64::from(c);
        sa += w64 * f64::from(lab[1]);
        sb += w64 * f64::from(lab[2]);
        if c > CHROMATIC[0] && saturation(c, l) > CHROMATIC[1] {
            chroma_w += w64;
            let bin = (((h + std::f32::consts::PI) / std::f32::consts::TAU * BINS as f32) as usize).min(BINS - 1);
            if let Some(b) = hist.get_mut(bin) {
                *b += w64 * f64::from(c);
            }
        }
    }
    if total < f64::from(MIN_WEIGHT) {
        return None;
    }
    let mean_hue = [(sa / total) as f32, (sb / total) as f32];
    let grey = || {
        let n = mean_hue[0].hypot(mean_hue[1]);
        let h = if n > 1e-9 { [mean_hue[0] / n, mean_hue[1] / n] } else { [1.0, 0.0] };
        Reference { l: (sl / total) as f32, c: (sc / total) as f32, h, spread: 0.0 }
    };
    let peak = match from {
        Some(f) => hue(f[1], f[2]),
        None if chroma_w < 0.1 * total => return Some(grey()),
        None => {
            let smoothed = (0..BINS).map(|i| {
                [(-2isize, 1.0), (-1, 2.0), (0, 3.0), (1, 2.0), (2, 1.0)]
                    .iter()
                    .map(|(d, k)| k * hist.get((i as isize + d).rem_euclid(BINS as isize) as usize).copied().unwrap_or(0.0))
                    .sum::<f64>()
            });
            let best = smoothed.enumerate().max_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(i, _)| i);
            (best as f32 + 0.5) / BINS as f32 * std::f32::consts::TAU - std::f32::consts::PI
        }
    };
    // the main hue: the weighted circular mean near the peak
    let (mut ca, mut cb) = (0f64, 0f64);
    for (lab, w) in samples.iter().filter(ok) {
        let (l, c, h) = lch(*lab);
        if c > CHROMATIC[0] && saturation(c, l) > CHROMATIC[1] && wrap(h - peak).abs() < PEAK_HALF {
            ca += f64::from(*w) * f64::from(lab[1]);
            cb += f64::from(*w) * f64::from(lab[2]);
        }
    }
    let main = if ca.hypot(cb) > 1e-12 { (cb as f32).atan2(ca as f32) } else { peak };
    // the cluster around it: its lightness, chroma and spread
    let (mut w_sum, mut l_sum, mut c_sum, mut spread) = (0f64, 0f64, 0f64, 0f64);
    for (lab, w) in samples.iter().filter(ok) {
        let (l, c, h) = lch(*lab);
        let d = wrap(h - main);
        if c > CHROMATIC[0] && saturation(c, l) > CHROMATIC[1] && d.abs() < CLUSTER_HALF {
            let w64 = f64::from(*w);
            w_sum += w64;
            l_sum += w64 * f64::from(l);
            c_sum += w64 * f64::from(c);
            spread += w64 * f64::from(d * d);
        }
    }
    let h = [main.cos(), main.sin()];
    if w_sum < f64::from(MIN_WEIGHT) * 0.5 {
        // a Colour to Replace the area hardly holds: its own lightness and chroma
        return from.map(|f| Reference { l: f[0], c: f[1].hypot(f[2]), h, spread: 0.15 }).or_else(|| Some(grey()));
    }
    Some(Reference { l: (l_sum / w_sum) as f32, c: (c_sum / w_sum) as f32, h, spread: (spread / w_sum).sqrt() as f32 })
}

/// The settings the references read: everything but each mask's Recolor amount, target colour and Range (which act
/// after the measurement).
fn key_of(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, frame: &Frame, space: OutputSpace, native_long: usize) -> u64 {
    let mut m = s.clone();
    for mask in &mut m.masks {
        let on = is_on(&mask.adjust);
        let a = &mut mask.adjust;
        (a.recolor, a.recolor_r, a.recolor_g, a.recolor_b, a.recolor_range) = (if on { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0, 0.0);
    }
    let info_key = (info.raw, info.as_shot_temp.to_bits(), info.as_shot_tint.to_bits(), info.mattes.as_ref().map(|m| Arc::as_ptr(m) as usize));
    crate::hash_of((
        crate::optics::fingerprint(src),
        format!("{m:?}"),
        format!("{frame:?}"),
        format!("{space:?}"),
        native_long,
        format!("{info_key:?}"),
    ))
}

/// The main colour of every visible mask whose Recolor is on (by mask id), measured on a [`PROXY_EDGE`] proxy of the
/// whole `frame` of `src` (a mask too small for it on a [`FINE_EDGE`] one). The proxy sizes don't depend on the
/// request, so every full render of one edit (preview, export, a window, CPU or GPU) gets the same numbers. The last
/// few edits' answers are kept: a Recolor amount, colour or Range measures nothing again. A `draft` render (a slider
/// drag) takes the last answer for the same photo, frame and masks without measuring; the full render after the drag
/// measures again.
pub(crate) fn references(
    src: &Rgb32f,
    info: &SourceInfo,
    s: &DevelopSettings,
    frame: &Frame,
    space: OutputSpace,
    native_long: usize,
    draft: bool,
) -> Vec<(u32, Reference)> {
    type Entry = (u64, u64, Vec<(u32, Reference)>);
    static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    let on: Vec<&Mask> = s.masks.iter().filter(|m| m.renders() && is_on(&m.adjust) && m.adjust.amount > 0.0).collect();
    if on.is_empty() || src.width == 0 || src.height == 0 {
        return Vec::new();
    }
    let key = key_of(src, info, s, frame, space, native_long);
    let ids: Vec<u32> = on.iter().map(|m| m.id).collect();
    let froms: Vec<Option<[u64; 3]>> = on.iter().map(|m| source_rgb(&m.adjust).map(|c| c.map(f64::to_bits))).collect();
    let loose = crate::hash_of((crate::optics::fingerprint(src), format!("{frame:?}"), &ids, froms));
    {
        let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = cache.iter().find(|e| e.0 == key).or_else(|| cache.iter().rev().find(|e| draft && e.1 == loose)) {
            return found.2.clone();
        }
    }
    let started = crate::profiling().then(std::time::Instant::now);
    let mut found = measure(src, info, s, frame, space, native_long, PROXY_EDGE, &ids);
    let small: Vec<u32> = ids.iter().copied().filter(|id| found.iter().find(|f| f.0 == *id).is_none_or(|f| f.2 < SMALL_WEIGHT)).collect();
    if !small.is_empty() {
        let fine = measure(src, info, s, frame, space, native_long, FINE_EDGE, &small);
        found.retain(|f| !small.contains(&f.0));
        found.extend(fine);
    }
    let out: Vec<(u32, Reference)> = ids.iter().filter_map(|id| found.iter().find(|f| f.0 == *id).and_then(|f| f.1.map(|r| (*id, r)))).collect();
    if let Some(t) = started {
        eprintln!("  recolor references: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    c.push((key, loose, out.clone()));
    if c.len() > KEPT {
        c.remove(0);
    }
    out
}

/// One measurement on a proxy of long edge `edge`: per mask id in `ids`, its main colour and its total alpha.
#[allow(clippy::too_many_arguments)]
fn measure(
    src: &Rgb32f,
    info: &SourceInfo,
    s: &DevelopSettings,
    frame: &Frame,
    space: OutputSpace,
    native_long: usize,
    edge: usize,
    ids: &[u32],
) -> Vec<(u32, Option<Reference>, f32)> {
    let (pw, ph) = frame.fit(edge, edge);
    let mut img = Rgb32f::clone(&crate::local::frame_proxy(src, frame, pw, ph));
    crate::local::white_balance(&mut img, info, s);
    let px_per_long = frame.px_per_long(pw);
    let detail_scale = crate::local::detail_scale(px_per_long, native_long);
    let mut planes = crate::local::Planes::default();
    let mut alphas = crate::masks::AlphaCache::default();
    let keys = crate::masks::AlphaKeys { geo: 0, image: 0 };
    let p = crate::local::prepare(
        Arc::new(img),
        s,
        frame,
        px_per_long,
        detail_scale,
        Quality::Full,
        &mut planes,
        info.mattes.as_ref(),
        (&mut alphas, keys),
    );
    let fp =
        FinishParams::new(FinishParamsInput { settings: s, frame, info, image_size: (pw, ph), px_per_long, air_pre: p.air, space, draft: false });
    let terms = crate::finish::scaled_terms(&p);
    let wanted: Vec<(usize, u32)> = p.masks.iter().enumerate().filter(|(_, m)| ids.contains(&m.id)).map(|(k, m)| (k, m.id)).collect();
    let mut samples: Vec<Vec<([f32; 3], f32)>> = vec![Vec::new(); wanted.len()];
    for y in 0..ph {
        let active: Vec<usize> =
            (0..p.masks.len()).filter(|k| terms.get(*k).is_some_and(Option::is_some) && p.masks[*k].alpha.row(y).is_some()).collect();
        if !wanted.iter().any(|(k, _)| active.contains(k)) {
            continue;
        }
        for x in 0..pw {
            let mut lab = None;
            for (list, (k, _)) in samples.iter_mut().zip(&wanted) {
                let a = p.masks.get(*k).map_or(0.0, |m| m.alpha.at(x, y));
                if a > 0.0 {
                    let v = *lab.get_or_insert_with(|| oklab_from_2020(colour_at(&p, &fp, &terms, &active, x, y)));
                    list.push((v, a));
                }
            }
        }
    }
    wanted
        .iter()
        .zip(&samples)
        .map(|((_, id), list)| {
            let from = s.masks.iter().find(|m| m.id == *id).and_then(|m| source_rgb(&m.adjust)).map(srgb_to_oklab);
            let weight: f32 = list.iter().map(|(_, a)| a).sum();
            (*id, main_colour(list, from), weight)
        })
        .collect()
}

/// The display-linear colour of proxy pixel `(x, y)` where the finish stage recolors it: the same steps as
/// `finish::finish_with` up to and including the colour overlay (exact encoding).
fn colour_at(
    p: &crate::Prepared,
    fp: &FinishParams,
    terms: &[Option<[f32; crate::finish::MASK_TERMS]>],
    active: &[usize],
    x: usize,
    y: usize,
) -> [f32; 3] {
    let i = y * p.img.width + x;
    let raw = p.img.data.get(i).copied().unwrap_or_default();
    let local = crate::finish::pixel_local_terms(p, terms, active, x, y, None);
    let (l_sat, l_hue) = (local.values.get(11).copied().unwrap_or(0.0), local.values.get(12).copied().unwrap_or(0.0));
    let l0 = p.log_l.data.get(i).copied().unwrap_or(0.0) + fp.ev;
    let (mut c, l1) = crate::finish::adjust_scene_pixel(p, fp, i, raw, &local);
    let base = p.base.data.get(i).copied().unwrap_or(0.0) + fp.ev + (l1 - l0);
    let delta = crate::finish::local_tone_delta(p, fp, i, base, l1, &local);
    if delta != 0.0 {
        let g = delta.exp2();
        c = c.map(|v| v * g);
    }
    if fp.calib.is_some() || fp.shadow_tint != 0.0 {
        c = crate::colorops::calibrate(c, fp.calib.as_ref(), fp.shadow_tint);
    }
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
    let srgb = crate::finish::srgb_lut();
    d = crate::finish::apply_light(d, fp.light.as_ref(), None, true, srgb);
    d = fp.ops.apply(d, l_sat, l_hue);
    if let Some((dir, amt)) = local.tint {
        let lab = oklab_from_2020(d);
        d = lightcraft_color::perceptual::oklab_to_2020([lab[0], lab[1] + dir[0] * 0.08 * amt, lab[2] + dir[1] * 0.08 * amt]);
    }
    d
}
