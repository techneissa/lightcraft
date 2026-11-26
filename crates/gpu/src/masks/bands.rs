//! A render that keeps nothing (the full-size picture, export) evaluates its masks band by band of rows and finishes
//! each band from them (issue #406, step 1 of `fork/memory-design.md`): its alphas are one band's, not a plane of the
//! whole frame for every mask.
//!
//! A band's mask is evaluated over the band and the mask's reach around it ([`reach`]), with the whole frame's
//! coordinates, filter radii and box-filter runs (`masks::Cut`), so each pixel of the band gets the whole frame's alpha,
//! bit for bit. Gradients, luminance ranges and brush strokes read only their own pixel; a colour range smooths over
//! [`guided_reach`] of its sigma, and Refine Edges over that of its own. These masks are evaluated once over the whole
//! frame instead, and their rows copied into each band ([`whole_only`]): one with a part the CPU evaluates (Auto Mask
//! strokes, stored AI selections, Sky, Subject and the other shapes without a kernel: the CPU reads the whole frame for
//! them), a decontaminated one (its edge is estimated from its whole alpha), and one whose reach is wider than a band
//! (Refine Edges at a high setting on a large frame), which each band would evaluate over several bands' rows.

use std::sync::Arc;

use lightcraft_develop::{Mask, MaskShape};
use lightcraft_pipeline::Plan;
use lightcraft_pipeline::finish::MASK_TERMS;
use lightcraft_raster::{Plane, Rgb32f};

use super::{Cut, Images, Inner, Target, evaluate, on_cpu, refine_sigma, terms};
use crate::ctx::{Buf, FailKind, fail};
use crate::decontam;
use crate::render::{Cx, Host, box_chunk, profiling};

/// Planes of a mask's evaluation over its cut (the band and twice its reach), besides the band's buffer: the part, the
/// guide's rows, the cross guided filter's two-channel buffers and the Gaussians' ping-pong pairs. For sizing bands.
const SCRATCH_PLANES: usize = 17;

/// The masks of a render that keeps nothing, evaluated band by band.
pub(crate) struct Bands<'a> {
    list: Vec<&'a Mask>,
    /// Relight's planes (issue #288), whole on the host as the CPU made them (`None`: not of this frame's size).
    relit: Vec<Option<Plane>>,
    /// The masks of `list` evaluated once over the whole frame ([`whole_only`], or a reach wider than banded masks may
    /// have).
    whole: Vec<Option<Buf>>,
    /// Rows each other mask reads around a band ([`reach`]).
    reach: Vec<usize>,
    /// Rows of a band (a multiple of 16: the finish kernel's workgroups never cross a band).
    rows: usize,
    /// The masks' adjustment terms, in the order of their planes.
    pub terms: Vec<[f32; MASK_TERMS]>,
}

