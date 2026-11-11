//! The loupe's prefetch while culling (#224, #223): the renders of the photos the user is about to step to, made
//! before they get there, so each is sharp the moment it is shown.
//!
//! - **Pace** ([`Pace`]): how fast and which way the user steps. A run of steps sets the time between them; a pause
//!   longer than [`PAUSE_SECS`] is no run (the calm plan), and a jump to another part of the shoot starts over.
//! - **Plan** ([`plan`]): from the pace and how long a prefetch takes, how far ahead to prepare, how many at once, and
//!   from which photo on. A photo the user will reach before a render started now could finish is started after the
//!   ones that can still be ready on arrival: chasing the nearest photo left every photo a little late.
//! - **The photo on screen** renders at once, first, at every step. What the user has left goes: its queued renders
//!   are dropped, and its render still running is told to stop ([`crate::render::Renderer::request`] with
//!   [`Slot::Main`]), so the workers are free for the photo they are on.
//! - **No waste.** A photo whose kept view already covers what the loupe draws is not rendered again (#223); prefetches
//!   still queued for photos stepped past are dropped; and the loupe takes over a prefetch running for the photo it
//!   lands on instead of decoding it a second time ([`crate::render::Renderer::adopt_prefetch`]).
//!
//! Memory stays bounded as before: the renders go to the preview cache, their decodes through the engine's memory
//! gate and source caches.

use egui::Rect;
use lightcraft_catalog::PhotoId;
use lightcraft_pipeline::geometry::Frame;

use super::look_ahead::BEHIND;
use super::{LightcraftApp, fit_rect, output_px, prep_mode};
use crate::render::Slot;

/// Neighbour prefetch: below on-screen thumbnails, above background thumbnail refreshes.
const PRIORITY: u32 = 4;
/// Photos prepared ahead at once at a calm pace (each render spreads over the cores already): the others wait, so the
/// photo the user steps to next finds a worker free.
const CALM_AT_ONCE: usize = 2;
/// At most, stepping fast.
const MOST_AT_ONCE: usize = 3;
/// Render workers prefetches leave free, for what the photo on screen needs at once: its render, its stand-in and the
/// thumbnails ahead. A running prefetch can't be stopped, so with fewer free a step found every worker busy and showed
/// nothing of the photo until one finished.
const FREE_WORKERS: usize = 3;
/// How far ahead stepping fast prepares at most, unless Settings ▸ Performance asks for more.
const FAST_DEPTH: usize = 12;
/// Prepared-ahead slots, by the photo's place in the view: a step keeps the photos still ahead in
/// the slots they have (nothing is asked for twice), and the jump to another part of the shoot
/// replaces what was waiting.
const SLOTS: usize = 32;
/// The furthest ahead any plan reaches: the photos ahead and behind keep slots of their own.
const MOST_DEPTH: usize = SLOTS - BEHIND - 1;
/// A photo stayed on longer than this (s) ends the run of steps: the user stopped to look.
pub(crate) const PAUSE_SECS: f64 = 2.0;
/// How much a step moves the run's time between steps.
const STEP_WEIGHT: f64 = 0.5;
/// A prefetch's time before one has been measured (s): a 24–45 MP raw on a laptop.
const RENDER_GUESS_SECS: f64 = 0.7;

pub(super) fn slot(at: usize) -> Slot {
    Slot::Prefetch((at % SLOTS) as u8)
}

/// How the user steps through the photos in the loupe.
#[derive(Debug, Default)]
pub struct Pace {
    /// The photo on screen, its place in the view and since when (s).
    at: Option<(PhotoId, usize, f64)>,
    /// The run's time between steps (s), smoothed: `None` before the second step of a run.
    step: Option<f64>,
    backward: bool,
}

impl Pace {
    /// The loupe shows photo `id`, at place `pos` of the view, in the frame at `now` (s). Whether it showed another
    /// photo before (a step or a jump).
    pub fn observe(&mut self, id: PhotoId, pos: usize, now: f64) -> bool {
        let moved = self.at.is_some_and(|(shown, _, _)| shown != id);
        match self.at {
            Some((shown, _, _)) if shown == id => return false,
            Some((_, last, since)) => {
                let dt = now - since;
                if last.abs_diff(pos) != 1 || !dt.is_finite() || dt < 0.0 {
                    // a jump (the grid, a filter, a click in the filmstrip): no run
                    self.step = None;
                } else if dt <= PAUSE_SECS {
                    self.step = Some(self.step.map_or(dt, |s| s + (dt - s) * STEP_WEIGHT));
                }
                // (a step after a pause keeps the run's pace: the user goes on culling at it)
                self.backward = pos < last;
            }
            None => {}
        }
        self.at = Some((id, pos, now));
        moved
    }

