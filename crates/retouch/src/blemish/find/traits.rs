//! What a found spot looks like up close, measured on the reduced skin picture: its darkening per colour, how its
//! darkness falls off from the centre (a sharp edge or a soft one), a red halo around it, and whether the darkness
//! carries on past it (a crease or a hair through it, or something larger and darker beside it).

use std::f64::consts::TAU;

use super::{Found, Work, absorb};

/// One spot's look ([`measure`]). Darkenings are natural logs (positive: darker than the skin around the spot).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Traits {
    /// Red, green and blue darkening of the spot's core against the skin 2.4–3.4 radii out.
    pub d: [f32; 3],
    /// The absorbing channel's darkening of the core.
    pub dark: f32,
    /// The absorbing channel's darkening at 1.0 and 1.5 radii, as a share of the core's: a sharp-edged spot keeps
    /// most of its darkness to its edge and none past it, a soft one fades all the way.
    pub edge: f32,
    pub rim: f32,
    /// How much redder (ln R/G) the skin 1.3–1.9 radii out is than the skin farther out, past what the spot's own
    /// fading edge explains: an inflamed halo around a mark.
    pub halo: f32,
    /// How dark the skin 2–3.5 radii out is on both sides along the darkest line through the spot, as a share of the
    /// spot's darkening: a crease, a wrinkle or a hair carries on past it, a spot does not.
    pub line: f32,
    /// How dark the darkest sector (a sixth of the way round) 2–3.5 radii out is, as a share of the spot's
    /// darkening: a spot at the edge of something larger and darker (a nostril, a fold, a lip corner, a shadow) is
    /// part of it.
    pub side: f32,
}

/// Bilinear sample of the weighted picture at work pixel position (`x`, `y`) (centres at +0.5); 0 outside it.
fn tap(w: &Work, x: f64, y: f64) -> [f32; 4] {
    if !(x.is_finite() && y.is_finite()) || w.w == 0 || w.h == 0 {
        return [0.0; 4];
    }
    let (fx, fy) = ((x - 0.5).clamp(0.0, (w.w - 1) as f64), (y - 0.5).clamp(0.0, (w.h - 1) as f64));
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w.w - 1), (y0 + 1).min(w.h - 1));
    let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let px = |x: usize, y: usize| w.weighted.data.get(y * w.w + x).copied().unwrap_or_default();
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bottom = c[i] + (d[i] - c[i]) * tx;
        top + (bottom - top) * ty
    })
}

/// The sum of `n` taps on a circle of radius `r` around `c`, turned by `turn` of a step (one tap at the centre when
/// `r` is 0).
fn ring(w: &Work, c: (f64, f64), r: f64, n: usize, turn: f64) -> [f32; 4] {
    if r <= 0.0 {
        return tap(w, c.0, c.1);
    }
    (0..n).fold([0.0; 4], |acc, k| {
        let a = (k as f64 + turn) * TAU / n as f64;
        let p = tap(w, c.0 + r * a.cos(), c.1 + r * a.sin());
        add(acc, p)
    })
}

fn add(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] + b[i])
}

/// R, G, B and the absorbing channel of a weighted sum, divided by its mask; `None` with no skin in it.
fn colour(p: [f32; 4]) -> Option<[f32; 4]> {
    (p[3] > 1e-4).then(|| [p[0] / p[3], p[1] / p[3], p[2] / p[3], absorb(&p) / p[3]])
}

fn ln(a: f32, b: f32) -> f32 {
    if a > 1e-6 && b > 1e-6 && a.is_finite() && b.is_finite() { (a / b).ln() } else { 0.0 }
}

/// The traits of spot `f`; `None` when there is no skin to compare it with.
pub(super) fn measure(w: &Work, f: &Found) -> Option<Traits> {
    let c = (f.x as f64 + 0.5, f.y as f64 + 0.5);
    let r = f.radius.max(0.75);
    let core = colour(add(ring(w, c, 0.0, 1, 0.0), ring(w, c, 0.4 * r, 8, 0.0)))?;
    let around = colour(add(ring(w, c, 2.4 * r, 24, 0.0), ring(w, c, 3.4 * r, 24, 0.5)))?;
    let edge = colour(ring(w, c, 1.0 * r, 16, 0.0))?;
    let rim = colour(ring(w, c, 1.5 * r, 16, 0.5))?;
    let halo = colour(add(ring(w, c, 1.3 * r, 16, 0.0), ring(w, c, 1.9 * r, 16, 0.5)))?;
    let dark = ln(around[3], core[3]);
    let d = [ln(around[0], core[0]), ln(around[1], core[1]), ln(around[2], core[2])];
    let share = |p: [f32; 4]| if dark > 1e-4 { ln(around[3], p[3]) / dark } else { 0.0 };
    let redness = |p: [f32; 4]| ln(p[0], p[1]);
    // per direction, the darkening 2 and 3.5 radii out
    let dirs = 16;
    let darkness: Vec<f32> = (0..dirs)
        .map(|k| {
            let a = k as f64 * TAU / dirs as f64;
            let p = add(tap(w, c.0 + 2.0 * r * a.cos(), c.1 + 2.0 * r * a.sin()), tap(w, c.0 + 3.5 * r * a.cos(), c.1 + 3.5 * r * a.sin()));
            colour(p).map_or(0.0, |p| ln(around[3], p[3]))
        })
        .collect();
    let at = |i: usize| darkness.get(i % dirs).copied().unwrap_or(0.0);
    let half = dirs / 2;
    // a line through it: the darkest pair of opposite directions (a line one direction off either way still counts)
    let line = (0..half)
        .map(|k| {
            let one = at(k).max(0.5 * (at(k + dirs - 1) + at(k + 1)));
            let other = at(k + half).max(0.5 * (at(k + half - 1) + at(k + half + 1)));
            one.min(other)
        })
        .fold(0.0f32, f32::max);
    // something beside it: the darkest three neighbouring directions
    let side = (0..dirs).map(|k| (at(k + dirs - 1) + at(k) + at(k + 1)) / 3.0).fold(0.0f32, f32::max);
    let per_dark = |v: f32| if dark > 1e-4 { v / dark } else { 0.0 };
    Some(Traits {
        d,
        dark,
        edge: share(edge),
        rim: share(rim),
        // the spot's own colour, as far as its darkness reaches into the ring, is not a halo
        halo: (redness(halo) - redness(around)) - share(halo).clamp(0.0, 1.0) * (redness(core) - redness(around)),
        line: per_dark(line),
        side: per_dark(side),
    })
}
