//! Fork feature (`fork/object-remove.md`): Remove spots in the shape of an object or a mask.
//!
//! Every shape goes the same way: it is put in a mask (an Object selection from SAM 3's clicks, or a mask of the photo
//! as it is), the mask is evaluated as the render evaluates masks ([`lightcraft_pipeline::mask_planes`], on the
//! uncropped picture), and its coverage is stored with the spot as a grid in the spot's coordinates
//! ([`lightcraft_pipeline::spot_area::area_from_coverage`]). Rendering then needs neither the model nor the mask.
//!
//! In the app, clicks go to the model in the background ([`ObjectJobs`], applied by [`Session::segment_poll`]); a
//! click that adds to or takes from a selection still on its way replaces that request.

use std::sync::atomic::Ordering;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, SegMask, Spot, SpotArea, SpotFill, SpotMode};
use lightcraft_geom::Point;
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk};

use crate::Session;
use crate::activity::{Cancel, TaskGuard};
use crate::media::SourceLevel;
use crate::segment::Click;
use crate::segment::clicks_request::ClicksRequest;

/// How far a new shaped spot reaches past its outline, as a share of the long edge (the Expand slider's default):
/// soft edges, a strap or a shadow just outside what the model selected.
pub const DEFAULT_EXPAND: f64 = 0.005;
/// The Expand slider's range (a spot's `size` for a shaped spot).
pub const EXPAND_RANGE: (f64, f64) = (0.001, 0.05);
/// Feather of a new shaped spot.
pub const AREA_FEATHER: f64 = 50.0;
/// Most of the photo one shaped spot may cover (Remove rebuilds an area from its surroundings).
const MAX_SHARE: f64 = 0.6;
/// Long edge of the render a mask is measured on.
const MEASURE_EDGE: usize = 1280;
/// Most clicks a brushed stroke turns into.
const STROKE_CLICKS: usize = 6;
/// How far past a brushed stroke the snapped selection may reach, in brush radii.
const STROKE_REACH: f64 = 2.5;

/// Where a brushed stroke lets the selection reach: its points (spot coordinates) and its radius (long edge).
#[derive(Clone, Debug, PartialEq)]
pub struct Reach {
    pub points: Vec<Point>,
    pub radius: f64,
}

/// One Object selection on its way to the model (the app).
pub(crate) struct ObjectJob {
    photo: PhotoId,
    /// The spot it refines (`None`: it makes a new one).
    target: Option<usize>,
    hint: Vec<Point>,
    exclude: Vec<Point>,
    reach: Option<Reach>,
    expand: f64,
    fill: SpotFill,
    req: ClicksRequest,
    /// The model has answered: its selection is being measured on a thread of its own (so a big photo never holds
    /// up the window).
    measuring: Option<std::sync::mpsc::Receiver<Result<SegMask, String>>>,
    _task: TaskGuard,
}

/// The Object selections on their way to the model.
#[derive(Default)]
pub struct ObjectJobs(Vec<ObjectJob>);

impl ObjectJobs {
    /// Whether any is on its way.
    pub fn busy(&self) -> bool {
        !self.0.is_empty()
    }
}

/// What makes or changes a shaped Object spot.
pub(crate) struct ObjectAsk {
    pub target: Option<usize>,
    pub hint: Vec<Point>,
    pub exclude: Vec<Point>,
    pub reach: Option<Reach>,
    pub expand: f64,
    pub fill: SpotFill,
}

impl Session {
    /// Mask `mid` of `d` (photo `id`'s settings, maybe with a mask added) as a stored spot shape, cut to `reach` when
    /// given. An error says why there is nothing to remove.
    pub(crate) fn mask_spot_area(&mut self, id: PhotoId, d: &DevelopSettings, mid: u32, reach: Option<&Reach>) -> Result<SegMask, String> {
        let (src, info) = self.measure_source(id)?;
        measure(&src, &info, d, mid, reach)
    }

