//! Fork feature (fork/birefnet-masks.md, issue #48): BiRefNet jobs on the SAM 3 worker thread — the model's loading,
//! its cut-out of the photo, and the mix with SAM 3's selection ("SAM 3 + BiRefNet Edges").

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_birefnet::{BiRefNet, Matte, combine};
use lightcraft_develop::SegMask;
use lightcraft_segment::remote::{CONCEPT_THRESHOLD, text_logits};

use super::{Failure, Flag, Job, Placement, Shared, State, encoded, render};
use crate::segment::birefnet::SelectionModel;

impl From<lightcraft_birefnet::Error> for Failure {
    fn from(e: lightcraft_birefnet::Error) -> Self {
        match e {
            lightcraft_birefnet::Error::Cancelled => Failure::Cancelled,
            // (a file that is not the model, or damaged, is not the GPU's fault)
            lightcraft_birefnet::Error::Model(_) | lightcraft_birefnet::Error::Missing(_) => {
                Failure::Other(format!("{e}. If the file is damaged, delete model.safetensors from the BiRefNet folder and download it again."))
            }
            lightcraft_birefnet::Error::Candle(_) => Failure::Model(e.to_string()),
            // (Core ML only, as Settings asked: candle would not help)
            lightcraft_birefnet::Error::CoreMl(_) => Failure::Other(e.to_string()),
        }
    }
}

/// The BiRefNet model for `dir`, loading it on `placement` when needed: on the GPU through `runtime` (Core ML when its
/// package is there and loads, issue #264), on the CPU through candle.
fn model<'a>(
    state: &'a mut State,
    shared: &Shared,
    dir: &Path,
    runtime: lightcraft_coreml::Runtime,
    placement: Placement,
) -> Result<&'a mut BiRefNet, Failure> {
    if state.birefnet.as_ref().is_none_or(|(d, r, _)| d != dir || *r != runtime) {
        state.birefnet = None;
        let _busy = Flag::raise(&shared.encoding);
        let t = web_time::Instant::now();
        let variant = lightcraft_birefnet::Variant::General;
        let m = match placement {
            Placement::Gpu => {
                // (the units measured fastest on this Mac, issue #267)
                let plan = crate::model_units::probes::plan(crate::model_units::probes::birefnet(dir, false), runtime);
                BiRefNet::load_in_order(dir, variant, plan.runtime(runtime), plan.coreml, lightcraft_birefnet::best_device())?
            }
            Placement::Cpu => BiRefNet::load_cpu(dir, variant)?,
        };
        log::info!("BiRefNet loaded in {:?} ({}, {})", t.elapsed(), m.backend(), if m.device().is_cpu() { "CPU" } else { "GPU" });
        state.birefnet = Some((dir.to_path_buf(), runtime, m));
        shared.loaded.store(true, Ordering::SeqCst);
    }
    state.birefnet.as_mut().map(|(_, _, m)| m).ok_or_else(|| "the BiRefNet model did not load".into())
}

/// BiRefNet's cut-out of the job's photo.
fn matte(state: &mut State, shared: &Shared, job: &mut Job, req: &Request, placement: Placement) -> Result<Matte, Failure> {
    // (a model measurement waits while this runs: issue #267)
    let _job = crate::model_units::shared().user_job();
    // (SAM 3's encode renders the photo too when it must; keep the render job for it)
    let saved = job.render.clone();
    let input = render(job)?;
    job.render = saved;
    if job.cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    let m = model(state, shared, req.dir, req.runtime, placement)?;
    m.set_cancel(Some(job.cancel.clone()));
    let _busy = Flag::raise(&shared.encoding);
    let t = web_time::Instant::now();
    let matte = m.matte(&input.rgb, input.w, input.h);
    // (Automatic hands a failed Core ML run to candle: the engine is read after the run)
    *shared.birefnet_backend.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(m.backend());
    let matte = matte?;
    log::info!("BiRefNet cut out {}×{} in {:?} ({})", input.w, input.h, t.elapsed(), m.backend());
    Ok(matte)
}

