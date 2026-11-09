//! Fork feature (`fork/glare-highlights.md`, issue #74): the Highlights mode, glare found and filled.
//!
//! [`make`] looks for glare ([`super::glare::find`]) in the area (the whole photo, a box or a mask), groups what it
//! found into a few clusters, and fills each cluster's crop at up to [`FILL_SIDE`] pixels with the learned fill (LaMa,
//! the one Remove spots use, with its activity row and cache) or, without it, the Classic patch fill. The change is
//! weighted by the glare's soft edge and saved as one tile per cluster ([`super::tiles`]), so only the glare changes and
//! a preview and an export apply the same thing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::{Glare, Reflection, ReflectionMode, SpotFill};
use lightcraft_pipeline::learned_fill::{FillRun, LearnedFill};
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f};

use super::area::Region;
use super::glare::{self, Stroke};
use super::tiles::{MAX_PIXELS, MAX_TILES, Tile};

/// Long edge of the picture the glare is looked for in.
const FIND_EDGE: usize = 3072;
/// Longest side of a crop handed to the fill.
pub const FILL_SIDE: usize = 1536;
/// Longest side of a crop handed to the Classic fill (it is slower on large crops).
const CLASSIC_SIDE: usize = 768;
/// Largest glare spot, as a share of a chosen area, and of the whole photo when no area is chosen.
const MAX_SPOT_AREA: f64 = 0.35;
const MAX_SPOT_WHOLE: f64 = 0.08;
/// The alpha above which a pixel is part of the hole handed to the fill.
const HOLE: f32 = 0.02;

/// What a Highlights run made, and how: the tiles and the settings to save with them ([`super::area::save`]).
pub struct Made {
    pub tiles: Vec<Tile>,
    pub reflection: Reflection,
    /// Share of the area that changed (0..=1).
    pub coverage: f64,
    /// The fill used: LaMa, or Classic when LaMa was missing or failed.
    pub fill: SpotFill,
}