    /// The picture a shape is measured on: the photo's preview-size source (the view usually holds it already).
    fn measure_source(&mut self, id: PhotoId) -> Result<(std::sync::Arc<lightcraft_raster::Rgb32f>, lightcraft_pipeline::SourceInfo), String> {
        Ok((self.source_now(id, SourceLevel::Preview)?, self.source_info(id)))
    }
}

/// Mask `mid` of `d` measured on `src` (see [`Session::mask_spot_area`]); plain data, so it runs on any thread.
fn measure(
    src: &std::sync::Arc<lightcraft_raster::Rgb32f>,
    info: &lightcraft_pipeline::SourceInfo,
    d: &DevelopSettings,
    mid: u32,
    reach: Option<&Reach>,
) -> Result<SegMask, String> {
    // on the photo without its spots: the mask's edges follow the photo's, and a spot already over the object
    // (the one a further click changes) would hand it the filled-in pixels instead
    let d = &DevelopSettings { spots: Vec::new(), ..d.clone() };
    let req = lightcraft_pipeline::RenderRequest { apply_crop: false, ..lightcraft_pipeline::RenderRequest::fit(MEASURE_EDGE, MEASURE_EDGE) };
    let out = lightcraft_pipeline::mask_planes(src, info, d, &req, None, &PlanesAsk { want: MaskWant::One(mid), known: None });
    let plane = out.planes.and_then(|p| p.into_iter().find(|(i, _)| *i == mid).map(|(_, p)| p)).ok_or("the mask selects nothing to remove")?;
    let (w, h) = (plane.width, plane.height);
    if w == 0 || h == 0 || plane.data.len() != w * h {
        return Err("the mask selects nothing to remove".into());
    }
    let frame = lightcraft_pipeline::frame_for(src, info, d, false);
    let (to_norm, to_px) = (frame.out_to_norm(w, h), frame.norm_to_out(w, h));
    // (a mask at a lower Amount still selects its area)
    let peak = plane.data.iter().copied().filter(|v| v.is_finite()).fold(0.0f32, f32::max);
    if peak <= 0.05 {
        return Err("the mask selects nothing to remove".into());
    }
    let long = |p: Point| frame.norm_to_long(p);
    let reach_long: Option<(Vec<Point>, f64)> = reach.map(|r| (r.points.iter().map(|p| long(*p)).collect(), STROKE_REACH * r.radius));
    let cov: Vec<f32> = plane
        .data
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let v = if v.is_finite() { (v / peak).clamp(0.0, 1.0) } else { 0.0 };
            match &reach_long {
                Some((pts, r)) if v > 0.0 => {
                    let q = long(to_norm.apply(Point::new((i % w) as f64 + 0.5, (i / w) as f64 + 0.5)));
                    if pts.iter().any(|p| (p.x - q.x).hypot(p.y - q.y) <= *r) { v } else { 0.0 }
                }
                _ => v,
            }
        })
        .collect();
    let inside = cov.iter().filter(|c| **c > 0.5).count();
    if inside == 0 {
        return Err("the mask selects nothing to remove".into());
    }
    if inside as f64 / (w * h) as f64 > MAX_SHARE {
        return Err("the area covers most of the photo; Remove rebuilds smaller areas from what is around them".into());
    }
    lightcraft_pipeline::spot_area::area_from_coverage(&cov, w, h, |p| to_norm.apply(p), |p| to_px.apply(p))
        .ok_or_else(|| "the mask selects nothing to remove".to_string())
}

/// The settings `d` with an Object mask of `seg` added (the model's photo coordinates), and the mask's id.
fn with_object(d: &DevelopSettings, seg: SegMask, ask: &ObjectAsk) -> (DevelopSettings, u32) {
    let mut d = d.clone();
    let mid = d.masks.iter().map(|m| m.id).max().unwrap_or(0).saturating_add(1);
    let shape = MaskShape::Object { hint: ask.hint.clone(), exclude: ask.exclude.clone(), seg: Some(seg), detail: Vec::new(), edge: 0.0 };
    d.masks.push(Mask { id: mid, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Default::default() });
    (d, mid)
}