    /// The time between steps expected now (s): `None` when the user is not stepping (the loupe just opened, a jump,
    /// or a pause on this photo).
    pub fn step_secs(&self, now: f64) -> Option<f64> {
        let (_, _, since) = self.at?;
        self.step.filter(|_| now - since <= PAUSE_SECS)
    }

    /// Whether the last step went back.
    pub fn backward(&self) -> bool {
        self.backward
    }
}

/// What to prefetch, from [`plan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// How many photos ahead (in the direction of travel).
    pub depth: usize,
    /// How many prefetches may be queued or running at once.
    pub at_once: usize,
    /// The nearest photo ahead a prefetch started now can still finish for before the user gets there (1: the next).
    pub lead: usize,
}

impl Plan {
    /// Stepping faster than a photo renders: the photos ahead come first, before the one on screen is done.
    pub fn racing(self) -> bool {
        self.lead > 1
    }
}

/// The prefetch for `ahead` photos ahead (Settings ▸ Performance, at least 1), a step every `step_secs` (`None`: not
/// stepping), a prefetch taking `render_secs`, with `workers` render threads.
///
/// The user passes `ceil(render / step)` photos while one renders: the nearest a prefetch started now is ready for
/// (`lead`). Keeping up takes one more render at once than that, and the photos from `lead` on as deep as those
/// renders reach (`depth`, never less than the settings' count). At a calm pace that is the settings' count, two at
/// a time, nearest first.
pub(crate) fn plan(ahead: usize, step_secs: Option<f64>, render_secs: Option<f64>, workers: usize) -> Plan {
    let ahead = ahead.clamp(1, MOST_DEPTH);
    // (never every worker: on a 2–3 core Mac, with two, one stays for the photo on screen; with one, no prefetch
    // runs at all; 0 is web or a pool not started yet, which keeps one)
    let most = workers.saturating_sub(FREE_WORKERS).clamp(CALM_AT_ONCE, MOST_AT_ONCE).min(workers.saturating_sub(1)).max(usize::from(workers == 0));
    let render = render_secs.filter(|r| r.is_finite() && *r > 0.0).unwrap_or(RENDER_GUESS_SECS);
    let Some(step) = step_secs.filter(|s| s.is_finite() && *s > 0.0) else {
        return Plan { depth: ahead, at_once: CALM_AT_ONCE.min(most), lead: 1 };
    };
    let passed = (render / step).ceil().clamp(1.0, FAST_DEPTH as f64) as usize;
    let at_once = (passed + 1).max(CALM_AT_ONCE).min(most);
    let depth = ahead.max((passed + at_once - 1).min(FAST_DEPTH));
    Plan { depth, at_once, lead: passed.min(depth) }
}

/// Note the photo the loupe shows (in the frame at `now`, s) for the pace, before anything is asked for it. On a step to
/// it, the views still queued for the photo left are dropped (the loupe's render queued for it is replaced by this
/// one's, and its render running is told to stop when this one's is asked for).
pub(super) fn observe(app: &mut LightcraftApp, id: PhotoId, now: f64) {
    let (_, ids) = app.session.visible_shared();
    let Some(i) = ids.iter().position(|p| *p == id) else { return };
    if app.look_ahead.pace.observe(id, i, now) {
        super::drop_unsettled(&mut app.renderer, id);
    }
}

/// The photos to prefetch around place `i` of `ids`, as (place, photo), in the order to start them: from the plan's
/// lead to its depth in the direction of travel, then the nearer ones (they will be late, but less late than a render
/// started on arrival), then [`BEHIND`] the other way.
pub(crate) fn order(ids: &[PhotoId], i: usize, backward: bool, plan: Plan) -> Vec<(usize, PhotoId)> {
    let at = |k: usize, back: bool| {
        let j = if back { i.checked_sub(k) } else { i.checked_add(k) };
        j.and_then(|j| ids.get(j).map(|p| (j, *p)))
    };
    let ahead = |k: usize| at(k, backward);
    let in_time = (plan.lead..=plan.depth).filter_map(ahead);
    let near = (1..plan.lead).filter_map(ahead);
    let behind = (1..=BEHIND).filter_map(|k| at(k, !backward));
    in_time.chain(near).chain(behind).collect()
}