/// Find and fill the glare of `src` (the full source) in `region`.
/// `cancel` is the run's stop flag: the learned fill's row stops it too, and a stopped run returns "Stopped" and makes
/// nothing.
pub fn make(src: &Rgb32f, glare: &Glare, region: &Region, learned: Option<Arc<dyn LearnedFill>>, cancel: &Arc<AtomicBool>) -> Result<Made, String> {
    let glare = glare.clone().sanitized();
    let (w, h) = (src.width, src.height);
    let search = region.pixel_box(w, h).ok_or("The area is empty")?;
    let (sw, sh) = (search[2] - search[0], search[3] - search[1]);
    let k = (FIND_EDGE as f64 / sw.max(sh) as f64).min(1.0);
    let (fw, fh) = (((sw as f64 * k).round() as usize).max(1), ((sh as f64 * k).round() as usize).max(1));
    let crop = src.crop(search[0], search[1], sw, sh);
    let small = if k < 1.0 { resize(&crop, fw, fh, Filter::Box) } else { crop };
    // the area's own alpha (a mask), in the search picture's pixels
    let to_frac = |x: usize, y: usize| ((search[0] as f64 + (x as f64 + 0.5) / k) / w as f64, (search[1] as f64 + (y as f64 + 0.5) / k) / h as f64);
    let region_alpha = region.alpha.as_ref().map(|_| {
        Plane::from_fn(fw, fh, |x, y| {
            let (fx, fy) = to_frac(x, y);
            region.alpha_at(fx, fy)
        })
    });
    let long = w.max(h) as f64;
    let strokes: Vec<Stroke> = glare
        .paint
        .iter()
        .map(|s| Stroke {
            erase: s.erase,
            radius: (s.size * long * k) as f32,
            points: s
                .points
                .iter()
                .map(|p| (((p.x * w as f64 - search[0] as f64) * k) as f32, ((p.y * h as f64 - search[1] as f64) * k) as f32))
                .collect(),
        })
        .collect();
    let whole = search == [0, 0, w, h] && region.alpha.is_none();
    let params = glare::Params {
        threshold: glare.threshold,
        expand: glare.expand,
        photo_long: long * k,
        max_spot: if whole { MAX_SPOT_WHOLE } else { MAX_SPOT_AREA },
        chosen: !whole,
        boxed: !whole && region.alpha.is_none(),
    };
    let mut mask = glare::find_until(&small, region_alpha.as_ref(), &params, &strokes, cancel);
    if cancel.load(Ordering::Relaxed) {
        return Err("Stopped".into());
    }
    if params.boxed {
        // a box's border fades in, as a reflection model's result does, so a stroke across it leaves no edge
        let feather = (fw.min(fh) as f64 * super::area::BOX_FEATHER).max(1.0) as f32;
        for (i, v) in mask.data.iter_mut().enumerate() {
            let (x, y) = ((i % fw) as f32 + 0.5, (i / fw) as f32 + 0.5);
            let edge = x.min(y).min(fw as f32 - x).min(fh as f32 - y);
            let t = (edge / feather).clamp(0.0, 1.0);
            *v *= t * t * (3.0 - 2.0 * t);
        }
    }
    let changed = mask.data.iter().filter(|v| **v > HOLE).count();
    if changed == 0 {
        return Err("No glare found in the area. Lower Threshold, or paint the glare.".into());
    }
    let coverage = changed as f64 / (fw * fh).max(1) as f64;
    let run = FillRun::with_stop(cancel.clone());
    let mut used = glare.fill;
    let mut tiles = Vec::new();
    // each cluster in the source's pixels, and the size it is filled at: all of them together within what a
    // correction may hold
    let px = |v: usize, origin: usize, n: usize| ((origin as f64 + v as f64 / k).round() as usize).min(n);
    let found: Vec<([usize; 4], [usize; 4])> = clusters(&mask, MAX_TILES)
        .into_iter()
        .map(|c| (c, [px(c[0], search[0], w), px(c[1], search[1], h), px(c[2], search[0], w), px(c[3], search[1], h)]))
        .filter(|(_, b)| b[2] > b[0] && b[3] > b[1])
        .collect();
    let side = if used == SpotFill::Lama && learned.is_some() { FILL_SIDE } else { CLASSIC_SIDE };
    let mut sizes: Vec<(usize, usize)> =
        found.iter().map(|(_, b)| lightcraft_raster::resample::fit_size(b[2] - b[0], b[3] - b[1], side, side)).collect();
    budget(&mut sizes, MAX_PIXELS);
    for ((c, b), size) in found.into_iter().zip(sizes) {
        if cancel.load(Ordering::Relaxed) || run.stopped() {
            return Err("Stopped".into());
        }
        let tile_src = src.crop(b[0], b[1], b[2] - b[0], b[3] - b[1]);
        // the Classic fill (also after LaMa gave none) works on smaller crops
        let (tw, th) = if used == SpotFill::Lama && learned.is_some() {
            size
        } else {
            lightcraft_raster::resample::fit_size(size.0, size.1, CLASSIC_SIDE, CLASSIC_SIDE)
        };
        let rgb = if (tw, th) == (tile_src.width, tile_src.height) { tile_src } else { resize(&tile_src, tw, th, Filter::Lanczos3) };
        // the glare's alpha at the tile's size
        let alpha: Vec<f32> = (0..tw * th)
            .map(|i| {
                let (x, y) = (i % tw, i / tw);
                let sx = (c[0] as f64 + (x as f64 + 0.5) * (c[2] - c[0]) as f64 / tw as f64) - 0.5;
                let sy = (c[1] as f64 + (y as f64 + 0.5) * (c[3] - c[1]) as f64 / th as f64) - 0.5;
                sample(&mask, sx as f32, sy as f32)
            })
            .collect();
        let (filled, how) = fill(&rgb, &alpha, used, learned.as_deref(), &run)?;
        used = how;
        let kept = keep_texture(&rgb, &filled, &alpha);
        let mut grid = Rgb32f::new(tw, th);
        for ((g, (s, f)), a) in grid.data.iter_mut().zip(rgb.data.iter().zip(&kept.data)).zip(&alpha) {
            if *a > 0.0 {
                *g = [0, 1, 2].map(|i| (s[i] - f[i]) * a);
            }
        }
        let rect = [b[0] as f32 / w as f32, b[1] as f32 / h as f32, b[2] as f32 / w as f32, b[3] as f32 / h as f32];
        tiles.push(Tile { rect, grid });
    }
    if cancel.load(Ordering::Relaxed) || run.stopped() {
        return Err("Stopped".into());
    }
    if tiles.is_empty() {
        return Err("No glare found in the area. Lower Threshold, or paint the glare.".into());
    }
    let base = Reflection {
        amount: 100.0,
        quality: 0,
        model: String::new(),
        protect_sky: false,
        detail_protection: 0.0,
        mode: ReflectionMode::Highlights,
        glare: Glare { fill: used, ..glare },
        ..Default::default()
    };
    Ok(Made { tiles, reflection: base, coverage, fill: used })
}

