//! Wrinkles v2, taking the lines away: where lines are, the skin is evened out across them and left as it is along them.
//!
//! A wrinkle runs one way: along it the skin barely changes, across it the light dips into the crease and, on a deeply
//! lined face, rises over the lit roll of skin before the next one. So at each pixel near a line (the lines found by
//! [`super::ridge`], their direction carried to the pixels around them):
//!
//! 1. the median of the picture a little way along the line ([`ALONG`]): the line's own profile, without the pores,
//!    freckles or grain on it (each is an odd one out along the line);
//! 2. that is averaged across the line, over about the spacing of the zone's lines ([`ACROSS`] of its widest line),
//!    reading skin only: the light the skin would have without the lines;
//! 3. the change is the second less the first, per channel. What the first left out (the pores, the grain) stays.
//!
//! Creases come up and the lit ridges between them come down to the skin's own level, so a lined forehead does not turn
//! paler than the skin around it, and a crease's colour (darker, redder) evens out with it. Away from the lines
//! ([`presence`]) nothing changes.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f, par_rows};

use super::heal::{MOST_LIGHTER, smoothstep};
use super::ridge::{Centre, bilinear};

/// How far along a line the median is taken (half the length): line widths, and at least [`ALONG_MIN`] of the face (a
/// freckle is then always the smaller part of it).
const ALONG: f32 = 4.0;
const ALONG_MIN: f64 = 0.008;
const ALONG_STEPS: usize = 13;
/// The blur across the lines (Gaussian sigma): this many of the zone's widest lines.
pub(super) const ACROSS: f32 = 1.6;
/// ...and at least this many of the line's own widths, so a wide line is evened against the skin beside it.
pub(super) const ACROSS_LINE: f32 = 4.0;
/// The change is smoothed along the lines over this many pixels each way: the medians it is made of vary a little
/// from pixel to pixel, and that must not show as grain.
const SMOOTH_ALONG: i32 = 2;
/// The spread of the lines' presence and direction around them (sigma), a fraction of the face: wide enough that where
/// lines lie close (a lined forehead) the skin between them is evened with them.
const SPREAD: f64 = 0.015;
/// Steps across a line, each side of the pixel, per sigma of the blur (and at least one pixel apart).
const ACROSS_STEPS: f32 = 4.0;
/// The most a pixel is darkened (log light): a lit ridge between two creases comes down to the skin's level, no further.
const DARKEST: f32 = -0.2;
/// A channel's change stays within this of the luminance's (log light): the colour evens out, nothing is painted.
const TINT: f32 = 0.12;

fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}

/// Where the lines are and which way they run, spread to the pixels around them: per pixel the presence (0…1), the unit
/// direction across the lines, their typical width and the blur across them (pixels), and how much of them goes at 100.
pub(super) struct Field {
    pub presence: Plane,
    pub n: Vec<(f32, f32)>,
    pub sigma: Plane,
    pub across: Plane,
    pub keep: Plane,
}

/// The lines' [`Field`] over a `w × h` crop whose face is `size` pixels across.
pub(super) fn presence(centres: &[Centre], w: usize, h: usize, size: f64) -> Field {
    // per pixel on a line: its weight, the direction's tensor (so opposite normals add up), its width, blur and share
    let mut planes: [Plane; 7] = std::array::from_fn(|_| Plane::new(w, h));
    for c in centres {
        let (x, y) = (c.x.floor(), c.y.floor());
        if !(x >= 0.0 && y >= 0.0) || x as usize >= w || y as usize >= h {
            continue;
        }
        let i = y as usize * w + x as usize;
        let (nx, ny) = c.n;
        for (p, v) in planes.iter_mut().zip([1.0, nx * nx, nx * ny, ny * ny, c.sigma, c.across, c.keep]) {
            if let Some(d) = p.data.get_mut(i) {
                *d += v;
            }
        }
    }
    let spread = ((SPREAD * size) as f32).max(1.0);
    let [count, xx, xy, yy, sg, ac, kp] = planes.map(|p| gaussian(&p, spread));
    // a single line, blurred, peaks at 1 / (√(2π) spread): presence is 1 there and within about a spread of it
    let peak = 1.0 / ((2.0 * std::f32::consts::PI).sqrt() * spread);
    let presence = count.map(|c| smoothstep(0.05 * peak, 0.6 * peak, c));
    let at = |p: &Plane, i: usize| p.data.get(i).copied().unwrap_or(0.0);
    let mut n = vec![(1.0, 0.0); w * h];
    par_rows(&mut n, w.max(1), |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (a, b, d) = (at(&xx, i), at(&xy, i), at(&yy, i));
            // (no line near: θ = ½·atan2(0, 0) = 0, the value it is given, without the trigonometry)
            if a == 0.0 && b == 0.0 && d == 0.0 {
                continue;
            }
            let theta = 0.5 * (2.0 * b).atan2(a - d);
            *o = (theta.cos(), theta.sin());
        }
    });
    let mean = |p: &Plane| p.zip_map(&count, |v, c| if c > 1e-6 { v / c } else { 0.0 });
    Field { presence, n, sigma: mean(&sg), across: mean(&ac), keep: mean(&kp) }
}

