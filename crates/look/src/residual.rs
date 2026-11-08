//! The residual Look (`fork/match-look-followups.md`): after the sliders, a colour lookup in OkLab
//! ([`lightcraft_develop::ResidualLook`]) for what they could not reach. Its offsets are read straight from the
//! measured looks (the lightness quantiles, each zone's cast, the colourfulness, each mixer band shared by both
//! pictures), refined over a few renders through the real pipeline, kept within gentle limits, and kept at all only
//! when the photo comes closer to the reference.

use lightcraft_develop::{DevelopSettings, LOOK_KNOTS, ResidualLook};

use crate::fit::LARGE_BAND_SHARE;
use crate::fit::{Fitter, Stop};
use crate::stats::{L_QUANTILES, Stats};

/// Furthest each kind of offset goes (OkLab units), so the Look stays a nudge: the sliders and the curve already did
/// the broad work, and a stronger Look turned skin orange on techneissa's own pair.
const CAP_L: f64 = 0.04;
const CAP_CAST: f64 = 0.03;
const CAP_CHROMA: f64 = 0.15;
const CAP_BAND_L: f64 = 0.03;
const CAP_BAND_AB: f64 = 0.02;
/// Most the lightness offset changes from one knot to the next, so the lightness map stays smooth and rising.
const MAX_KNOT_STEP: f64 = 0.02;
/// Most smoothing passes; it settles in a few.
const SMOOTH_PASSES: usize = 64;
/// Offsets all below this are noise, well under a visible step (OkLab ~0.02): no Look.
const NOISE: f64 = 0.01;
/// A zone counts once it holds this share of both pictures.
const MIN_ZONE: f32 = 0.05;
/// Refinement rounds (two or three renders each).
const ROUNDS: usize = 4;
/// What the Look costs in the fit at its caps: enough that only a clear gain keeps it.
const PRIOR: f64 = 1e-3;

/// The cost of `look` (squared offsets on the scale of their caps).
fn prior(look: &ResidualLook) -> f64 {
    let sq = |v: f64, cap: f64| (v / cap).powi(2);
    let mut e = 0.0;
    e += look.lightness.iter().map(|v| sq(*v, CAP_L)).sum::<f64>() / LOOK_KNOTS as f64;
    e += look.cast.iter().flatten().map(|v| sq(*v, CAP_CAST)).sum::<f64>() / 3.0;
    e += sq(look.chroma, CAP_CHROMA);
    e += look.bands.iter().map(|b| sq(b[0], CAP_BAND_L) + sq(b[1], CAP_BAND_AB) + sq(b[2], CAP_BAND_AB)).sum::<f64>() / 4.0;
    PRIOR * e
}

fn clampf(v: f64, cap: f64) -> f64 {
    if v.is_finite() { v.clamp(-cap, cap) } else { 0.0 }
}

/// The lightness offset at `l` that maps the photo's quantiles `now` onto the reference's `goal` (0 at black and
/// white, linear between).
fn lightness_offset(now: &[f32; L_QUANTILES.len()], goal: &[f32; L_QUANTILES.len()], l: f32) -> f32 {
    let mut pts = vec![(0.0f32, 0.0f32)];
    for (t, r) in now.iter().zip(goal) {
        if *t > pts.last().map_or(0.0, |p| p.0) + 1e-3 && *t < 1.0 {
            pts.push((*t, r - t));
        }
    }
    pts.push((1.0, 0.0));
    for w in pts.windows(2) {
        let [(x0, y0), (x1, y1)] = [w[0], w[1]];
        if l <= x1 {
            let t = ((l - x0) / (x1 - x0).max(1e-6)).clamp(0.0, 1.0);
            return y0 + (y1 - y0) * t;
        }
    }
    0.0
}