/// What a BiRefNet job asks for.
pub(super) struct Request<'a> {
    /// Which model makes the selection.
    pub model: SelectionModel,
    pub text: &'a str,
    pub main: bool,
    pub dir: &'a Path,
    /// BiRefNet-matting redraws the selection's edge (`fork/hair-edges.md`).
    pub matting: Option<&'a crate::hair_edges::HairEdges>,
    /// Where the edge refinement shows (fork, issue #240).
    pub activity: Option<&'a crate::activity::Activity>,
    /// The engine BiRefNet runs on (issue #264).
    pub runtime: lightcraft_coreml::Runtime,
}

/// The two steps of "SAM 3 + BiRefNet Edges", so the order they run in can be tested without a model.
trait Steps {
    /// SAM 3's logits for the concept, `None` when it finds nothing. May take `job.render` (the encode does when
    /// the photo is not cached).
    fn sam(&mut self, job: &mut Job) -> Result<Option<Vec<f32>>, Failure>;
    /// BiRefNet's cut-out; renders the photo from `job.render`.
    fn matte(&mut self, job: &mut Job) -> Result<Matte, Failure>;
}

/// The real steps, on the worker's models.
struct Models<'a> {
    state: &'a mut State,
    shared: &'a Shared,
    placement: Placement,
    req: &'a Request<'a>,
}

impl Steps for Models<'_> {
    fn sam(&mut self, job: &mut Job) -> Result<Option<Vec<f32>>, Failure> {
        let (m, enc) = encoded(self.state, self.shared, job, self.placement)?;
        Ok(text_logits(m, enc, self.req.text, CONCEPT_THRESHOLD, self.req.main)?)
    }

    fn matte(&mut self, job: &mut Job) -> Result<Matte, Failure> {
        matte(self.state, self.shared, job, self.req, self.placement)
    }
}

/// Run a BiRefNet job: SAM 3 alone (asked for `text`, the main subject's instances only when `main`), BiRefNet's
/// cut-out alone, or SAM 3 deciding what the subject is and BiRefNet drawing its edge; then, with `matting`,
/// BiRefNet-matting redraws the edge; then the edge is refined on the photo at a finer size (`hair_edges::refine`,
/// its finer grids left in `job.detail`). `Ok(None)`: nothing found.
pub(super) fn run(state: &mut State, shared: &Shared, job: &mut Job, placement: Placement, req: &Request) -> Result<Option<SegMask>, Failure> {
    let Some(decided) = decide(state, shared, job, placement, req)? else { return Ok(None) };
    Ok(Some(refined(job, req, decided)?))
}

/// `decided` with its edge refined on the job's finer render, the finer grids put in `job.detail`; as it is when the
/// job has no finer render, the refinement can't run (a damaged grid) or its row in the top bar was stopped.
fn refined(job: &mut Job, req: &Request, decided: SegMask) -> Result<SegMask, Failure> {
    // (the row's ✕ stops only the refinement: the selection the models made stays; `job.cancel` (the photo switched,
    // the mask deleted) drops it)
    let row_stop = Arc::new(AtomicBool::new(false));
    let _row = req.activity.map(|a| a.start_background("refine", "Refining mask edges", crate::activity::Cancel::Flag(row_stop.clone())));
    refined_unless(job, decided, &row_stop)
}

/// [`refined`] with the refinement's own stop flag `row_stop`.
fn refined_unless(job: &mut Job, decided: SegMask, row_stop: &AtomicBool) -> Result<SegMask, Failure> {
    let Some(finer) = job.refine.clone() else { return Ok(decided) };
    if job.cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    if row_stop.load(Ordering::Relaxed) {
        return Ok(decided);
    }
    // (the finer render goes through the job's own render step, which says when the photo's file is missing)
    let saved = job.render.replace(finer);
    let input = render(job);
    job.render = saved;
    let input = input?;
    let t = web_time::Instant::now();
    let cancel = job.cancel.clone();
    let stop = || cancel.load(Ordering::Relaxed) || row_stop.load(Ordering::Relaxed);
    match crate::hair_edges::refine::subject(&decided, &input.rgb, input.w, input.h, &stop) {
        Some((main, detail)) => {
            log::info!("mask edges refined at {}×{} in {:?} ({} finer grids)", input.w, input.h, t.elapsed(), detail.len());
            job.detail = detail;
            Ok(main)
        }
        None if job.cancel.load(Ordering::Relaxed) => Err(Failure::Cancelled),
        None => Ok(decided),
    }
}

