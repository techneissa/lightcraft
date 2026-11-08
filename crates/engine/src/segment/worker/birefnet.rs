//! Fork feature (fork/birefnet-masks.md, issue #48): BiRefNet jobs on the SAM 3 worker thread — the model's loading,
//! its cut-out of the photo, and the mix with SAM 3's selection ("SAM 3 + BiRefNet Edges").

use std::path::Path;
use std::sync::atomic::Ordering;

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
        }
    }
}

/// The BiRefNet model for `dir`, loading it on `placement` when needed.
fn model<'a>(state: &'a mut State, shared: &Shared, dir: &Path, placement: Placement) -> Result<&'a mut BiRefNet, Failure> {
    if state.birefnet.as_ref().is_none_or(|(d, _)| d != dir) {
        state.birefnet = None;
        let _busy = Flag::raise(&shared.encoding);
        let t = web_time::Instant::now();
        let m = match placement {
            Placement::Gpu => BiRefNet::load(dir, lightcraft_birefnet::Variant::General)?,
            Placement::Cpu => BiRefNet::load_cpu(dir, lightcraft_birefnet::Variant::General)?,
        };
        log::info!("BiRefNet loaded in {:?} ({})", t.elapsed(), if m.device().is_cpu() { "CPU" } else { "GPU" });
        state.birefnet = Some((dir.to_path_buf(), m));
        shared.loaded.store(true, Ordering::SeqCst);
    }
    state.birefnet.as_mut().map(|(_, m)| m).ok_or_else(|| "the BiRefNet model did not load".into())
}

/// BiRefNet's cut-out of the job's photo.
fn matte(state: &mut State, shared: &Shared, job: &mut Job, dir: &Path, placement: Placement) -> Result<Matte, Failure> {
    // (SAM 3's encode renders the photo too when it must; keep the render job for it)
    let saved = job.render.clone();
    let input = render(job)?;
    job.render = saved;
    if job.cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    let m = model(state, shared, dir, placement)?;
    m.set_cancel(Some(job.cancel.clone()));
    let _busy = Flag::raise(&shared.encoding);
    let t = web_time::Instant::now();
    let matte = m.matte(&input.rgb, input.w, input.h)?;
    log::info!("BiRefNet cut out {}×{} in {:?}", input.w, input.h, t.elapsed());
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
        matte(self.state, self.shared, job, self.req.dir, self.placement)
    }
}

/// Run a BiRefNet job: SAM 3 alone (asked for `text`, the main subject's instances only when `main`), BiRefNet's
/// cut-out alone, or SAM 3 deciding what the subject is and BiRefNet drawing its edge; then, with `matting`,
/// BiRefNet-matting redraws the edge. `Ok(None)`: nothing found.
pub(super) fn run(state: &mut State, shared: &Shared, job: &mut Job, placement: Placement, req: &Request) -> Result<Option<SegMask>, Failure> {
    // (the matte renders the photo from the job's render, which SAM 3's encode may take)
    let saved = job.render.clone();
    let base = match req.model {
        SelectionModel::BiRefNet => stand_alone(&matte(state, shared, job, req.dir, placement)?),
        SelectionModel::Combined => combined(&mut Models { state, shared, placement, req }, job)?,
        SelectionModel::Sam3 => {
            let (m, enc) = encoded(state, shared, job, placement)?;
            text_logits(m, enc, req.text, CONCEPT_THRESHOLD, req.main)?.map(|l| SegMask::from_logits(lightcraft_segment::MASK_SIDE, &l))
        }
    };
    // (the edge read now, not when the job was queued: set back to Current meanwhile, the part keeps its own edge)
    let matting = req.matting.filter(|e| e.edge(crate::hair_edges::EdgeKind::Subject) == crate::hair_edges::Edge::Matting);
    let (Some(edges), Some(base)) = (matting, base.as_ref()) else { return Ok(base) };
    job.render = saved;
    let input = render(job)?;
    let m = edges.matte(&input.rgb, input.w, input.h, &job.cancel).map_err(|e| match job.cancel.load(Ordering::Relaxed) {
        true => Failure::Cancelled,
        false => Failure::Other(format!("Hair edges: {e}")),
    })?;
    Ok(crate::hair_edges::subject_edge(base, &m))
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
        };
        let mut stub = Stub { matte_saw_render: false };
        let seg = combined(&mut stub, &mut job).unwrap().expect("a selection");
        assert!(stub.matte_saw_render);
        assert_eq!(seg.model.as_deref(), Some("sam3+birefnet"));
        assert_eq!(seg.side, 64);
    }
}