/// Keep neighbouring lightness knots within [`MAX_KNOT_STEP`] of each other, the ends at 0.
fn smooth(knots: &mut [f64; LOOK_KNOTS]) {
    let last = LOOK_KNOTS - 1;
    knots[0] = 0.0;
    knots[last] = 0.0;
    for _ in 0..SMOOTH_PASSES {
        let before = *knots;
        // interior knots only: the ends stay pinned at 0
        for i in 1..last {
            let prev = knots[i - 1];
            knots[i] = knots[i].clamp(prev - MAX_KNOT_STEP, prev + MAX_KNOT_STEP);
        }
        for i in (1..last).rev() {
            let next = knots[i + 1];
            knots[i] = knots[i].clamp(next - MAX_KNOT_STEP, next + MAX_KNOT_STEP);
        }
        if *knots == before {
            break;
        }
    }
    let smooth_now = knots.windows(2).all(|w| (w[1] - w[0]).abs() <= MAX_KNOT_STEP + 1e-12);
    // (never seen: a map that did not settle is dropped rather than kept rough)
    if !smooth_now || knots.iter().any(|v| !v.is_finite()) {
        *knots = [0.0; LOOK_KNOTS];
    }
}

/// One refinement: `look` moved by what still separates the photo's look (`now`, rendered with `look`) from `goal`.
fn refine(look: &mut ResidualLook, now: &Stats, goal: &Stats) {
    for (k, v) in look.lightness.iter_mut().enumerate() {
        let l = k as f32 / (LOOK_KNOTS - 1) as f32;
        *v = clampf(*v + lightness_offset(&now.l_q, &goal.l_q, l) as f64, CAP_L);
    }
    smooth(&mut look.lightness);
    for z in 0..3 {
        let (Some(n), Some(g), Some(nw), Some(gw), Some(out)) =
            (now.zone_ab.get(z), goal.zone_ab.get(z), now.zone_w.get(z), goal.zone_w.get(z), look.cast.get_mut(z))
        else {
            continue;
        };
        let presence = (nw.min(*gw) / MIN_ZONE).min(1.0);
        out[0] = clampf(out[0] + ((g[0] - n[0]) * presence) as f64, CAP_CAST);
        out[1] = clampf(out[1] + ((g[1] - n[1]) * presence) as f64, CAP_CAST);
    }
    if now.c_mean > 0.005 && goal.c_mean > 0.0 {
        look.chroma = clampf((1.0 + look.chroma) * (goal.c_mean / now.c_mean) as f64 - 1.0, CAP_CHROMA);
    }
    for ((nb, gb), out) in now.bands.iter().zip(&goal.bands).zip(look.bands.iter_mut()) {
        // a hue that is a large part of both pictures (as for the mixer's caps); a smaller one is a colour the two
        // share by chance (a face against a yellow shirt) and stays as it is
        if nb.share < LARGE_BAND_SHARE || gb.share < LARGE_BAND_SHARE {
            continue;
        }
        out[0] = clampf(out[0] + (gb.l - nb.l) as f64, CAP_BAND_L);
        out[1] = clampf(out[1] + (gb.a - nb.a) as f64, CAP_BAND_AB);
        out[2] = clampf(out[2] + (gb.b - nb.b) as f64, CAP_BAND_AB);
    }
}

/// Rounded to 4 decimals, so the saved edit stays short.
fn rounded(look: &ResidualLook) -> ResidualLook {
    let r = |v: &mut f64| *v = (*v * 1e4).round() / 1e4;
    let mut l = look.clone();
    l.lightness.iter_mut().chain(l.cast.iter_mut().flatten()).chain(l.bands.iter_mut().flatten()).for_each(r);
    r(&mut l.chroma);
    l
}

fn biggest(look: &ResidualLook) -> f64 {
    look.lightness
        .iter()
        .chain(look.cast.iter().flatten())
        .chain(look.bands.iter().flatten())
        .chain([&look.chroma])
        .fold(0.0f64, |m, v| m.max(v.abs()))
}

