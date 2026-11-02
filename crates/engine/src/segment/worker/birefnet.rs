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
    pub combined: bool,
    pub text: &'a str,
    pub main: bool,
    pub dir: &'a Path,
}

/// Run a BiRefNet job: `combined` lets SAM 3 (asked for `text`, the main subject's instances only when `main`) decide
/// what the subject is and BiRefNet draw its edge; otherwise BiRefNet's cut-out stands alone. `Ok(None)`: nothing found.
pub(super) fn run(state: &mut State, shared: &Shared, job: &mut Job, placement: Placement, req: &Request) -> Result<Option<SegMask>, Failure> {
    let Request { combined, text, main, dir } = *req;
    if !combined {
        let m = matte(state, shared, job, dir, placement)?;
        if !m.logits.iter().any(|l| *l > 0.0) {
            return Ok(None);
        }
        return Ok(Some(SegMask::from_logits(m.side, &m.logits).made_by(SelectionModel::BiRefNet.id())));
    }
    let side = lightcraft_segment::MASK_SIDE;
    let found = {
        let (m, enc) = encoded(state, shared, job, placement)?;
        text_logits(m, enc, text, CONCEPT_THRESHOLD, main)?
    };
    if job.cancel.load(Ordering::Relaxed) {
        return Err(Failure::Cancelled);
    }
    let m = matte(state, shared, job, dir, placement)?;
    // SAM 3's words found nothing (ferns, branches, a bottle): BiRefNet's cut-out stands alone, and says so
    let Some(sam) = found else {
        if !m.logits.iter().any(|l| *l > 0.0) {
            return Ok(None);
        }
        return Ok(Some(SegMask::from_logits(m.side, &m.logits).made_by(SelectionModel::BiRefNet.id())));
    };
    let logits = combine::refine_edges(&sam, side, &m);
    if !logits.iter().any(|l| *l > 0.0) {
        return Ok(None);
    }
    Ok(Some(SegMask::from_logits(m.side, &logits).made_by(SelectionModel::Combined.id())))
}