/// The stored shape of an Object spot from its measured coverage.
fn object_area(seg: SegMask, ask: &ObjectAsk) -> SpotArea {
    SpotArea { hint: ask.hint.clone(), exclude: ask.exclude.clone(), ..SpotArea::new("object", seg) }
}

impl Session {
    /// An Object selection (`seg`, in the model's photo coordinates, from `hint` and `exclude`) as a stored spot shape.
    pub(crate) fn object_spot_area(&mut self, id: PhotoId, seg: SegMask, ask: &ObjectAsk) -> Result<SpotArea, String> {
        let (d, mid) = with_object(&*self.develop_of(id).ok_or("no such photo")?, seg, ask);
        Ok(object_area(self.mask_spot_area(id, &d, mid, ask.reach.as_ref())?, ask))
    }

    /// Add a shaped Remove spot of `area` to photo `id` (one undo step, `label`) and select it; its index.
    pub(crate) fn add_area_spot(&mut self, id: PhotoId, area: SpotArea, expand: f64, fill: SpotFill, label: &str) -> crate::Result<usize> {
        let mut d = (*self.develop_of(id).unwrap_or_default()).clone();
        d.spots.push(area_spot(area, expand, fill));
        let n = d.spots.len();
        self.set_develop(id, d, label)?;
        self.active_spot = Some(n - 1);
        Ok(n - 1)
    }

    /// Give spot `i` of photo `id` the shape `area` (one undo step).
    pub(crate) fn reshape_spot(&mut self, id: PhotoId, i: usize, area: SpotArea) -> crate::Result<()> {
        let mut d = (*self.develop_of(id).unwrap_or_default()).clone();
        let sp = d.spots.get_mut(i).ok_or_else(|| crate::EngineError::Other("the spot is gone".into()))?;
        sp.points = vec![centre_of(&area).unwrap_or(sp.points.first().copied().unwrap_or(Point::new(0.5, 0.5)))];
        sp.area = Some(area);
        self.set_develop(id, d, "Edit Object")?;
        self.active_spot = Some(i);
        Ok(())
    }

    /// Apply an Object selection: a new spot, or spot `ask.target` reshaped. Its index.
    pub(crate) fn apply_object(&mut self, id: PhotoId, seg: SegMask, ask: &ObjectAsk) -> crate::Result<usize> {
        let area = self.object_spot_area(id, seg, ask).map_err(crate::EngineError::Other)?;
        self.apply_object_area(id, area, ask)
    }

    /// A measured Object shape applied: a new spot, or spot `ask.target` reshaped. Its index.
    fn apply_object_area(&mut self, id: PhotoId, area: SpotArea, ask: &ObjectAsk) -> crate::Result<usize> {
        match ask.target {
            Some(i) => self.reshape_spot(id, i, area).map(|_| i),
            None => self.add_area_spot(id, area, ask.expand, ask.fill, "Remove Object"),
        }
    }

    /// The clicks of the Object selection still on its way to the model for spot `target` (`None`: a new spot) of
    /// photo `id`, if any (a further click builds on them).
    pub(crate) fn object_pending(&self, id: PhotoId, target: Option<usize>) -> Option<(Vec<Point>, Vec<Point>)> {
        self.object_jobs.0.iter().rev().find(|j| j.photo == id && j.target == target).map(|j| (j.hint.clone(), j.exclude.clone()))
    }

    /// The latest new-spot selection still on its way for photo `id` (a click that adds to "the selected spot" before
    /// it exists builds on it).
    pub(crate) fn object_pending_new(&self, id: PhotoId) -> bool {
        self.object_jobs.0.iter().any(|j| j.photo == id && j.target.is_none())
    }

