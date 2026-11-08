//! The fitter: renders the target's small copy through the real develop pipeline, measures it ([`crate::stats`]) and
//! moves one slider at a time while that brings the look closer to the reference's (coordinate descent with
//! growing and halving steps). The sliders go in stages, broad first: white balance and exposure, then tone, then
//! colourfulness, the colour mixer, colour grading, and last a master tone curve for what the sliders left over.
//!
//! Every stage starts from the photo's own settings and keeps a change only when it lowers the loss, so a photo
//! matched to itself stays as it is.

use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::{DevelopSettings, MIXER_BANDS, ToneCurve};
use lightcraft_geom::Point;
use lightcraft_pipeline::{Quality, RenderRequest, SourceInfo};
use lightcraft_raster::{Plane, Rgb32f};

use crate::params::{Param, Zone};
use crate::stats::{self, MIN_BAND_SHARE, Stats};
use crate::{LookError, MatchOptions};

/// Why a stage stopped early.
pub(crate) enum Stop {
    Budget,
    Cancelled,
}

/// One region of the target, with the measured look of its counterpart in the reference.
pub(crate) struct Goal {
    pub mask: Option<Plane>,
    pub stats: Stats,
    pub weight: f64,
}

pub(crate) struct Fitter<'a> {
    pub src: &'a Rgb32f,
    pub info: &'a SourceInfo,
    pub req: RenderRequest,
    pub goals: Vec<Goal>,
    pub evals: usize,
    pub budget: usize,
    pub cancel: &'a AtomicBool,
    pub progress: &'a (dyn Fn(usize, usize) + Sync),
    /// Where the photo's settings started (the prior pulls towards it).
    pub start: DevelopSettings,
    /// Every number any stage may move (the prior is summed over them).
    pub all: Vec<Param>,
    /// The colour mixer bands that are a large part of both pictures (their sliders may move further).
    pub large_bands: Vec<usize>,
}

/// Renders kept for the residual Look ([`Fitter::fit_look`]: up to 4 rounds of 4 parts × 3 strengths, usually far
/// fewer).
pub(crate) const LOOK_BUDGET: usize = 48;

/// A mixer band counts as large once it holds this share of both pictures.
pub(crate) const LARGE_BAND_SHARE: f32 = 0.08;

/// The improvement that counts (relative to the loss, plus a floor far below anything visible).
pub(crate) fn tolerance(best: f64) -> f64 {
    best * 1e-4 + 1e-9
}