/// The selection a BiRefNet job's models decide on, before its edge is refined.
fn decide(state: &mut State, shared: &Shared, job: &mut Job, placement: Placement, req: &Request) -> Result<Option<SegMask>, Failure> {
    // (the matte renders the photo from the job's render, which SAM 3's encode may take)
    let saved = job.render.clone();
    let base = match req.model {
        SelectionModel::BiRefNet => stand_alone(&matte(state, shared, job, req, placement)?),
        SelectionModel::Combined => combined(&mut Models { state, shared, placement, req }, job)?,
        SelectionModel::Sam3 => {
            let (m, enc) = encoded(state, shared, job, placement)?;
            text_logits(m, enc, req.text, CONCEPT_THRESHOLD, req.main)?.map(|l| SegMask::from_logits(lightcraft_segment::MASK_SIDE, &l))
        }
    };
    // (the edge read now, not when the job was queued: set back to Current meanwhile, the part keeps its own edge)
    let kind = crate::hair_edges::EdgeKind::Subject;
    let edge = req.matting.map_or(crate::hair_edges::Edge::Current, |e| e.edge(kind));
    let matting = req.matting.filter(|_| edge != crate::hair_edges::Edge::Current);
    let (Some(edges), Some(base)) = (matting, base.as_ref()) else { return Ok(base) };
    job.render = saved;
    // (BiRefNet Matting mattes the picture a people parse uses: a matte one of them made serves the other)
    let input = match job.plain.take().filter(|_| edge == crate::hair_edges::Edge::Matting) {
        Some(plain) => {
            let saved = job.render.replace(plain);
            let input = render(job);
            job.render = saved;
            input?
        }
        None => render(job)?,
    };
    let m = edges.subject_matte(edge, &input.rgb, input.w, input.h, &job.cancel).map_err(|e| match job.cancel.load(Ordering::Relaxed) {
        true => Failure::Cancelled,
        false => Failure::Other(format!("Hair edges: {e}")),
    })?;
    // (Vision found no subject: the part keeps its own edge)
    let Some(m) = m else { return Ok(Some(base.clone())) };
    Ok(crate::hair_edges::subject_edge(base, &m, edge))
}

/// BiRefNet's cut-out as a selection; `None` when it holds nothing.
fn stand_alone(m: &Matte) -> Option<SegMask> {
    m.logits.iter().any(|l| *l > 0.0).then(|| SegMask::from_logits(m.side, &m.logits).made_by(SelectionModel::BiRefNet.id()))
}