/// Shrink the largest of `sizes` (width, height) until all of them together hold at most `max_total` pixels.
pub fn budget(sizes: &mut [(usize, usize)], max_total: usize) {
    while sizes.iter().map(|(w, h)| w.saturating_mul(*h)).fold(0usize, usize::saturating_add) > max_total {
        let Some(big) = sizes.iter_mut().max_by_key(|(w, h)| w.saturating_mul(*h)) else { return };
        if big.0.saturating_mul(big.1) <= 1 {
            return;
        }
        *big = (((big.0 as f64 * 0.9) as usize).max(1), ((big.1 as f64 * 0.9) as usize).max(1));
    }
}

/// The fill with the photo's own fine detail kept where the glare did not blow it out. Glare adds a smooth veil of
/// light: under a veil the detail is still there, so only the smooth part is taken from the fill (`src - blur(src) +
/// blur(fill)`); where the photo is clipped there is no detail left and the fill is taken as it is.
pub fn keep_texture(src: &Rgb32f, filled: &[f32], alpha: &[f32]) -> Rgb32f {
    let (w, h) = (src.width, src.height);
    // non-finite values (a broken fill or source) would reach every neighbour through the blurs: the source's value,
    // or 0, stands in for them
    let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
    let src = &Rgb32f { width: w, height: h, data: src.data.iter().map(|p| p.map(finite)).collect() };
    let fill = Rgb32f {
        width: w,
        height: h,
        data: filled.as_chunks::<3>().0.iter().zip(&src.data).map(|(f, s)| [0, 1, 2].map(|c| if f[c].is_finite() { f[c] } else { s[c] })).collect(),
    };
    if fill.data.len() != src.data.len() || alpha.len() != src.data.len() {
        return fill;
    }
    let bright = |p: &[f32; 3]| p[0].max(p[1]).max(p[2]);
    let white = src.data.iter().zip(alpha).filter(|(_, a)| **a > HOLE).map(|(p, _)| bright(p)).filter(|v| v.is_finite()).fold(0.0f32, f32::max);
    if white <= 0.0 {
        return fill;
    }
    // where the glare clearly outshines what the fill puts there (or the photo is clipped), it is glare: the fill is
    // taken as it is; elsewhere the glare is a veil over the photo's own detail
    let luma = |p: &[f32; 3]| 0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2];
    let smooth = |e0: f32, e1: f32, x: f32| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    // compared smoothed, so the photo's own texture does not count as glare
    let radius = (w.max(h) / 200).clamp(2, 8);
    let (smooth_src, low_fill) = (blur(src, radius), blur(&fill, radius));
    let glare: Vec<f32> = src
        .data
        .iter()
        .zip(smooth_src.data.iter().zip(&low_fill.data))
        .map(|(s, (ss, lf))| {
            let outshines = (luma(ss) - luma(lf)) / luma(lf).max(1e-4);
            smooth(0.85, 0.98, bright(s) / white).max(smooth(0.3, 1.0, outshines))
        })
        .collect();
    // the photo with its glare already filled: its blur carries no glare light into the neighbours (a dark halo)
    let mut veiled = src.clone();
    for ((v, f), g) in veiled.data.iter_mut().zip(&fill.data).zip(&glare) {
        *v = [0, 1, 2].map(|c| v[c] + (f[c] - v[c]) * g);
    }
    let low_src = blur(&veiled, radius);
    let mut out = fill.clone();
    for (i, o) in out.data.iter_mut().enumerate() {
        let (Some(s), Some(ls), Some(lf), Some(g)) = (veiled.data.get(i), low_src.data.get(i), low_fill.data.get(i), glare.get(i)) else { continue };
        for c in 0..3 {
            // the detail moves the fill by at most a quarter of its own value, so an edge the blur missed leaves no line
            // (max / min, never `clamp`: its bounds must not cross or be NaN)
            let cap = 0.25 * lf[c].abs();
            let detail = lf[c] + (s[c] - ls[c]).max(-cap).min(cap);
            o[c] = (g * o[c] + (1.0 - g) * detail).max(0.0);
        }
    }
    out
}