impl Fitter<'_> {
    /// Fit the residual Look on top of `d`'s sliders; `d.look` is set only when it brings the photo closer. Each round
    /// measures the photo with the Look so far and works out what still separates the two; then each part of the Look
    /// (cast, colourfulness, hue bands, lightness) moves all, half or a quarter of the way, kept only when the loss
    /// drops. Running out of renders keeps what was found.
    pub(crate) fn fit_look(&mut self, d: &mut DevelopSettings, best: &mut f64) -> Result<(), Stop> {
        let Some(goal) = self.goals.first().map(|g| g.stats.clone()) else { return Ok(()) };
        let mut trial = d.clone();
        trial.look = ResidualLook::default();
        let mut found = false;
        let mut low = *best;
        'rounds: for _ in 0..ROUNDS {
            let now = self.measure(&trial);
            let mut target = trial.look.clone();
            refine(&mut target, &now, &goal);
            let mut moved = false;
            for part in Part::ALL {
                for k in [1.0, 0.5, 0.25] {
                    let step = rounded(&part.lerp(&trial.look, &target, k));
                    if step == trial.look {
                        break;
                    }
                    let mut t = trial.clone();
                    t.look = step;
                    let l = match self.loss_of(&t) {
                        Ok(l) => l + prior(&t.look),
                        Err(Stop::Budget) => break 'rounds,
                        Err(e) => return Err(e),
                    };
                    if l < low - crate::fit::tolerance(low) {
                        low = l;
                        trial = t;
                        moved = true;
                        found = true;
                        break;
                    }
                }
            }
            if !moved {
                break;
            }
        }
        if found && biggest(&trial.look) >= NOISE {
            *d = trial;
            *best = low;
        }
        Ok(())
    }
}

/// A part of the Look that moves on its own.
#[derive(Clone, Copy)]
enum Part {
    Cast,
    Chroma,
    Bands,
    Lightness,
}

impl Part {
    const ALL: [Part; 4] = [Part::Cast, Part::Chroma, Part::Bands, Part::Lightness];

    /// `a` with this part moved `k` of the way to `b`.
    fn lerp(self, a: &ResidualLook, b: &ResidualLook, k: f64) -> ResidualLook {
        let mut r = a.clone();
        let f = |x: &mut f64, y: &f64| *x += (y - *x) * k;
        match self {
            Part::Lightness => r.lightness.iter_mut().zip(&b.lightness).for_each(|(x, y)| f(x, y)),
            Part::Cast => r.cast.iter_mut().flatten().zip(b.cast.iter().flatten()).for_each(|(x, y)| f(x, y)),
            Part::Bands => r.bands.iter_mut().flatten().zip(b.bands.iter().flatten()).for_each(|(x, y)| f(x, y)),
            Part::Chroma => f(&mut r.chroma, &b.chroma),
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_quantiles_give_no_lightness_offset() {
        let q = [0.1, 0.15, 0.2, 0.3, 0.5, 0.6, 0.7, 0.8, 0.9];
        for l in [0.0, 0.25, 0.5, 0.95, 1.0] {
            assert_eq!(lightness_offset(&q, &q, l), 0.0);
        }
        let brighter = q.map(|v| v + 0.05);
        assert!((lightness_offset(&q, &brighter, 0.5) - 0.05).abs() < 1e-6);
    }

    #[test]
    fn the_lightness_map_stays_smooth() {
        let mut k = [0.0, 0.04, -0.04, 0.04, -0.04, 0.04, -0.04, 0.04, 0.03];
        smooth(&mut k);
        assert_eq!((k[0], k[LOOK_KNOTS - 1]), (0.0, 0.0));
        assert!(k.windows(2).all(|w| (w[1] - w[0]).abs() <= MAX_KNOT_STEP + 1e-12), "{k:?}");
    }

    #[test]
    fn random_lightness_maps_end_at_zero_and_stay_smooth() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..20_000 {
            let mut k = [0.0; LOOK_KNOTS];
            for v in &mut k {
                *v = (next() * 2.0 - 1.0) * CAP_L * 2.0;
            }
            smooth(&mut k);
            assert_eq!((k[0], k[LOOK_KNOTS - 1]), (0.0, 0.0), "{k:?}");
            assert!(k.windows(2).all(|w| (w[1] - w[0]).abs() <= MAX_KNOT_STEP + 1e-12), "{k:?}");
        }
    }

    #[test]
    fn refinement_stays_within_the_caps() {
        let now = Stats { weight: 1.0, c_mean: 0.01, zone_w: [0.3; 3], ..Stats::default() };
        let mut goal = now.clone();
        goal.l_q = [0.9; L_QUANTILES.len()];
        goal.zone_ab = [[0.5, -0.5]; 3];
        goal.c_mean = 1.0;
        let mut look = ResidualLook::default();
        for _ in 0..5 {
            refine(&mut look, &now, &goal);
        }
        assert!(look.lightness.iter().all(|v| v.abs() <= CAP_L));
        assert!(look.cast.iter().flatten().all(|v| v.abs() <= CAP_CAST));
        assert!(look.chroma.abs() <= CAP_CHROMA);
    }
}