/// SAM 3 first, then BiRefNet, then their mix.
fn combined(steps: &mut impl Steps, job: &mut Job) -> Result<Option<SegMask>, Failure> {
    let side = lightcraft_segment::MASK_SIDE;
    // (SAM 3's encode takes the render job when the photo is not cached; BiRefNet needs it too)
    let render = job.render.clone();
    let found = steps.sam(job)?;
    job.render = render;
    if job.cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    let m = steps.matte(job)?;
    // SAM 3's words found nothing (ferns, branches, a bottle): BiRefNet's cut-out stands alone, and says so
    let Some(sam) = found else { return Ok(stand_alone(&m)) };
    let logits = combine::refine_edges(&sam, side, &m);
    if !logits.iter().any(|l| *l > 0.0) {
        return Ok(None);
    }
    Ok(Some(SegMask::from_logits(m.side, &logits).made_by(SelectionModel::Combined.id())))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::Session;

    /// SAM 3 that takes the render job as its encode does with nothing cached, and a BiRefNet that needs it.
    struct Stub {
        matte_saw_render: bool,
    }

    impl Steps for Stub {
        fn sam(&mut self, job: &mut Job) -> Result<Option<Vec<f32>>, Failure> {
            assert!(job.render.take().is_some(), "the job carries its render");
            let side = lightcraft_segment::MASK_SIDE;
            Ok(Some((0..side * side).map(|i| if (i % side) < side / 2 { 6.0 } else { -6.0 }).collect()))
        }

        fn matte(&mut self, job: &mut Job) -> Result<Matte, Failure> {
            self.matte_saw_render = job.render.is_some();
            if !self.matte_saw_render {
                return Err(Failure::Other("the photo is not available".into()));
            }
            let side = 64;
            Ok(Matte { side, logits: (0..side * side).map(|i| if (i % side) < side / 2 { 9.0 } else { -9.0 }).collect() })
        }
    }

    /// The first run of "SAM 3 + BiRefNet Edges" on a photo (nothing cached) must not lose the render job to SAM 3's
    /// encode: it failed with "the photo is not available" every time until SAM 3 had run alone on the photo.
    #[test]
    fn the_combined_model_works_on_a_photo_nothing_is_cached_for() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        let render = s.render_job(id, 64, 64, false, true).unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut job = Job {
            id: 1,
            cancel: Arc::new(AtomicBool::new(false)),
            dir: std::env::temp_dir(),
            key: 1,
            render: Some(render),
            missing: None,
            kind: super::super::Kind::Prepare,
            tag: crate::segment::Tag::Wait,
            reply: tx,
            remote: None,
            refine: None,
            plain: None,
            detail: Vec::new(),
            runtime: lightcraft_coreml::Runtime::Automatic,
            activity: None,
        };
        let mut stub = Stub { matte_saw_render: false };
        let seg = combined(&mut stub, &mut job).unwrap().expect("a selection");
        assert!(stub.matte_saw_render);
        assert_eq!(seg.model.as_deref(), Some("sam3+birefnet"));
        assert_eq!(seg.side, 64);
    }

    /// Stopping the "Refining mask edges" row keeps the selection the models made, unrefined; stopping the job (the
    /// photo switched, the mask deleted) still drops it.
    #[test]
    fn stopping_the_refine_row_keeps_the_unrefined_selection() {
        let mut s = Session::with_demo();
        let id = s.active().unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut job = Job {
            id: 1,
            cancel: Arc::new(AtomicBool::new(false)),
            dir: std::env::temp_dir(),
            key: 1,
            render: None,
            missing: None,
            kind: super::super::Kind::Prepare,
            tag: crate::segment::Tag::Wait,
            reply: tx,
            remote: None,
            refine: Some(s.render_job(id, 96, 64, false, true).unwrap()),
            plain: None,
            detail: Vec::new(),
            runtime: lightcraft_coreml::Runtime::Automatic,
            activity: None,
        };
        let side = 16;
        let decided = SegMask::from_logits(side, &(0..side * side).map(|i| if (i % side) < side / 2 { 8.0 } else { -8.0 }).collect::<Vec<_>>())
            .made_by("birefnet");
        // (left alone, it is refined: the test below is not vacuous)
        let free = AtomicBool::new(false);
        let refined = refined_unless(&mut job, decided.clone(), &free).unwrap();
        assert_eq!(refined.model.as_deref(), Some("birefnet+refined"));
        // the row's ✕
        job.detail.clear();
        let row = AtomicBool::new(true);
        let kept = refined_unless(&mut job, decided.clone(), &row).unwrap();
        assert_eq!(kept, decided);
        assert!(job.detail.is_empty());
        // the job stopped
        job.cancel.store(true, Ordering::Relaxed);
        assert!(matches!(refined_unless(&mut job, decided, &free), Err(Failure::Cancelled)));
    }
}