/// Prepare the photos the user is about to step to (source decoded and kept, view render cached): see the module.
pub(super) fn prefetch_neighbours(app: &mut LightcraftApp, id: PhotoId, main_area: Rect, ppp: f32, texture_side: usize, with_crop: bool, now: f64) {
    let (_, ids) = app.session.visible_shared();
    let Some(i) = ids.iter().position(|p| *p == id) else { return };
    let mode = prep_mode(app);
    let prep = app.session.prep_settings();
    let ahead = prep.ahead(mode);
    if ahead == 0 {
        app.renderer.drop_prefetches_except(&[]);
        return;
    }
    let pace = &app.look_ahead.pace;
    let plan = plan(ahead, pace.step_secs(now), app.renderer.prefetch_secs(), app.renderer.live_workers());
    let mut todo = order(&ids, i, pace.backward(), plan);
    if prep.scope(mode) == lightcraft_engine::prep::Scope::Marked {
        todo.retain(|(_, t)| lightcraft_engine::prep::marked(&app.session, *t));
    }
    let keep: Vec<PhotoId> = todo.iter().map(|(_, p)| *p).collect();
    app.renderer.drop_prefetches_except(&keep);
    // calm: once this photo is on screen (its render has the cores to itself); racing, the photos ahead can't wait
    let shown = !app.renderer.is_pending(Slot::Main) && app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == id);
    if !shown && !plan.racing() {
        return;
    }
    // editing: the next photos are decoded and kept, ready for their edits (their view render is not
    // what is wanted, and a cached one would leave the photo undecoded)
    let develop = mode == lightcraft_engine::prep::Mode::Develop;
    let mut room = plan.at_once.saturating_sub(app.renderer.prefetches_pending());
    for (at, nid) in todo {
        if room == 0 {
            break;
        }
        let n = slot(at);
        let Some(mut job) = neighbour_job(app, nid, main_area, ppp, texture_side, with_crop) else { continue };
        if develop {
            job.view_cache = None;
        }
        // (asked for already, in this slot: queued, running, made or covered; or the photo just left, still rendering
        // for the loupe, which keeps its view)
        if app.renderer.prefetch_asked(n, job.key) || app.renderer.main_rendering(nid) {
            continue;
        }
        // a view of this look kept at least this big is what the loupe will show: nothing to render (#223)
        if let Some((cache, key)) = &job.view_cache
            && cache.dims(*key).is_some_and(|have| crate::render::prefetch::covers(have, [job.request.max_w, job.request.max_h]))
        {
            app.renderer.prefetch_covered(n, &job);
            continue;
        }
        room -= 1;
        app.renderer.prefetch(n, job, PRIORITY);
    }
}