/// Three box blurs of `radius` (close to a Gaussian), edges clamped.
fn blur(img: &Rgb32f, radius: usize) -> Rgb32f {
    let mut cur = img.clone();
    for _ in 0..3 {
        cur = box_pass(&box_pass(&cur, radius, true), radius, false);
    }
    cur
}

fn box_pass(img: &Rgb32f, r: usize, horizontal: bool) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    let (n, lines) = if horizontal { (w, h) } else { (h, w) };
    let mut out = img.clone();
    if n == 0 {
        return out;
    }
    let at = |line: usize, k: usize| if horizontal { line * w + k } else { k * w + line };
    for line in 0..lines {
        let get = |k: isize| img.data.get(at(line, k.clamp(0, n as isize - 1) as usize)).copied().unwrap_or_default();
        let mut sum = [0.0f32; 3];
        for k in -(r as isize)..=(r as isize) {
            let p = get(k);
            (0..3).for_each(|c| sum[c] += p[c]);
        }
        let norm = 1.0 / (2 * r + 1) as f32;
        for k in 0..n {
            if let Some(o) = out.data.get_mut(at(line, k)) {
                *o = sum.map(|v| v * norm);
            }
            let (add, sub) = (get(k as isize + r as isize + 1), get(k as isize - r as isize));
            (0..3).for_each(|c| sum[c] += add[c] - sub[c]);
        }
    }
    out
}