impl<'a> Bands<'a> {
    /// The bands of `plan`'s masks, with the masks evaluated whole done; `None` when it has no mask, or its alphas fit
    /// one band (the render evaluates them whole, as one that keeps its stages does).
    pub(crate) fn plan(cx: &mut Cx<'_>, img: &Images<'_>, plan: &'a Plan<'_>, host: &mut Host) -> Option<Bands<'a>> {
        let list: Vec<&Mask> = plan.settings.masks.iter().filter(|m| m.renders()).collect();
        let relit = lightcraft_pipeline::relight::masks(plan);
        if list.is_empty() && relit.is_empty() {
            return None;
        }
        let (w, h) = (plan.w, plan.h);
        let planes = list.len() + relit.len() + if img.prep.chroma.is_some() { 3 } else { 0 };
        let whole_only: Vec<bool> = list.iter().map(|m| whole_only(cx, plan, m)).collect();
        let reach: Vec<usize> = list.iter().map(|m| reach(plan, m)).collect();
        let banded: Vec<usize> = reach.iter().zip(&whole_only).filter(|(_, w)| !**w).map(|(r, _)| *r).collect();
        let limit = usize::try_from(cx.gpu.limit()).unwrap_or(usize::MAX);
        let (rows, widest) = band_rows(share(), limit, (w, h, planes), &banded);
        if rows >= h {
            return None;
        }
        let terms = terms(plan, &list, &relit);
        let relit = relit.iter().map(|e| Some(e.alpha.to_plane()).filter(|p| p.data.len() == w * h)).collect();
        // (the guide of stored selections and the Auto Mask planes serve only the masks evaluated here: let go after)
        let mut held = Inner::default();
        let mut whole = Vec::with_capacity(list.len());
        for ((m, r), only) in list.iter().zip(&reach).zip(&whole_only) {
            lightcraft_pipeline::masks::note_evaluation();
            if !only && *r <= widest {
                whole.push(None);
                continue;
            }
            #[cfg(test)]
            tests::WHOLE.with(|c| c.set(c.get() + 1));
            let (alpha, comp) = (cx.zeroed(w * h), cx.gpu.buffer(w * h));
            evaluate(cx, img, plan, host, (&mut held, false), m, Target { cut: Cut::whole(h), dst: &alpha, at: 0, comp: &comp });
            // (waited for: what it let go is freed before the next, see `finish`)
            cx.sync();
            whole.push(Some(alpha));
        }
        Some(Bands { list, relit, whole, reach, rows, terms })
    }

    /// Values a plane of a band's masks buffer holds: a whole band's rows, the last band's too.
    pub(crate) fn plane(&self, w: usize) -> usize {
        w * self.rows
    }

    /// The length of a band's masks buffer ([`Self::band`]), the same for every band.
    pub(crate) fn len(&self, w: usize, chroma: bool) -> usize {
        self.plane(w) * (self.list.len() + self.relit.len() + if chroma { 3 } else { 0 })
    }

    /// Mask `k`'s alpha over the whole frame (a buffer and where it starts), when it was evaluated whole.
    pub(crate) fn whole_alpha(&self, k: usize) -> Option<(&Buf, usize)> {
        self.whole.get(k)?.as_ref().map(|b| (b, 0))
    }

    /// Fill `buf`'s first [`Self::len`] values (zeroed) with the band from row `y0`: each mask's alpha on the band's
    /// rows, then the blurred chromaticity's (three values a pixel), at [`Self::plane`] values a plane.
    fn band(&self, cx: &mut Cx<'_>, img: &Images<'_>, plan: &Plan<'_>, host: &mut Host, (buf, y0): (&Buf, usize)) {
        let (w, h) = (plan.w, plan.h);
        let (rows, np) = (self.rows.min(h - y0), self.plane(w));
        let band = Cut { y0, rows };
        let stack = self.list.len() + self.relit.len();
        if let Some(ch) = &img.prep.chroma {
            cx.copy_range(ch, 3 * y0 * w, buf, np * stack, 3 * w * rows);
        }
        for (k, p) in self.relit.iter().enumerate() {
            if let Some(p) = p {
                let rows = cx.gpu.upload(band.of(&p.data, w));
                cx.copy_into(&rows, buf, (self.list.len() + k) * np);
            }
        }
        // (a banded mask has no part the CPU evaluates, and keeps no brush part: nothing is held between bands)
        let mut held = Inner::default();
        for (mi, m) in self.list.iter().enumerate() {
            if let Some(b) = &self.whole[mi] {
                cx.copy_range(b, y0 * w, buf, mi * np, w * rows);
                continue;
            }
            let r = self.reach[mi];
            let cut = Cut { y0: y0.saturating_sub(r), rows: (y0 + rows + r).min(h) - y0.saturating_sub(r) };
            let comp = cx.gpu.buffer(w * cut.rows);
            if cut == band && rows == self.rows {
                evaluate(cx, img, plan, host, (&mut held, false), m, Target { cut, dst: buf, at: mi, comp: &comp });
            } else {
                let alpha = cx.zeroed(w * cut.rows);
                evaluate(cx, img, plan, host, (&mut held, false), m, Target { cut, dst: &alpha, at: 0, comp: &comp });
                cx.copy_range(&alpha, (y0 - cut.y0) * w, buf, mi * np, w * rows);
            }
            // (what it used goes back to the pool for the next)
            cx.flush();
        }
    }

    /// Band by band: one masks buffer for every band, with `edges` (decontaminated edges) after it, filled with the band
    /// and handed to `run` with the finish kernel's parameters `p` set for it (its planes' size and first row) and the
    /// band's rectangle. `None` when that failed the render.
    fn finish(
        &self,
        cx: &mut Cx<'_>,
        (plan, img): (&Plan<'_>, &Images<'_>),
        host: &mut Host,
        edges: Option<&decontam::Found>,
        p: &mut [u32],
        run: &impl Fn(&mut Cx<'_>, &mut [u32], Option<&Buf>, &[usize; 4]),
    ) -> Option<()> {
        let (w, h) = (plan.w, plan.h);
        let (Some(n_at), Some(y0_at)) = (crate::params::index("MASK_N"), crate::params::index("MASK_Y0")) else {
            fail(FailKind::Limit, "the finish kernel's parameters have no MASK_N or MASK_Y0".into());
            return None;
        };
        *p.get_mut(n_at)? = self.plane(w) as u32;
        if profiling() {
            let whole = self.whole.iter().filter(|b| b.is_some()).count();
            eprintln!("  gpu mask bands: {} of {} rows, {whole} of {} masks whole", h.div_ceil(self.rows), self.rows, self.list.len());
        }
        let len = self.len(w, img.prep.chroma.is_some());
        let buf = match edges {
            Some(e) => e.after(cx, len)?,
            None => cx.gpu.buffer(len),
        };
        for y0 in (0..h).step_by(self.rows) {
            cx.clear_range(&buf, 0, len);
            self.band(cx, img, plan, host, (&buf, y0));
            note_band(cx, &buf, (y0, self.rows.min(h - y0)), (w, self.plane(w)));
            *p.get_mut(y0_at)? = y0 as u32;
            run(cx, p, Some(&buf), &[0, y0, w, y0 + self.rows.min(h - y0)]);
            // waited for: else every band is recorded before the device runs the first, and the buffers each band let go
            // are held until it has (a 33 MP export with 12 masks then peaked at 9.6 GB footprint, 3.2 GB with the wait)
            cx.sync();
        }
        Some(())
    }
}