impl Fitter<'_> {
    /// Render `d` and say how far its look is from the reference's.
    pub fn loss_of(&mut self, d: &DevelopSettings) -> Result<f64, Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        if self.evals >= self.budget {
            return Err(Stop::Budget);
        }
        self.evals += 1;
        (self.progress)(self.evals, self.budget);
        let img = lightcraft_pipeline::render(self.src, self.info, d, &self.req).image;
        let mut e = 0.0;
        for g in &self.goals {
            e += g.weight * stats::loss(&stats::measure(&img, g.mask.as_ref()), &g.stats);
        }
        e += self.all.iter().map(|p| p.prior(p.get(&self.start), p.get(d))).sum::<f64>();
        Ok(if e.is_finite() { e } else { f64::MAX })
    }

    /// Step each of `params` while it lowers the loss, for at most `passes` rounds.
    fn descend(&mut self, d: &mut DevelopSettings, best: &mut f64, params: &[Param], passes: usize) -> Result<(), Stop> {
        let mut steps: Vec<f64> = params.iter().map(Param::step).collect();
        for _ in 0..passes {
            let mut moved = false;
            for (p, step) in params.iter().zip(steps.iter_mut()) {
                if *step < p.min_step() {
                    continue;
                }
                if self.try_param(d, best, p, *step)? {
                    moved = true;
                } else {
                    *step *= 0.5;
                }
            }
            if !moved && params.iter().zip(&steps).all(|(p, s)| *s < p.min_step() * 2.0) {
                break;
            }
        }
        Ok(())
    }

    /// `v` kept within `p`'s cap around where the photo started ([`Param::cap`]) and within its band
    /// ([`Param::band`], widened to where the photo started when that is outside it). A bound that isn't a number
    /// (settings from a damaged file) keeps the current value.
    pub fn limit(&self, p: &Param, d: &DevelopSettings, v: f64) -> f64 {
        let now = p.get(d);
        let cap = p.cap(p.mixer_band().is_some_and(|b| self.large_bands.contains(&b)));
        let s0 = p.get(&self.start);
        let (lo, hi) = match p.other_axis() {
            Some(o) => {
                let (off, other) = (o.get(d) - o.get(&self.start), o.get(d));
                let reach = (cap * cap - off * off).max(0.0).sqrt();
                // the wheel's saturation stays within its band's radius (or where the photo had it)
                let radius = p.band().1.max(s0.hypot(o.get(&self.start)));
                let inside = (radius * radius - other * other).max(0.0).sqrt();
                ((s0 - reach).max(-inside), (s0 + reach).min(inside))
            }
            None => {
                let (blo, bhi) = p.band();
                ((s0 - cap).max(blo.min(s0)), (s0 + cap).min(bhi.max(s0)))
            }
        };
        if !(lo.is_finite() && hi.is_finite() && v.is_finite()) || lo > hi {
            return now;
        }
        v.clamp(lo, hi)
    }

    /// Try `p` one step up and down; on a better look keep going that way with growing steps. Whether it moved.
    fn try_param(&mut self, d: &mut DevelopSettings, best: &mut f64, p: &Param, step: f64) -> Result<bool, Stop> {
        let v0 = p.get(d);
        for dir in [1.0, -1.0] {
            let mut trial = d.clone();
            let v = p.set(&mut trial, self.limit(p, d, v0 + dir * step));
            if (v - v0).abs() < 1e-9 {
                continue;
            }
            let l = self.loss_of(&trial)?;
            if l < *best - tolerance(*best) {
                *d = trial;
                *best = l;
                let mut s = step;
                for _ in 0..6 {
                    s *= 2.0;
                    let mut further = d.clone();
                    let from = p.get(d);
                    let to = p.set(&mut further, self.limit(p, d, from + dir * s));
                    if (to - from).abs() < 1e-9 {
                        break;
                    }
                    let l = self.loss_of(&further)?;
                    if l < *best - tolerance(*best) {
                        *d = further;
                        *best = l;
                    } else {
                        break;
                    }
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The colour mixer bands both pictures have enough of.
    fn shared_bands(&self, current: &Stats) -> Vec<usize> {
        (0..MIXER_BANDS.len())
            .filter(|&i| {
                let t = current.bands.get(i).map_or(0.0, |b| b.share);
                t >= MIN_BAND_SHARE * 2.0 && self.goals.iter().any(|g| g.stats.bands.get(i).is_some_and(|b| b.share >= MIN_BAND_SHARE))
            })
            .collect()
    }

    pub(crate) fn measure(&mut self, d: &DevelopSettings) -> Stats {
        let img = lightcraft_pipeline::render(self.src, self.info, d, &self.req).image;
        stats::measure(&img, self.goals.first().and_then(|g| g.mask.as_ref()))
    }

    /// A master curve that maps the current lightness quantiles onto the reference's, when the photo has no master
    /// curve of its own; kept at the strength that helps most (or not at all).
    fn fit_curve(&mut self, d: &mut DevelopSettings, best: &mut f64) -> Result<(), Stop> {
        if !ToneCurve::point_curve_is_identity(&d.curve.master) {
            return Ok(());
        }
        let Some(goal) = self.goals.first().map(|g| g.stats.l_q) else { return Ok(()) };
        let now = self.measure(d).l_q;
        let enc = |l: f32| lightcraft_color::transfer::linear_to_srgb(l.clamp(0.0, 1.0).powi(3)) as f64;
        let mut pts = vec![Point::new(0.0, 0.0)];
        for (&t, &r) in now.iter().zip(&goal).skip(1).step_by(2) {
            let (x, y) = (enc(t), enc(r));
            let last = pts.last().copied().unwrap_or(Point::new(0.0, 0.0));
            if x > last.x + 0.04 && x < 0.97 && y > last.y && y < 1.0 {
                pts.push(Point::new(x, y));
            }
        }
        pts.push(Point::new(1.0, 1.0));
        if pts.iter().all(|p| (p.y - p.x).abs() < 0.004) {
            return Ok(());
        }
        let mut kept = None;
        for k in [1.0, 0.5] {
            let mut trial = d.clone();
            trial.curve.master = pts.iter().map(|p| Point::new(p.x, p.x + (p.y - p.x) * k)).collect();
            let l = self.loss_of(&trial)?;
            if l < *best - tolerance(*best) {
                *best = l;
                kept = Some(trial);
            }
        }
        if let Some(t) = kept {
            *d = t;
        }
        Ok(())
    }

    /// Fit `d` (the photo's own settings) towards the goals.
    pub fn run(&mut self, d: &mut DevelopSettings, opts: &MatchOptions) -> Result<(f64, f64), LookError> {
        let before = match self.loss_of(d) {
            Ok(l) => l,
            Err(Stop::Cancelled) => return Err(LookError::Cancelled),
            Err(Stop::Budget) => return Err(LookError::Invalid("the evaluation budget is too small".into())),
        };
        let mut best = before;
        // the residual Look has renders of its own, so sliders that use up theirs never leave it out
        let full = self.budget;
        if opts.look {
            self.budget = full.saturating_sub(LOOK_BUDGET).max(self.evals + 1);
        }
        match self.stages(d, &mut best, opts) {
            Ok(()) | Err(Stop::Budget) => {}
            Err(Stop::Cancelled) => return Err(LookError::Cancelled),
        }
        if opts.look {
            self.budget = self.evals.saturating_add(LOOK_BUDGET).max(full);
            match self.fit_look(d, &mut best) {
                Ok(()) | Err(Stop::Budget) => {}
                Err(Stop::Cancelled) => return Err(LookError::Cancelled),
            }
        }
        Ok((before, best))
    }

    fn stages(&mut self, d: &mut DevelopSettings, best: &mut f64, opts: &MatchOptions) -> Result<(), Stop> {
        use Param::{Control, Mired, Tint, Wheel};
        // white balance first: a cast is the white balance's to fix, before Saturation could drain it instead
        let mut broad = if opts.white_balance { vec![Mired, Tint] } else { Vec::new() };
        broad.push(Control("light.exposure"));
        self.descend(d, best, &broad, 5)?;
        let tone = [
            Control("light.exposure"),
            Control("light.contrast"),
            Control("light.highlights"),
            Control("light.shadows"),
            Control("light.whites"),
            Control("light.blacks"),
        ];
        self.descend(d, best, &tone, 4)?;
        // colourfulness and white balance together: Saturation scales a cast as well, so each corrects the other
        let mut colour = vec![Control("color.saturation"), Control("color.vibrance")];
        if opts.white_balance {
            colour.extend([Mired, Tint]);
        }
        self.descend(d, best, &colour, 5)?;
        if opts.grading {
            let wheels =
                [Zone::Midtones, Zone::Shadows, Zone::Highlights].into_iter().flat_map(|z| [Wheel(z, false), Wheel(z, true)]).collect::<Vec<_>>();
            self.descend(d, best, &wheels, 3)?;
        }
        if opts.mixer {
            let now = self.measure(d);
            self.large_bands = (0..MIXER_BANDS.len())
                .filter(|&i| {
                    now.bands.get(i).is_some_and(|b| b.share >= LARGE_BAND_SHARE)
                        && self.goals.iter().any(|g| g.stats.bands.get(i).is_some_and(|b| b.share >= LARGE_BAND_SHARE))
                })
                .collect();
            let mut mix = Vec::new();
            for i in self.shared_bands(&now) {
                for part in ["hue", "sat", "lum"] {
                    if let Some(id) = mixer_id(i, part) {
                        mix.push(Control(id));
                    }
                }
            }
            self.descend(d, best, &mix, 2)?;
        }
        if opts.curve {
            self.fit_curve(d, best)?;
        }
        let mut polish = vec![
            Control("light.exposure"),
            Control("light.contrast"),
            Control("color.saturation"),
            Wheel(Zone::Midtones, false),
            Wheel(Zone::Midtones, true),
        ];
        if opts.white_balance {
            polish.extend([Mired, Tint]);
        }
        self.descend(d, best, &polish, 1)?;
        Ok(())
    }
}

/// Every number [`Fitter::stages`] may move.
pub(crate) fn all_params() -> Vec<Param> {
    use Param::{Control, Mired, Tint, Wheel};
    let mut all = vec![Mired, Tint];
    all.extend(
        [
            "light.exposure",
            "light.contrast",
            "light.highlights",
            "light.shadows",
            "light.whites",
            "light.blacks",
            "color.vibrance",
            "color.saturation",
        ]
        .map(Control),
    );
    for i in 0..MIXER_BANDS.len() {
        all.extend(["hue", "sat", "lum"].iter().filter_map(|part| mixer_id(i, part)).map(Control));
    }
    for z in [Zone::Shadows, Zone::Midtones, Zone::Highlights] {
        all.extend([Wheel(z, false), Wheel(z, true)]);
    }
    all
}

/// The control id of a mixer band's `part` (`mixer.blue.sat`), from the develop crate's own list.
fn mixer_id(band: usize, part: &str) -> Option<&'static str> {
    let name = MIXER_BANDS.get(band)?;
    lightcraft_develop::CONTROLS.iter().map(|c| c.id).find(|id| id.strip_prefix("mixer.").and_then(|r| r.split_once('.')) == Some((name, part)))
}

/// A render request for the fitter's small copies.
pub(crate) fn request(size: usize) -> RenderRequest {
    RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(size, size) }
}