/// The render a neighbour is prepared with: its loupe render at the size it will be drawn at.
fn neighbour_job(
    app: &mut LightcraftApp,
    nid: PhotoId,
    main_area: Rect,
    ppp: f32,
    texture_side: usize,
    with_crop: bool,
) -> Option<lightcraft_engine::media::RenderJob> {
    let np = app.session.catalog.photo(nid)?;
    let nf = Frame::with_lens(np.width.max(1) as usize, np.height.max(1) as usize, &np.develop, with_crop, np.embedded_lens.as_ref());
    let na = nf.aspect() as f32;
    let nr = fit_rect(main_area, na, app.ui.zoom, output_px(&nf), ppp, app.ui.pan);
    let nw = app.ui.settings.prefetch_edge(nr.width().max(nr.height()) * ppp, output_px(&nf).into_iter().max().unwrap_or(1), texture_side);
    let (w, h) = if na >= 1.0 { (nw, (nw as f32 / na) as usize) } else { ((nw as f32 * na) as usize, nw) };
    app.session.loupe_job(nid, w.max(8), h.max(8), with_crop)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKERS: usize = 6;

    #[test]
    fn stepping_faster_prepares_further_ahead_and_more_at_once() {
        let render = Some(0.75);
        let calm = plan(5, None, render, WORKERS);
        assert_eq!(calm, Plan { depth: 5, at_once: 2, lead: 1 }, "not stepping: the settings' count, two at a time, nearest first");
        let slow = plan(5, Some(1.5), render, WORKERS);
        let brisk = plan(5, Some(0.5), render, WORKERS);
        let fast = plan(5, Some(0.2), render, WORKERS);
        let held = plan(5, Some(0.03), render, WORKERS);
        assert_eq!(slow, calm, "slower than a render: as calm");
        assert!(brisk.lead > 1 && brisk.at_once > calm.at_once, "{brisk:?}");
        assert!(fast.depth > brisk.depth && fast.lead > brisk.lead, "{fast:?} {brisk:?}");
        assert!(held.depth >= fast.depth && held.depth <= FAST_DEPTH && held.at_once <= MOST_AT_ONCE, "{held:?}");
        // (never less than the settings ask for, and never a lead past the depth)
        for p in [calm, slow, brisk, fast, held, plan(20, Some(0.03), render, WORKERS)] {
            assert!(p.depth >= 5 && p.lead >= 1 && p.lead <= p.depth, "{p:?}");
        }
        // a slower render is the same as faster stepping
        assert_eq!(plan(5, Some(1.0), Some(1.5), WORKERS), plan(5, Some(0.5), Some(0.75), WORKERS));
        // two workers (a 2–3 core Mac): one prefetch at a time, the other worker stays for the photo on screen
        assert_eq!(plan(5, Some(0.03), render, 2).at_once, 1);
        assert_eq!(plan(5, None, render, 2).at_once, 1);
        assert_eq!(plan(5, None, render, 3).at_once, 2);
        // one worker: it stays for the photo on screen, so nothing is prepared ahead (0: web, or a pool not started)
        assert_eq!(plan(5, None, render, 1).at_once, 0);
        assert_eq!(plan(5, Some(0.2), render, 1).at_once, 0);
        assert_eq!(plan(5, None, render, 0).at_once, 1);
    }

    #[test]
    fn photos_that_can_be_ready_on_arrival_go_first() {
        let ids: Vec<PhotoId> = (0..20).map(PhotoId).collect();
        let racing = Plan { depth: 5, at_once: 3, lead: 2 };
        let places = |v: Vec<(usize, PhotoId)>| v.into_iter().map(|(at, _)| at).collect::<Vec<_>>();
        assert_eq!(places(order(&ids, 10, false, racing)), vec![12, 13, 14, 15, 11, 9]);
        assert_eq!(places(order(&ids, 10, true, racing)), vec![8, 7, 6, 5, 9, 11], "stepping back: the other way");
        let calm = Plan { depth: 3, at_once: 2, lead: 1 };
        assert_eq!(places(order(&ids, 10, false, calm)), vec![11, 12, 13, 9], "calm: nearest first");
        assert_eq!(places(order(&ids, 18, false, calm)), vec![19, 17], "the end of the view");
        assert_eq!(places(order(&ids, 0, true, calm)), vec![1], "the start of the view");
    }

    #[test]
    fn the_pace_follows_a_run_of_steps() {
        let mut p = Pace::default();
        p.observe(PhotoId(1), 0, 10.0);
        assert_eq!(p.step_secs(10.1), None, "the loupe just opened");
        p.observe(PhotoId(2), 1, 10.5);
        assert_eq!(p.step_secs(10.6), Some(0.5));
        p.observe(PhotoId(3), 2, 11.0);
        p.observe(PhotoId(4), 3, 11.3);
        let s = p.step_secs(11.4).unwrap();
        assert!(s > 0.3 && s < 0.5, "smoothed: {s}");
        assert!(!p.backward());
        assert_eq!(p.step_secs(11.3 + PAUSE_SECS + 0.1), None, "a pause on this photo: not stepping");
        p.observe(PhotoId(5), 4, 20.0);
        assert_eq!(p.step_secs(20.1), Some(s), "a step after a pause goes on at the run's pace");
        p.observe(PhotoId(4), 3, 20.4);
        assert!(p.backward());
        p.observe(PhotoId(9), 30, 20.6);
        assert_eq!(p.step_secs(20.7), None, "a jump: no run");
    }
}