/// Rows of a band, and the widest reach a mask evaluated per band may have, on a `w × h` frame whose band buffer holds
/// `planes` planes, for masks that may go per band with `reaches`: the band's buffer and one mask's evaluation
/// ([`SCRATCH_PLANES`] planes of the band and twice that reach) fit the band `share` (bytes), and the band's buffer and a
/// cut's two-channel buffers the device's largest buffer, `limit` bytes. The widest reach whose scratch leaves room for a
/// band of 16 rows or more, with a cut still shorter than the frame, is taken: a mask evaluated whole holds that scratch
/// for the whole frame. A mask that reaches farther is evaluated whole. Whole 16-row workgroups, at least 16 rows.
fn band_rows(share: usize, limit: usize, (w, h, planes): (usize, usize, usize), reaches: &[usize]) -> (usize, usize) {
    let row = 4 * w.max(1);
    let mut wide: Vec<usize> = reaches.iter().copied().filter(|r| *r > 0).collect();
    wide.sort_unstable_by(|a, b| b.cmp(a));
    wide.dedup();
    let rows_for = |reach: usize| {
        let room = (share / row).saturating_sub(SCRATCH_PLANES * 2 * reach) / (planes + SCRATCH_PLANES);
        let cap = (limit / (row * planes.max(1))).min((limit / (2 * row)).saturating_sub(2 * reach));
        room.min(cap) / 16 * 16
    };
    wide.into_iter()
        .map(|reach| (rows_for(reach), reach))
        .find(|(rows, reach)| *rows >= 16 && rows + 2 * reach < h)
        .unwrap_or_else(|| (rows_for(0).max(16), 0))
}

/// The type [`edges`] gives: the whole frame's masks buffer, the finish kernel's decontamination inputs, and the edges
/// each band's buffer gets.
type WithEdges = (Option<Arc<Buf>>, decontam::Edges, Option<decontam::Found>);