    /// Queue an Object selection (the app); it replaces one still on its way for the same spot when `replace`.
    pub(crate) fn queue_object(&mut self, id: PhotoId, ask: ObjectAsk, model_clicks: &[Click], replace: bool) -> crate::Result<()> {
        self.segmenter.require_model()?;
        if replace {
            let (keep, gone): (Vec<ObjectJob>, Vec<ObjectJob>) =
                std::mem::take(&mut self.object_jobs.0).into_iter().partition(|j| !(j.photo == id && j.target == ask.target));
            for j in gone {
                j.req.cancel.store(true, Ordering::SeqCst);
            }
            self.object_jobs.0 = keep;
        }
        let req = self.segment_clicks_request(id, model_clicks).map_err(crate::EngineError::Other)?;
        let task = self.activity.start("segment", "Selecting with SAM 3", Cancel::Flag(req.cancel.clone()));
        let ObjectAsk { target, hint, exclude, reach, expand, fill } = ask;
        self.object_jobs.0.push(ObjectJob { photo: id, target, hint, exclude, reach, expand, fill, req, measuring: None, _task: task });
        Ok(())
    }

    /// Apply the Object selections the model has answered (called by [`Session::segment_poll`]): true when a spot
    /// changed; errors go to `messages`.
    pub(crate) fn poll_object_jobs(&mut self, messages: &mut Vec<String>) -> bool {
        let active = self.active();
        let mut changed = false;
        let jobs = std::mem::take(&mut self.object_jobs.0);
        let mut waiting = Vec::new();
        for mut j in jobs {
            if active != Some(j.photo) || j.req.cancel.load(Ordering::SeqCst) {
                // another photo now, or stopped with ✕
                j.req.cancel.store(true, Ordering::SeqCst);
                continue;
            }
            let ask = ObjectAsk {
                target: j.target,
                hint: j.hint.clone(),
                exclude: j.exclude.clone(),
                reach: j.reach.clone(),
                expand: j.expand,
                fill: j.fill,
            };
            // (a spot it refines must still be an Object spot)
            let photo = j.photo;
            let fits = |s: &Session| ask.target.is_none_or(|i| s.develop_of(photo).is_some_and(|d| d.spots.get(i).is_some_and(is_object_spot)));
            const CHANGED: &str = "The spot was changed before the object was found; click again.";
            if let Some(rx) = &j.measuring {
                let measured = match rx.try_recv() {
                    Ok(m) => m,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        waiting.push(j);
                        continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("measuring the selection failed; click again".into()),
                };
                match measured {
                    Ok(_) if !fits(self) => messages.push(CHANGED.into()),
                    Ok(seg) => match self.apply_object_area(photo, object_area(seg, &ask), &ask) {
                        Ok(_) => changed = true,
                        Err(e) => messages.push(e.to_string()),
                    },
                    Err(e) => messages.push(e),
                }
                continue;
            }
            let Some(result) = j.req.try_take() else {
                waiting.push(j);
                continue;
            };
            match result {
                Ok(Some(_)) if !fits(self) => messages.push(CHANGED.into()),
                Ok(Some(seg)) => match self.measure_later(photo, seg, &ask) {
                    Ok(rx) => {
                        j.measuring = Some(rx);
                        waiting.push(j);
                    }
                    Err(e) => messages.push(e),
                },
                Ok(None) => messages.push("Nothing was selected there; click on the object.".into()),
                Err(e) => messages.push(e),
            }
        }
        // (jobs queued while these were applied stay too)
        waiting.append(&mut self.object_jobs.0);
        self.object_jobs.0 = waiting;
        changed
    }

    /// Put a job whose request is `req` on the list, as [`Session::queue_object`] does (tests).
    #[cfg(all(test, feature = "sam"))]
    pub(crate) fn push_object_job(&mut self, id: PhotoId, ask: ObjectAsk, req: ClicksRequest) {
        let task = self.activity.start("segment", "Selecting with SAM 3", Cancel::Flag(req.cancel.clone()));
        let ObjectAsk { target, hint, exclude, reach, expand, fill } = ask;
        self.object_jobs.0.push(ObjectJob { photo: id, target, hint, exclude, reach, expand, fill, req, measuring: None, _task: task });
    }