/// 1 within a line's core (one and a half line widths of its centre), 0 elsewhere, over a `w × h` crop.
pub(super) fn cores(centres: &[Centre], w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h);
    for c in centres {
        let r = (CORE * c.sigma).max(1.5);
        let (x0, x1) = ((c.x - r).floor().max(0.0) as usize, ((c.x + r).ceil().max(0.0) as usize).min(w));
        let (y0, y1) = ((c.y - r).floor().max(0.0) as usize, ((c.y + r).ceil().max(0.0) as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = (x as f32 + 0.5 - c.x, y as f32 + 0.5 - c.y);
                if dx * dx + dy * dy <= r * r
                    && let Some(v) = out.data.get_mut(y * w + x)
                {
                    *v = 1.0;
                }
            }
        }
    }
    out
}

/// A line's core, in line widths each side of its centre.
const CORE: f32 = 1.5;

/// 1 at every pixel of a `w × h` crop the evening's second step reads (bilinearly, at `j·step` across the line from
/// each skin pixel `changed` says it changes, `steps_at` giving the spacing and count), 0 elsewhere: the pixels whose
/// median along the line the first step must make.
fn read_across(
    field: &Field,
    skin: &Plane,
    w: usize,
    h: usize,
    changed: impl Fn(usize) -> bool + Sync,
    steps_at: impl Fn(usize) -> (f32, f32, i32) + Sync,
) -> Plane {
    if !super::skip_idle() {
        return Plane::filled(w, h, 1.0);
    }
    use std::sync::atomic::{AtomicBool, Ordering};
    let marks: Vec<AtomicBool> = (0..w.saturating_mul(h)).map(|_| AtomicBool::new(false)).collect();
    // (rows side by side: a row marks pixels of other rows, hence the atomic marks)
    let mut rows = vec![(); h];
    par_rows(&mut rows, 1, |y, _| {
        for x in 0..w {
            let i = y * w + x;
            if skin.data.get(i).is_none_or(|s| *s < 0.5) || !changed(i) {
                continue;
            }
            let (nx, ny) = field.n.get(i).copied().unwrap_or((0.0, 1.0));
            let (_, step, steps) = steps_at(i);
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            for j in -steps..=steps {
                let u = j as f32 * step;
                let (qx, qy) = (px + u * nx, py + u * ny);
                if !(qx.is_finite() && qy.is_finite()) {
                    continue;
                }
                // the top-left pixel of the four a bilinear read takes (the others are one step on: the dilation below)
                let at = |q: f32, n: usize| ((q - 0.5).floor().max(0.0) as usize).min(n.saturating_sub(1));
                if let Some(m) = marks.get(at(qy, h) * w + at(qx, w)) {
                    m.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    let seeds = Plane { width: w, height: h, data: marks.iter().map(|m| if m.load(Ordering::Relaxed) { 1.0 } else { 0.0 }).collect() };
    lightcraft_raster::morph::max_filter(&seeds, 1)
}

/// The change (log light per channel) that evens the log picture `lg` out across the lines of `field`, reading skin only
/// where `skin` (0…1) is, and changing a pixel only as far as `allowed` (0…1) lets it.
pub(super) fn flatten(lg: &Rgb32f, field: &Field, skin: &Plane, allowed: &Plane, size: f64) -> Rgb32f {
    let along_min = ((ALONG_MIN * size) as f32).max(3.0);
    let (w, h) = (lg.width, lg.height);
    let mut out = Rgb32f::new(w, h);
    let n = w.saturating_mul(h);
    if skin.data.len() != n || allowed.data.len() != n || field.presence.data.len() != n || field.n.len() != n {
        return out;
    }
    // the median along the line at a point (by light; skin only): a freckle or a pore on the line is an odd one out
    // and is not the line. `None` where too little of the way is skin.
    let along = |qx: f32, qy: f32, t: (f32, f32), half: f32| -> Option<[f32; 3]> {
        let mut picks: [([f32; 3], f32); ALONG_STEPS] = [([0.0; 3], 0.0); ALONG_STEPS];
        let mut k = 0usize;
        for j in 0..ALONG_STEPS {
            let v = (j as f32 / (ALONG_STEPS - 1) as f32 * 2.0 - 1.0) * half;
            let (sx, sy) = (qx + v * t.0, qy + v * t.1);
            if bilinear(skin, sx, sy) < 0.5 {
                continue;
            }
            let p = lg.sample_bilinear(sx, sy);
            if let Some(slot) = picks.get_mut(k) {
                *slot = (p, luma(p));
                k += 1;
            }
        }
        if k < ALONG_STEPS / 2 + 1 {
            return None;
        }
        let picked = picks.get_mut(..k)?;
        picked.sort_by(|a, b| a.1.total_cmp(&b.1));
        picked.get(k / 2).map(|p| p.0)
    };
    // how much of a pixel's change goes (0: none, and nothing is read for it)
    let share = |i: usize| {
        field.presence.data.get(i).copied().unwrap_or(0.0)
            * field.keep.data.get(i).copied().unwrap_or(0.0)
            * allowed.data.get(i).copied().unwrap_or(0.0)
    };
    // the steps across the line at a pixel: their spacing and count each side (the profile is read at `u = j·step`)
    let steps_at = |i: usize| {
        let sigma = field.across.data.get(i).copied().unwrap_or(0.0).max(1.0);
        let step = (sigma / ACROSS_STEPS).max(1.0);
        (sigma, step, (3.0 * sigma / step).ceil() as i32)
    };
    let needed = read_across(field, skin, w, h, |i| share(i) > 0.0, steps_at);
    // 1. every skin pixel the evening reads: the median along the lines' direction there (the field spreads it past
    // the lines). (Only those: on a lined face most skin is too far from a line to be read, issue #253.)
    let mut med: Vec<Option<[f32; 3]>> = vec![None; n];
    par_rows(&mut med, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            if skin.data.get(i).is_none_or(|s| *s < 0.5) || needed.data.get(i).is_none_or(|v| *v <= 0.0) {
                continue;
            }
            let (nx, ny) = field.n.get(i).copied().unwrap_or((0.0, 1.0));
            let half = (ALONG * field.sigma.data.get(i).copied().unwrap_or(1.0)).max(along_min);
            *o = along(x as f32 + 0.5, y as f32 + 0.5, (-ny, nx), half);
        }
    });
    let weight = Plane { width: w, height: h, data: med.iter().map(|m| if m.is_some() { 1.0 } else { 0.0 }).collect() };
    let medians = Rgb32f { width: w, height: h, data: med.iter().map(|m| m.unwrap_or([0.0; 3])).collect() };
    // 2. each pixel near a line: the Gaussian mean of those medians across the line (skin only), less its own
    par_rows(&mut out.data, w, |y, row| {
        // (one list of picks for the row's pixels)
        let mut picks: Vec<([f32; 3], f32, f32)> = Vec::new();
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let k = share(i);
            let Some(Some(own)) = med.get(i).copied() else { continue };
            if k <= 0.0 {
                continue;
            }
            let (nx, ny) = field.n.get(i).copied().unwrap_or((0.0, 1.0));
            let (sigma, step, steps) = steps_at(i);
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // the profile's weighted median by light (each step weighed by a Gaussian and by its skin): for a lone line in
            // clear skin that is the clear skin, for creases between lit ridges the level between them, on a slope the
            // middle of it
            picks.clear();
            let mut total = 0.0f32;
            for j in -steps..=steps {
                let u = j as f32 * step;
                let (qx, qy) = (px + u * nx, py + u * ny);
                let on = bilinear(&weight, qx, qy);
                // (off the skin first: its weight is not needed there)
                if on < 0.5 {
                    continue;
                }
                let g = (-0.5 * (u / sigma).powi(2)).exp() * on;
                if g <= 1e-4 {
                    continue;
                }
                // (the medians are zero off the skin: a point between skin and none is divided back by its share)
                let p = medians.sample_bilinear(qx, qy).map(|v| v / on);
                picks.push((p, luma(p), g));
                total += g;
            }
            if total <= 1e-3 {
                continue;
            }
            picks.sort_by(|a, b| a.1.total_cmp(&b.1));
            let mut run = 0.0f32;
            let Some(flat) = picks.iter().find(|p| {
                run += p.2;
                run >= 0.5 * total
            }) else {
                continue;
            };
            let flat = flat.0;
            let d: [f32; 3] = std::array::from_fn(|c| flat[c] - own[c]);
            let y = luma(d);
            *o = d.map(|v| {
                let v = k * v.clamp(y - TINT, y + TINT);
                if v.is_finite() { v.clamp(DARKEST, MOST_LIGHTER) } else { 0.0 }
            });
        }
    });
    // smoothed along the lines (never across them: a fine line's change is a pixel or two wide)
    let raw = out;
    let mut out = Rgb32f::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            if raw.data.get(i).is_none_or(|p| *p == [0.0; 3]) {
                continue;
            }
            let (nx, ny) = field.n.get(i).copied().unwrap_or((0.0, 1.0));
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut sum = [0.0f32; 3];
            for v in -SMOOTH_ALONG..=SMOOTH_ALONG {
                let p = raw.sample_bilinear(px - v as f32 * ny, py + v as f32 * nx);
                sum.iter_mut().zip(p).for_each(|(a, b)| *a += b);
            }
            *o = sum.map(|v| v / (2 * SMOOTH_ALONG + 1) as f32);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skin with dark, reddish lines across it every 16 rows (sigma 1.5 px, 25 % darker) and pores.
    fn lined() -> Rgb32f {
        Rgb32f::from_fn(160, 160, |x, y| {
            let d = ((y % 16) as f32 + 0.5 - 8.0) / 1.5;
            let line = (-0.5 * d * d).exp();
            let pore = if (x * 7 + y * 3).is_multiple_of(11) { -0.06 } else { 0.0 };
            [0.45f32.ln() - 0.25 * line, 0.3f32.ln() - 0.32 * line, 0.24f32.ln() - 0.3 * line].map(|v| v + pore)
        })
    }

    fn lines(w: usize, h: usize) -> Vec<Centre> {
        let mut c = Vec::new();
        for y in (8..h).step_by(16) {
            for x in 0..w {
                c.push(Centre { x: x as f32 + 0.5, y: y as f32, n: (0.0, 1.0), sigma: 1.5, keep: 1.0, reach: 8.0, across: 8.0 });
            }
        }
        c
    }

    #[test]
    fn the_lines_even_out_in_light_and_colour_and_the_pores_stay() {
        let lg = lined();
        let field = presence(&lines(160, 160), 160, 160, 400.0);
        let one = Plane::filled(160, 160, 1.0);
        let d = flatten(&lg, &field, &one, &one, 400.0);
        let after = |x: usize, y: usize| -> [f32; 3] { std::array::from_fn(|c| lg.get(x, y)[c] + d.get(x, y)[c]) };
        // a row through a line's middle and a row between two lines end up alike, channel by channel (both near the mean)
        let row = |y: usize, c: usize| {
            let v: Vec<f32> = (40..120).filter(|x| !(x * 7 + y * 3).is_multiple_of(11)).map(|x| after(x, y)[c]).collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        for c in 0..3 {
            let (on, off) = (row(72, c), row(64, c));
            assert!((on - off).abs() < 0.03, "channel {c}: on the line {on}, between {off}");
        }
        // no paler patch: the evened skin is at the level of the clear skin between the lines, not above it
        let mean = (32..128).flat_map(|y| (32..128).map(move |x| (x, y))).map(|(x, y)| luma(after(x, y))).sum::<f32>() / (96.0 * 96.0);
        let clear = luma([0.45f32.ln(), 0.3f32.ln(), 0.24f32.ln()]);
        assert!(mean - clear < 0.01 && clear - mean < 0.03, "the evened light {mean:.3} against the clear skin's {clear:.3}");
        // a pore on a line is still a pore against its neighbours along the line
        let x = (40..120usize).find(|x| (x * 7 + 72 * 3).is_multiple_of(11)).unwrap_or(0);
        let pore = after(x + 1, 72)[1] - after(x, 72)[1];
        assert!(pore > 0.035, "the pore stays: {pore}");
    }

    #[test]
    fn far_from_the_lines_and_outside_the_allowed_part_nothing_changes() {
        let lg = lined();
        let mut few = lines(160, 160);
        few.retain(|c| c.y < 30.0);
        let field = presence(&few, 160, 160, 200.0);
        let one = Plane::filled(160, 160, 1.0);
        let d = flatten(&lg, &field, &one, &one, 200.0);
        assert!(d.get(80, 120).iter().all(|v| *v == 0.0), "far from the lines");
        let half = Plane::from_fn(160, 160, |x, _| if x < 80 { 1.0 } else { 0.0 });
        let all = presence(&lines(160, 160), 160, 160, 200.0);
        let d = flatten(&lg, &all, &one, &half, 200.0);
        assert!(d.get(120, 72).iter().all(|v| *v == 0.0), "not allowed");
        assert!(d.get(40, 72)[1] > 0.1, "allowed");
        assert!(flatten(&lg, &all, &Plane::new(3, 3), &one, 200.0).data.iter().all(|p| *p == [0.0; 3]));
    }
}