/// Step 5 of a render (`render::render`): `run` (the finish kernel's dispatches over a rectangle, with parameters `p` and
/// a masks buffer) over each band with its masks and decontaminated `edges` ([`Bands::finish`]), or over the rectangles
/// `rects` with the whole frame's `masks`. `None` when a band failed the render.
pub(crate) fn per_pixel(
    cx: &mut Cx<'_>,
    (bands, edges): (Option<&Bands<'_>>, Option<&decontam::Found>),
    (plan, img): (&Plan<'_>, &Images<'_>),
    host: &mut Host,
    (masks, rects): (Option<&Buf>, &[[usize; 4]]),
    p: &mut [u32],
    run: impl Fn(&mut Cx<'_>, &mut [u32], Option<&Buf>, &[usize; 4]),
) -> Option<()> {
    match bands {
        Some(b) => {
            // (a banded render starts from no earlier output: its rectangle is the whole frame)
            debug_assert_eq!(rects, &[[0, 0, plan.w, plan.h]]);
            b.finish(cx, (plan, img), host, edges, p, &run)
        }
        None => {
            for r in rects {
                run(cx, p, masks, r);
            }
            Some(())
        }
    }
}

/// Step 4 of a render (`render::render`): its masks band by band when it keeps nothing (no view's `store`), reads no
/// alpha back (no mask planes `want`ed, no overlay in `req`) and its alphas are large ([`Bands::plan`]; no alphas of
/// the whole frame then), else over the whole frame (`masks::masks`). With their adjustment terms.
pub(crate) fn stage<'a>(
    cx: &mut Cx<'_>,
    img: &Images<'_>,
    plan: &'a Plan<'_>,
    host: &mut Host,
    store: Option<(&super::AlphaStore, &Arc<Rgb32f>)>,
    (want, req): (&crate::render::Want<'_>, &lightcraft_pipeline::RenderRequest),
) -> (Option<Bands<'a>>, Option<Arc<Buf>>, Vec<[f32; MASK_TERMS]>) {
    let banded = store.is_none() && !matches!(want, crate::render::Want::MaskPlanes(_)) && req.overlay == lightcraft_pipeline::Overlay::None;
    match banded.then(|| Bands::plan(cx, img, plan, host)).flatten() {
        Some(b) => {
            let terms = b.terms.clone();
            (Some(b), None, terms)
        }
        None => {
            let (masks, terms) = super::masks(cx, img, plan, host, store);
            (None, masks, terms)
        }
    }
}

/// The decontaminated edges of a render's masks (fork, `fork/perf-decontam-gpu.md`): appended to the whole frame's
/// `masks` (`decontam::with_edges`, with the view's `kept` edges), or estimated once to go after each band's buffer
/// (`decontam::for_bands`). The masks buffer, the finish kernel's inputs and the bands' edges; `None` when that failed
/// the render.
pub(crate) fn edges(
    cx: &mut Cx<'_>,
    (plan, img): (&Plan<'_>, &Images<'_>),
    bands: Option<&Bands<'_>>,
    (masks, terms): (Option<Arc<Buf>>, &[[f32; MASK_TERMS]]),
    lin: &decontam::Lin<'_>,
    host: &mut Host,
    kept: Option<(&decontam::KeptEdges, &Arc<Rgb32f>)>,
) -> Option<WithEdges> {
    match bands {
        Some(b) => {
            let len = b.len(plan.w, img.prep.chroma.is_some());
            let (found, dc) = decontam::for_bands(cx, plan, |k| b.whole_alpha(k), terms, lin, host, len)?;
            Some((None, dc, found))
        }
        None => {
            let (masks, dc) = decontam::with_edges(cx, plan, masks, terms, lin, host, kept)?;
            Some((masks, dc, None))
        }
    }
}

/// Whether mask `m` is evaluated once over the whole frame in a banded render: it has a part the CPU evaluates over the
/// whole frame (`masks::on_cpu`), or it is decontaminated (its edge is estimated from its whole alpha).
fn whole_only(cx: &Cx<'_>, plan: &Plan<'_>, m: &Mask) -> bool {
    lightcraft_pipeline::decontam::applies(m) || on_cpu(cx, plan, m)
}

/// Rows next to a cut's edge inside a `w × h` frame where the cross guided filter (`masks::guided_cross`: two Gaussians
/// of `sigma`, one after the other) differs from the whole frame's. Each vertical box pass reaches its radius, and its
/// running sums start on the whole frame's rows ([`box_chunk`]): rows before the first of them in the cut may round
/// differently. Horizontal passes run along whole rows.
pub(crate) fn guided_reach(sigma: f32, (w, h): (usize, usize)) -> usize {
    lightcraft_raster::blur::radii_for(sigma, w, h).map_or(0, |radii| 2 * radii.iter().map(|&r| r + box_chunk(r)).sum::<usize>())
}

/// Rows around a band that mask `m` reads to give the band's pixels their whole-frame alpha: its colour ranges'
/// smoothing (`masks::evaluate`), then Refine Edges over the combined alpha.
fn reach(plan: &Plan<'_>, m: &Mask) -> usize {
    use lightcraft_pipeline::masks::color_range;
    let (w, h) = (plan.w, plan.h);
    let frame = plan.frame.for_mask(m.on_photo);
    let smooth = m
        .components
        .iter()
        .filter_map(|c| match &c.shape {
            MaskShape::ColorRange { samples, .. } if !color_range::keys(samples).is_empty() => {
                Some(guided_reach(color_range::smooth_sigma(&frame, w, h), (w, h)))
            }
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let refine = if m.refine > 0.0 { guided_reach(refine_sigma(&frame, (w, h), m.refine), (w, h)) } else { 0 };
    smooth + refine
}

/// Bytes a band may hold: the app's band share (`crate::set_band_budget`).
fn share() -> usize {
    #[cfg(test)]
    if let Some(s) = tests::SHARE_HERE.with(std::cell::Cell::get) {
        return s;
    }
    usize::try_from(crate::ctx::BAND_BUDGET.load(std::sync::atomic::Ordering::Relaxed)).unwrap_or(usize::MAX)
}

/// One more band filled on this thread, `buf` its masks buffer with rows `y0..y0 + rows` of a frame `w` wide in planes
/// of `np` values (tests: counted, and its alphas read back when asked).
fn note_band(cx: &mut Cx<'_>, buf: &Buf, (y0, rows): (usize, usize), (w, np): (usize, usize)) {
    #[cfg(test)]
    tests::note_band(cx, buf, (y0, rows), (w, np));
    #[cfg(not(test))]
    let _ = (cx, buf, y0, rows, w, np);
}

#[cfg(test)]
mod tests;