fn sample(p: &Plane, x: f32, y: f32) -> f32 {
    if p.width == 0 || p.height == 0 {
        return 0.0;
    }
    let (x, y) = (x.clamp(0.0, (p.width - 1) as f32), y.clamp(0.0, (p.height - 1) as f32));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(p.width - 1), (y0 + 1).min(p.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let g = |x: usize, y: usize| p.data.get(y * p.width + x).copied().unwrap_or(0.0);
    let top = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * fx;
    top + (g(x0, y1) + (g(x1, y1) - g(x0, y1)) * fx - top) * fy
}

/// The fill of `rgb` where `alpha` is above [`HOLE`], with the fill asked for, or Classic when it gives none.
fn fill(rgb: &Rgb32f, alpha: &[f32], want: SpotFill, learned: Option<&dyn LearnedFill>, run: &FillRun) -> Result<(Vec<f32>, SpotFill), String> {
    let (w, h) = (rgb.width, rgb.height);
    let flat: Vec<f32> = rgb.data.iter().flatten().copied().collect();
    if want == SpotFill::Lama
        && let Some(provider) = learned
    {
        let hole: Vec<f32> = alpha.iter().map(|a| if *a > HOLE { 1.0 } else { 0.0 }).collect();
        if let Some(out) = provider.fill(run, &flat, &hole, w, h, false)
            && out.len() == flat.len()
            && out.iter().all(|v| v.is_finite())
        {
            return Ok((out.as_ref().clone(), SpotFill::Lama));
        }
        if run.stopped() {
            return Err("Stopped".into());
        }
    }
    let hole: Vec<bool> = alpha.iter().map(|a| *a > HOLE).collect();
    let out = lightcraft_inpaint::complete(w, h, 3, &flat, &hole, &lightcraft_inpaint::CompleteParams::default())
        .unwrap_or_else(|| lightcraft_inpaint::membrane_fill(w, h, 3, &flat, &hole));
    if out.len() != flat.len() {
        return Err("The fill returned the wrong size".into());
    }
    Ok((out, SpotFill::Classic))
}

/// Boxes `[x0, y0, x1, y1)` around the groups of glare in `mask`, each with a margin of context as wide as the group
/// (at least 8 pixels); boxes that touch are merged, and at most `max` are kept (the rest merged into one).
pub fn clusters(mask: &Plane, max: usize) -> Vec<[usize; 4]> {
    let (w, h) = (mask.width, mask.height);
    let on: Vec<bool> = mask.data.iter().map(|v| *v > HOLE).collect();
    let mut seen = vec![false; w * h];
    let mut boxes: Vec<[usize; 4]> = Vec::new();
    for start in 0..w * h {
        if !on[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if on[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let m = (x1 - x0).max(y1 - y0).max(8);
        boxes.push([x0.saturating_sub(m), y0.saturating_sub(m), (x1 + m).min(w), (y1 + m).min(h)]);
    }
    // merge boxes that overlap until none do
    loop {
        let mut merged = false;
        'outer: for i in 0..boxes.len() {
            for j in i + 1..boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                if a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3] {
                    boxes[i] = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                    boxes.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    if boxes.len() > max.max(1) {
        let mut rest = boxes.split_off(max.max(1) - 1);
        let first = rest.pop().unwrap_or([0, 0, w, h]);
        let all = rest.iter().fold(first, |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
        boxes.push(all);
    }
    boxes.sort_by_key(|b| (b[1], b[0]));
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_preview::Hash128;

    /// A fill that paints every hole pixel one grey and counts its calls.
    struct Grey(std::sync::atomic::AtomicUsize);
    impl LearnedFill for Grey {
        fn fill(&self, _run: &FillRun, rgb: &[f32], hole: &[f32], _w: usize, _h: usize, _draft: bool) -> Option<Arc<Vec<f32>>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            let mut out = rgb.to_vec();
            for (px, hl) in out.as_chunks_mut::<3>().0.iter_mut().zip(hole) {
                if *hl > 0.0 {
                    px.copy_from_slice(&[0.3, 0.2, 0.14]);
                }
            }
            Some(Arc::new(out))
        }
    }

    fn photo() -> Rgb32f {
        // skin-like grey with two glare spots far apart
        Rgb32f::from_fn(600, 400, |x, y| {
            let d = |cx: f32, cy: f32| ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d(150.0, 200.0) < 14.0 || d(450.0, 200.0) < 14.0 { [1.0, 1.0, 0.97] } else { [0.3, 0.2, 0.14] }
        })
    }

    #[test]
    fn texture_under_a_veil_is_kept_and_a_clipped_core_takes_the_fill() {
        // stripes under a smooth veil of light, with a clipped core in the middle
        let src = Rgb32f::from_fn(64, 64, |x, y| {
            let stripe = if (x / 2) % 2 == 0 { 0.15 } else { 0.25 };
            let d = ((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt();
            if d < 4.0 { [1.0; 3] } else { [stripe + 0.08 * (1.0 - d / 45.0).max(0.0); 3] }
        });
        let grey: Vec<f32> = vec![0.2; 64 * 64 * 3];
        let alpha = vec![1.0f32; 64 * 64];
        let out = keep_texture(&src, &grey, &alpha);
        assert!((out.get(32, 32)[0] - 0.2).abs() < 0.02, "core takes the fill: {:?}", out.get(32, 32));
        let (a, b) = (out.get(12, 32)[0], out.get(14, 32)[0]);
        assert!((a - b).abs() > 0.05, "stripes survive: {a} {b}");
        assert!(out.data.iter().all(|p| p.iter().all(|v| v.is_finite() && *v >= 0.0)));
    }

    #[test]
    fn a_box_of_two_pixels_is_an_error_or_a_result_never_a_panic() {
        let src = photo();
        let tiny = Region { rect: [0.25, 0.5, 0.25 + 2.0 / 600.0, 0.5 + 2.0 / 400.0], alpha: None };
        let _ = make(&src, &Glare::default(), &tiny, None, &Arc::new(AtomicBool::new(false)));
    }

    #[test]
    fn a_fill_with_nan_values_gives_finite_pixels_not_a_panic() {
        let src = Rgb32f::filled(16, 16, [0.5, 0.4, 0.3]);
        let mut fill = vec![0.2f32; 16 * 16 * 3];
        fill[0] = f32::NAN;
        fill[100] = f32::INFINITY;
        let out = keep_texture(&src, &fill, &[1.0; 256]);
        assert!(out.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn many_large_clusters_are_shrunk_to_fit_one_correction() {
        let mut sizes = vec![(1536, 1536); 8];
        budget(&mut sizes, MAX_PIXELS);
        let total: usize = sizes.iter().map(|(w, h)| w * h).sum();
        assert!(total <= MAX_PIXELS, "{total}");
        assert!(sizes.iter().all(|(w, h)| *w > 500 && *h > 500), "{sizes:?}");
        let mut small = vec![(100, 50)];
        budget(&mut small, MAX_PIXELS);
        assert_eq!(small, vec![(100, 50)]);
    }

    /// A fill that stops the run while it fills (✕ on a row).
    struct Stopper(Arc<AtomicBool>);
    impl LearnedFill for Stopper {
        fn fill(&self, _run: &FillRun, rgb: &[f32], _hole: &[f32], _w: usize, _h: usize, _draft: bool) -> Option<Arc<Vec<f32>>> {
            self.0.store(true, Ordering::Relaxed);
            Some(Arc::new(rgb.to_vec()))
        }
    }

    #[test]
    fn a_run_stopped_during_the_fill_makes_nothing() {
        let cancel = Arc::new(AtomicBool::new(false));
        let stopper: Arc<dyn LearnedFill> = Arc::new(Stopper(cancel.clone()));
        let e = make(&photo(), &Glare::default(), &Region::whole(), Some(stopper), &cancel).err();
        assert_eq!(e.as_deref(), Some("Stopped"));
    }

    #[test]
    fn painted_glare_fades_in_at_a_box_border() {
        let dir = std::env::temp_dir().join(format!("lc-glare-feather-{}", std::process::id()));
        let src = Rgb32f::filled(200, 200, [0.6, 0.6, 0.6]);
        let region = Region { rect: [0.25, 0.25, 0.75, 0.75], alpha: None };
        let stroke = lightcraft_develop::GlareStroke { erase: false, size: 0.4, points: vec![lightcraft_geom::Point::new(0.5, 0.5)] };
        let glare = Glare { paint: vec![stroke], ..Default::default() };
        let grey: Arc<dyn LearnedFill> = Arc::new(Grey(Default::default()));
        let made = make(&src, &glare, &region, Some(grey), &Arc::new(AtomicBool::new(false))).unwrap();
        let r = super::super::area::save_in(&dir, &made.tiles, Some(Hash128(9)), made.reflection).unwrap();
        let out = super::super::apply(&Arc::new(src.clone()), &r, Some(Hash128(9)), None).unwrap();
        let change = |x: usize, y: usize| src.get(x, y)[0] - out.get(x, y)[0];
        assert!(change(100, 100) > 0.25, "the middle is filled");
        assert!(change(50, 100) < 0.05 * change(100, 100), "the border fades in: {}", change(50, 100));
        assert!(change(56, 100) > change(50, 100));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clusters_far_apart_stay_apart_and_close_ones_merge() {
        let mut m = Plane::new(200, 100);
        for (x, y) in [(20, 50), (22, 50), (180, 50)] {
            m.set(x, y, 1.0);
        }
        let c = clusters(&m, 64);
        assert_eq!(c.len(), 2);
        assert!(clusters(&m, 1).len() == 1);
        assert!(clusters(&Plane::new(10, 10), 4).is_empty());
    }

    #[test]
    fn glare_is_filled_with_the_learned_fill_and_only_the_glare_changes() {
        let dir = std::env::temp_dir().join(format!("lc-glare-make-{}", std::process::id()));
        let src = photo();
        let grey = Arc::new(Grey(Default::default()));
        let made = make(&src, &Glare::default(), &Region::whole(), Some(grey.clone()), &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(made.fill, SpotFill::Lama);
        assert_eq!(grey.0.load(Ordering::Relaxed), 2, "one fill per cluster");
        assert!(made.coverage > 0.0 && made.coverage < 0.05);
        let r = super::super::area::save_in(&dir, &made.tiles, Some(Hash128(5)), made.reflection).unwrap();
        assert_eq!(r.mode, ReflectionMode::Highlights);
        let out = super::super::apply(&Arc::new(src.clone()), &r, Some(Hash128(5)), None).unwrap();
        assert!(out.get(150, 200)[0] < 0.35, "glare filled: {:?}", out.get(150, 200));
        assert!(out.get(450, 200)[0] < 0.35);
        assert_eq!(out.get(300, 50), src.get(300, 50), "away from the glare nothing changes");
        // a preview-sized render applies the same correction
        let small = Arc::new(resize(&src, 300, 200, Filter::Box));
        let preview = super::super::apply(&small, &r, Some(Hash128(5)), None).unwrap();
        assert!(preview.get(75, 100)[0] < 0.4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn without_the_learned_fill_the_classic_fill_is_used_and_no_glare_is_an_error() {
        let made = make(&photo(), &Glare::default(), &Region::whole(), None, &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(made.fill, SpotFill::Classic);
        let plain = Rgb32f::filled(100, 100, [0.3, 0.2, 0.14]);
        let e = make(&plain, &Glare::default(), &Region::whole(), None, &Arc::new(AtomicBool::new(false))).err().unwrap();
        assert!(e.contains("No glare found"), "{e}");
        let e = make(&photo(), &Glare::default(), &Region::whole(), None, &Arc::new(AtomicBool::new(true))).err().unwrap();
        assert_eq!(e, "Stopped");
    }

    #[test]
    fn an_area_keeps_the_search_inside_it() {
        let dir = std::env::temp_dir().join(format!("lc-glare-area-{}", std::process::id()));
        let src = photo();
        let left = Region { rect: [0.0, 0.0, 0.5, 1.0], alpha: None };
        let grey: Arc<dyn LearnedFill> = Arc::new(Grey(Default::default()));
        let made = make(&src, &Glare::default(), &left, Some(grey), &Arc::new(AtomicBool::new(false))).unwrap();
        let r = super::super::area::save_in(&dir, &made.tiles, Some(Hash128(6)), made.reflection).unwrap();
        let out = super::super::apply(&Arc::new(src.clone()), &r, Some(Hash128(6)), None).unwrap();
        assert!(out.get(150, 200)[0] < 0.35);
        assert_eq!(out.get(450, 200), src.get(450, 200), "glare outside the area stays");
        let _ = std::fs::remove_dir_all(dir);
    }
}