    /// Measure the Object selection `seg` of photo `id` on a thread of its own; the answer comes on the channel.
    fn measure_later(&mut self, id: PhotoId, seg: SegMask, ask: &ObjectAsk) -> Result<std::sync::mpsc::Receiver<Result<SegMask, String>>, String> {
        let (d, mid) = with_object(&*self.develop_of(id).ok_or("no such photo")?, seg, ask);
        let (src, info) = self.measure_source(id)?;
        let reach = ask.reach.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let work = move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| measure(&src, &info, &d, mid, reach.as_ref())))
                .unwrap_or_else(|_| Err("measuring the selection failed unexpectedly (the photo is untouched)".into()));
            let _ = tx.send(r);
        };
        std::thread::Builder::new().name("object-measure".into()).spawn(work).map_err(|e| format!("could not measure the selection: {e}"))?;
        Ok(rx)
    }
}

/// A shaped Remove spot of `area`, reaching `expand` (long edge) past it.
pub fn area_spot(area: SpotArea, expand: f64, fill: SpotFill) -> Spot {
    let expand = if expand.is_finite() { expand.clamp(EXPAND_RANGE.0, EXPAND_RANGE.1) } else { DEFAULT_EXPAND };
    Spot {
        mode: SpotMode::Remove,
        points: vec![centre_of(&area).unwrap_or(Point::new(0.5, 0.5))],
        size: expand,
        feather: AREA_FEATHER,
        opacity: 100.0,
        source_offset: None,
        fill,
        area: Some(area),
        ..Default::default()
    }
}

/// The middle of a shape's rectangle (the spot's pin).
pub(crate) fn centre_of(area: &SpotArea) -> Option<Point> {
    let r = area.bounds()?;
    Some(Point::new((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0))
}

/// Whether `spot` is a shaped spot made by Object clicks (more clicks refine it).
pub fn is_object_spot(spot: &Spot) -> bool {
    spot.area.as_ref().is_some_and(|a| a.kind == "object")
}

/// Up to [`STROKE_CLICKS`] points spread along `points` (the first and the last included).
pub fn stroke_clicks(points: &[Point]) -> Vec<Point> {
    let n = points.len();
    if n <= STROKE_CLICKS {
        return points.to_vec();
    }
    (0..STROKE_CLICKS).filter_map(|k| points.get(k * (n - 1) / (STROKE_CLICKS - 1)).copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_clicks_spread_along_the_stroke() {
        let pts: Vec<Point> = (0..100).map(|i| Point::new(i as f64 / 100.0, 0.5)).collect();
        let c = stroke_clicks(&pts);
        assert_eq!(c.len(), STROKE_CLICKS);
        assert_eq!(c.first(), pts.first());
        assert_eq!(c.last(), pts.last());
        assert_eq!(stroke_clicks(&pts[..3]).len(), 3);
        assert!(stroke_clicks(&[]).is_empty());
    }

    #[test]
    fn an_area_spot_has_no_source_and_a_sane_expand() {
        let seg = SegMask::from_logits_in(4, &[1.0; 16], [0.2, 0.2, 0.4, 0.6]);
        for (e, want) in [(0.0, EXPAND_RANGE.0), (1.0, EXPAND_RANGE.1), (f64::NAN, DEFAULT_EXPAND), (0.01, 0.01)] {
            let s = area_spot(SpotArea::new("mask", seg.clone()), e, SpotFill::Lama);
            assert_eq!(s.size, want);
            assert!(s.source_offset.is_none());
            let p = s.points[0];
            assert!((p.x - 0.3).abs() < 1e-12 && (p.y - 0.4).abs() < 1e-12, "{p:?}");
        }
    }
}
