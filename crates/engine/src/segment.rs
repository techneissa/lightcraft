//! AI masks: Object (clicks) and Describe (text) selections computed with SAM 3
//! (`lightcraft-segment`, cargo feature `sam`; the desktop app enables it).
//!
//! **Nothing requires the model.** It is not part of LightCraft (SAM License); the user
//! downloads it when they first use an AI mask and agree to (`segment.model.download`), or
//! puts the files in the model folder themselves. Without it, AI mask requests fail with a
//! clear "not installed" error; renders, exports and every other feature never touch it
//! (stored segmentations are plain data in the develop settings).
//!
//! The model sees the photo as developed, uncropped and without masks, at 1008 px — the frame
//! mask coordinates live in — so a stored segmentation lines up with the photo at any size.
//! Everything that touches the model (loading it, rendering and encoding the photo, clicks,
//! descriptions, detail passes) runs on one worker thread ([`worker`]); the session talks to it
//! over channels and never waits on a lock. With [`Segmenter::background`] (the desktop app)
//! commands return at once and [`Session::segment_poll`] applies the results; without it (CLI,
//! MCP, tests) they wait for the worker.

pub(crate) mod adaptive_jobs;
pub mod birefnet;
pub(crate) mod clicks_request;
#[cfg(feature = "sam")]
pub(crate) mod download;
pub(crate) mod request;
#[cfg(feature = "sam")]
mod worker;

use std::path::PathBuf;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::{MaskShape, SegMask};
use lightcraft_geom::Point;

use crate::Session;

/// Long edge of the image the model sees.
pub const INPUT_EDGE: usize = 1008;
/// Download size of the model (`model.safetensors`; the tokenizer files add ~2 MB).
pub const MODEL_BYTES: u64 = 3_439_938_512;
/// The model's licence (not LightCraft's): shown before downloading.
pub const LICENSE_NAME: &str = "SAM License (Meta)";
pub const LICENSE_URL: &str = "https://github.com/facebookresearch/sam3/blob/main/LICENSE";
/// How errors about a missing model start (frontends decide on [`crate::ErrorKind::ModelMissing`], not on this text).
pub const NOT_INSTALLED: &str = "The SAM 3 model is not installed";
/// Longest a waiting command (CLI, MCP) waits for the model (loading on a CPU can take minutes).
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
const WAIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// Most clicks one Object selection takes (the model's prompt grows with each).
pub const MAX_CLICKS: usize = 64;

/// One click of an Object selection (normalized image coordinates).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub at: Point,
    pub include: bool,
}

/// What a request's result is for (how [`Session::segment_poll`] applies it).
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) enum Tag {
    /// A command waiting for the result itself.
    Wait,
    /// Load the model and encode the photo ahead of the first click.
    Prepare,
    /// Object clicks on component `comp` of mask `mask`.
    Clicks { photo: PhotoId, mask: u32, comp: usize, hint: Vec<Point>, exclude: Vec<Point> },
    /// A Describe selection: a new mask (`mask: None`) or a component combined by `op`.
    NewPrompt { photo: PhotoId, mask: Option<u32>, op: String, name: Option<String>, text: String },
    /// A Subject, Sky or Background selection: a new mask (`mask: None`) or a component combined by `op`, added when
    /// SAM 3 has found it (`invert`: Background, the subject turned around).
    NewConcept {
        photo: PhotoId,
        mask: Option<u32>,
        op: String,
        name: Option<String>,
        kind: String,
        invert: bool,
        /// The model that makes it (Subject and Background: the dropdown's choice when the request was made).
        model: birefnet::SelectionModel,
    },
    /// A zoomed-in pass for a component, computed from `shape` (without its detail).
    Detail { photo: PhotoId, mask: u32, comp: usize, shape: MaskShape },
    /// Find Subject for component `comp` of mask `mask` (an Object waiting for it): the people /
    /// animals found become a "person" Describe selection; none, the object at `point`.
    Subject { photo: PhotoId, mask: u32, comp: usize, point: Point },
}

/// A background job currently queued or running on the SAM 3 worker thread.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
struct InFlight {
    id: u64,
    tag: Tag,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    detail: bool,
    /// The job's row in the activity stack; it goes when the job leaves `in_flight`.
    _task: crate::activity::TaskGuard,
}

/// The activity row's label for a request on the worker.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
fn activity_label(tag: &Tag) -> &'static str {
    match tag {
        Tag::Prepare => "Analyzing the photo for AI masks…",
        Tag::Detail { .. } => "Refining the mask's detail…",
        Tag::NewConcept { model: birefnet::SelectionModel::BiRefNet, .. } => "Selecting with BiRefNet",
        Tag::NewConcept { model: birefnet::SelectionModel::Combined, .. } => "Selecting with SAM 3 + BiRefNet",
        _ => "Selecting with SAM 3",
    }
}

/// The model input: 8-bit RGB.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) struct Input {
    rgb: Vec<u8>,
    w: usize,
    h: usize,
}

/// What a detail pass re-runs.
#[derive(Clone)]
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
pub(crate) enum Prompt {
    Clicks(Vec<Click>),
    Text(String),
    /// Subject, Sky or Background (the looser score; `true`: the main subject's instances only).
    Concept(String, bool),
}

/// Object clicks sent to the worker and not applied yet (background mode): further clicks add
/// to these, and the loupe shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingClicks {
    pub photo: PhotoId,
    pub mask: u32,
    pub comp: usize,
    pub hint: Vec<Point>,
    pub exclude: Vec<Point>,
}

/// What [`Session::segment_poll`] did.
#[derive(Debug, Default)]
pub struct Polled {
    /// A mask changed.
    pub changed: bool,
    /// Errors and notices from background work, for the user.
    pub messages: Vec<String>,
    /// A mask that got a new selection (worth a detail pass).
    pub refine: Option<u32>,
}

/// The model download (`segment.model.*`).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct DownloadStatus {
    pub running: bool,
    pub done: u64,
    pub total: u64,
    pub file: String,
    pub error: Option<String>,
    /// The last download was cancelled (it resumes next time): decide on this, not on `error`'s text.
    pub cancelled: bool,
    pub finished: bool,
}

/// The model (on its worker thread), the download, and AI mask requests in flight.
pub struct Segmenter {
    /// Where the `facebook/sam3` files live (set by the app; `None`: no AI masks).
    pub dir: Option<PathBuf>,
    /// The user's list of download mirrors (one base URL per line), if any.
    pub mirrors_file: Option<PathBuf>,
    /// Run requests in the background and apply them in [`Session::segment_poll`] (the
    /// desktop app); otherwise commands wait for their result (CLI, MCP, tests).
    pub background: bool,
    /// Optional remote AI worker endpoint override (host:port).
    pub remote: Option<String>,
    /// The selection model dropdown and the BiRefNet files (fork feature, issue #48).
    pub birefnet: birefnet::Settings,
    pending: Option<PendingClicks>,
    messages: Vec<String>,
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    next_job_id: u64,
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    in_flight: Vec<InFlight>,
    /// The last refined selection the worker made and the finer grids over its edge (fork, issue #240), until the
    /// mask it is for is added ([`Segmenter::attach_refined`]).
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    refined: Option<(SegMask, Vec<SegMask>)>,
    #[cfg(feature = "sam")]
    worker: worker::Worker,
    #[cfg(feature = "sam")]
    download: download::Downloader,
    #[cfg(feature = "sam")]
    results: (std::sync::mpsc::Sender<worker::Outcome>, std::sync::mpsc::Receiver<worker::Outcome>),
}

// (derivable only without the `sam` feature: the results channel has no Default)
#[allow(clippy::derivable_impls)]
impl Default for Segmenter {
    fn default() -> Self {
        Segmenter {
            dir: None,
            mirrors_file: None,
            background: false,
            remote: None,
            birefnet: birefnet::Settings::default(),
            pending: None,
            messages: Vec::new(),
            next_job_id: 1,
            in_flight: Vec::new(),
            refined: None,
            #[cfg(feature = "sam")]
            worker: worker::Worker::default(),
            #[cfg(feature = "sam")]
            download: download::Downloader::default(),
            #[cfg(feature = "sam")]
            results: std::sync::mpsc::channel(),
        }
    }
}

impl Segmenter {
    /// Keep the finer grids a worker result came with (fork, issue #240) for the mask its selection makes.
    #[cfg(feature = "sam")]
    fn keep_refined(&mut self, o: &worker::Outcome) {
        self.refined = match (&o.result, o.detail.is_empty()) {
            (Ok(Some(seg)), false) => Some((seg.clone(), o.detail.clone())),
            _ => None,
        };
    }

    /// Give a new Subject or Background `shape` the finer grids over its edge, when its selection is the refined one
    /// the worker made last (as made, or turned around for a Background). Fork, issue #240: the grids travel beside
    /// the selection from the worker to the mask, so the commands' parameters stay as they were.
    pub(crate) fn attach_refined(&mut self, shape: &mut MaskShape) {
        let (MaskShape::Subject { seg: Some(seg), detail, .. } | MaskShape::Background { seg: Some(seg), detail, .. }) = shape else { return };
        if !detail.is_empty() {
            return;
        }
        if let Some(grids) = self.take_refined(seg) {
            *detail = grids;
        }
    }

    /// The finer grids of `seg` when it is the refined selection the worker made last, as made or turned around (the
    /// grids turned with it); taken, so they go to one mask.
    pub(crate) fn take_refined(&mut self, seg: &SegMask) -> Option<Vec<SegMask>> {
        let (made, grids) = self.refined.as_ref()?;
        let out = if seg.data == made.data && seg.rect == made.rect {
            grids.clone()
        } else if made.invert().is_some_and(|inv| inv.data == seg.data && inv.rect == seg.rect) {
            grids.iter().filter_map(SegMask::invert).collect()
        } else {
            return None;
        };
        self.refined = None;
        Some(out)
    }
    /// Whether this build can compute AI masks at all.
    pub const AVAILABLE: bool = cfg!(feature = "sam");

    /// The configured remote AI worker endpoint, if any.
    pub fn remote_endpoint(&self) -> Option<String> {
        if let Some(r) = &self.remote {
            return Some(r.clone());
        }
        #[cfg(feature = "sam")]
        return lightcraft_segment::remote::configured();
        #[cfg(not(feature = "sam"))]
        None
    }

    /// Whether the local model files are in place.
    pub fn installed(&self) -> bool {
        #[cfg(feature = "sam")]
        if let Some(dir) = &self.dir {
            return lightcraft_segment::is_model_dir(dir);
        }
        false
    }

    /// Whether the SAM 3 model is available to segment, either locally installed or through a configured remote worker.
    pub fn has_model(&self) -> bool {
        self.installed() || self.remote_endpoint().is_some()
    }

    /// Whether requests are queued or running (loading, analyzing the photo, a click…).
    pub fn busy(&self) -> bool {
        #[cfg(feature = "sam")]
        {
            if self.background {
                return self.in_flight.iter().any(|j| !j.detail) && self.worker.pending() > 0;
            }
            self.worker.pending() > 0
        }
        #[cfg(not(feature = "sam"))]
        false
    }

    /// Whether the slow part runs now: loading the model or analyzing a photo.
    pub fn analyzing(&self) -> bool {
        #[cfg(feature = "sam")]
        {
            if self.background && self.in_flight.is_empty() {
                return false;
            }
            self.worker.encoding()
        }
        #[cfg(not(feature = "sam"))]
        false
    }

    /// Whether a zoomed-in detail pass is queued or running.
    pub fn detail_busy(&self) -> bool {
        #[cfg(feature = "sam")]
        {
            if self.background {
                return self.in_flight.iter().any(|j| j.detail) && self.worker.detail() > 0;
            }
            self.worker.detail() > 0
        }
        #[cfg(not(feature = "sam"))]
        false
    }

    /// Immediately cancel all in-flight background AI requests (e.g. when the user deletes
    /// all masks, switches tools, or presses the Cancel button next to the spinner).
    pub fn cancel_all(&mut self) -> bool {
        let had = !self.in_flight.is_empty() || self.pending.is_some();
        for job in self.in_flight.drain(..) {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.pending = None;
        had
    }

    /// Immediately cancel any in-flight AI requests targeting `mask` on `photo`.
    pub fn cancel_mask(&mut self, photo: PhotoId, mask: u32) -> bool {
        let mut cancelled = false;
        self.in_flight.retain(|job| {
            let matches_mask = match &job.tag {
                Tag::Clicks { photo: p, mask: m, .. } | Tag::Detail { photo: p, mask: m, .. } => *p == photo && *m == mask,
                Tag::NewPrompt { photo: p, mask: Some(m), .. } => *p == photo && *m == mask,
                Tag::NewConcept { photo: p, mask: Some(m), .. } => *p == photo && *m == mask,
                _ => false,
            };
            if matches_mask {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                cancelled = true;
                false
            } else {
                true
            }
        });
        if self.pending.as_ref().is_some_and(|p| p.photo == photo && p.mask == mask) {
            self.pending = None;
            cancelled = true;
        }
        cancelled
    }

    /// Whether the model is in memory (it is unloaded after a while without use).
    pub fn loaded(&self) -> bool {
        #[cfg(feature = "sam")]
        return self.worker.loaded();
        #[cfg(not(feature = "sam"))]
        false
    }

    /// Whether BiRefNet's Core ML version is in its folder (`tools/convert-coreml-birefnet.py`, issue #264).
    pub fn birefnet_coreml_installed(&self) -> bool {
        #[cfg(feature = "sam")]
        return self.birefnet_dir().is_some_and(|d| lightcraft_birefnet::coreml::installed(&d, lightcraft_birefnet::Variant::General));
        #[cfg(not(feature = "sam"))]
        false
    }

    /// Whether SAM 3's Core ML version is beside its folder (`tools/convert-coreml-sam3.py`, issue #268).
    pub fn sam3_coreml_installed(&self) -> bool {
        #[cfg(feature = "sam")]
        return self.dir.as_deref().is_some_and(lightcraft_segment::coreml::installed);
        #[cfg(not(feature = "sam"))]
        false
    }

    /// The engine SAM 3 ran on last (`coreml` or `candle`, issue #268); `None` before its first run.
    pub fn sam3_backend(&self) -> Option<&'static str> {
        #[cfg(feature = "sam")]
        return self.worker.sam3_backend();
        #[cfg(not(feature = "sam"))]
        None
    }

    /// The engine BiRefNet ran on last (`coreml` or `candle`, issue #264); `None` before its first run.
    pub fn birefnet_backend(&self) -> Option<&'static str> {
        #[cfg(feature = "sam")]
        return self.worker.birefnet_backend();
        #[cfg(not(feature = "sam"))]
        None
    }

    /// Clicks waiting for their segmentation.
    pub fn pending_clicks(&self) -> Option<&PendingClicks> {
        self.pending.as_ref()
    }

    /// The model download's state.
    pub fn download_status(&self) -> DownloadStatus {
        #[cfg(feature = "sam")]
        {
            let s = self.download.status();
            DownloadStatus {
                running: s.running,
                done: s.done,
                total: s.total,
                file: s.file,
                error: s.error,
                cancelled: s.cancelled,
                finished: s.finished,
            }
        }
        #[cfg(not(feature = "sam"))]
        DownloadStatus::default()
    }

    /// Where the model is downloaded from, in order (the user's mirrors first).
    pub fn mirrors(&self) -> Vec<String> {
        #[cfg(feature = "sam")]
        {
            let env = std::env::var(lightcraft_segment::fetch::MIRRORS_ENV).ok();
            lightcraft_segment::fetch::mirrors(env.as_deref(), self.mirrors_file.as_deref())
        }
        #[cfg(not(feature = "sam"))]
        Vec::new()
    }

    /// The model folder, if the model is there; otherwise an error that says what to do.
    pub fn model_dir(&self) -> Result<PathBuf, String> {
        self.require_model().map_err(|e| e.to_string())
    }

    /// [`Segmenter::model_dir`] for commands: a model that is not installed (or still downloading) is
    /// [`crate::EngineError::ModelMissing`], so a frontend offers the download without reading the message.
    pub fn require_model(&self) -> crate::Result<PathBuf> {
        let other = |why: &str| crate::EngineError::Other(why.into());
        if !Self::AVAILABLE {
            return Err(other("AI masks are not available in this build"));
        }
        #[cfg(feature = "sam")]
        if self.remote_endpoint().is_some() {
            return Ok(PathBuf::new());
        }
        let dir = self.dir.clone().ok_or_else(|| other("no folder is set for the SAM 3 model"))?;
        if self.installed() {
            return Ok(dir);
        }
        let d = self.download_status();
        let msg = if d.running {
            let pct = d.done.saturating_mul(100).checked_div(d.total).unwrap_or(0);
            format!("{NOT_INSTALLED} yet: it is downloading ({pct} %).")
        } else {
            format!(
                "{NOT_INSTALLED}. Download it (about {:.1} GB, {LICENSE_NAME}) when LightCraft offers it, with `segment.model.download {{\"acknowledged\": true}}`, or put model.safetensors, vocab.json and merges.txt in {}.",
                MODEL_BYTES as f64 / 1e9,
                dir.display()
            )
        };
        Err(crate::EngineError::ModelMissing { model: crate::AiModel::Sam3, msg })
    }

    /// Start downloading the model (on a background thread; see `segment.model.download`).
    /// `Ok(false)` when it is installed or downloading already.
    pub fn start_download(&self, activity: &crate::activity::Activity) -> Result<bool, String> {
        if !Self::AVAILABLE {
            return Err("AI masks are not available in this build".into());
        }
        let dir = self.dir.clone().ok_or("no folder is set for the SAM 3 model")?;
        if self.installed() {
            return Ok(false);
        }
        #[cfg(feature = "sam")]
        {
            let mirrors = self.mirrors();
            if mirrors.is_empty() {
                return Err(lightcraft_segment::fetch::no_mirrors_message());
            }
            self.download.start(lightcraft_segment::fetch::SAM3_FILES, mirrors, dir, lightcraft_segment::fetch::options(), activity, "SAM 3")
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (dir, activity);
            Ok(false)
        }
    }

    /// Stop a running download (its partial files stay, to resume). False when none runs.
    pub fn cancel_download(&self) -> bool {
        #[cfg(feature = "sam")]
        return self.download.cancel();
        #[cfg(not(feature = "sam"))]
        false
    }
}

/// Mix `v` into a 64-bit hash.
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
fn mix(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x0100_0000_01b3).rotate_left(29)
}

/// Objects covering more than this much of the photo gain little from a zoomed pass.
const DETAIL_MAX_AREA: f64 = 0.4;

/// The part of the photo to zoom into for `seg`'s selection: its bounding box with a margin of
/// a quarter of its size, at least 12 % of the long edge, at most 2:1, inside the photo
/// (`aspect` = width / height). `None` when nothing is selected or it is most of the photo.
pub fn detail_region(seg: &SegMask, aspect: f64) -> Option<[f64; 4]> {
    let logits = seg.logits()?;
    let r = seg.bounds()?;
    let side = seg.side as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, l) in logits.iter().enumerate() {
        if *l > 0.0 {
            let (x, y) = (i % side, i / side);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let (rw, rh) = (r[2] - r[0], r[3] - r[1]);
    let nx = |c: usize| r[0] + c as f64 / side as f64 * rw;
    let ny = |c: usize| r[1] + c as f64 / side as f64 * rh;
    let (bx0, by0, bx1, by1) = (nx(x0), ny(y0), nx(x1), ny(y1));
    if (bx1 - bx0) * (by1 - by0) > DETAIL_MAX_AREA {
        return None;
    }
    // in units of the photo's height: x × aspect
    let a = if aspect.is_finite() { aspect.clamp(0.1, 10.0) } else { 1.0 };
    let (cx, cy) = ((bx0 + bx1) / 2.0 * a, (by0 + by1) / 2.0);
    let size = ((bx1 - bx0) * a).max(by1 - by0);
    let m = 0.25 * size;
    let mut w = ((bx1 - bx0) * a + 2.0 * m).max(0.12 * a.max(1.0));
    let mut h = ((by1 - by0) + 2.0 * m).max(0.12 * a.max(1.0));
    w = w.max(h / 2.0).min(a);
    h = h.max(w / 2.0).min(1.0);
    let x = (cx - w / 2.0).clamp(0.0, (a - w).max(0.0));
    let y = (cy - h / 2.0).clamp(0.0, (1.0 - h).max(0.0));
    let out = [x / a, y, (x + w) / a, y + h];
    // a region nearly the whole photo isn't worth a second pass
    ((out[2] - out[0]) * (out[3] - out[1]) < 0.8).then_some(out)
}

/// `shape` without its detail patches (what a detail pass was computed from)
#[cfg_attr(not(feature = "sam"), allow(dead_code))]
fn strip_detail(mut shape: MaskShape) -> MaskShape {
    // (nor its edge setting: moving the slider meanwhile keeps the pass)
    match &mut shape {
        MaskShape::Object { detail, edge, .. }
        | MaskShape::Prompt { detail, edge, .. }
        | MaskShape::Subject { detail, edge, .. }
        | MaskShape::Sky { detail, edge, .. }
        | MaskShape::Background { detail, edge, .. } => {
            detail.clear();
            *edge = 0.0;
        }
        _ => {}
    }
    shape
}

fn clicks_of(include: &[Point], exclude: &[Point]) -> Vec<Click> {
    include.iter().map(|p| Click { at: *p, include: true }).chain(exclude.iter().map(|p| Click { at: *p, include: false })).collect()
}

impl Session {
    /// The key and settings of photo `id` as the model sees it: its look without masks, uncropped
    /// and unflipped, and without lens corrections, Upright and Transform. Mask coordinates
    /// (clicks, stored selections) live on the photo before those (`Mask::on_photo`), so a
    /// selection stays on its content when they change, and changing them doesn't ask again.
    #[cfg_attr(not(feature = "sam"), allow(dead_code))]
    pub(crate) fn segment_key(&self, id: PhotoId) -> Option<(u64, lightcraft_develop::DevelopSettings)> {
        let p = self.catalog.photo(id)?;
        let mut d = (*p.develop).clone();
        d.masks.clear();
        // (the crop doesn't show in the uncropped input: it doesn't ask again either)
        d.crop = Default::default();
        d.geometry = Default::default();
        d.optics.lens_profile = false;
        d.optics.distortion = 0.0;
        d.optics.remove_ca = false;
        d.optics.ca_red = 0.0;
        d.optics.ca_blue = 0.0;
        let content = crate::media::content_key(p);
        let mut h = mix(0xcbf2_9ce4_8422_2325, d.hash64());
        h = mix(h, id.0);
        for b in content.bytes() {
            h = mix(h, u64::from(b));
        }
        Some((h, d))
    }

    /// Clicks (in normalized photo coordinates) for the model.
    #[cfg(feature = "sam")]
    fn model_clicks(clicks: &[Click]) -> Vec<lightcraft_segment::Click> {
        clicks.iter().map(|c| lightcraft_segment::Click { x: c.at.x as f32, y: c.at.y as f32, positive: c.include }).collect()
    }

    /// Cancel any in-flight AI requests whose target photo, mask, component, or spot no longer
    /// exists (for example after `mask.delete`, `spot.delete`, or `edit.undo`).
    pub fn reconcile_segmenter(&mut self) {
        let active = self.active();
        let active_dev = active.and_then(|id| self.develop_of(id));
        let mut keep_pending = true;
        if let Some(p) = &self.segmenter.pending {
            let exists = active == Some(p.photo)
                && active_dev.as_ref().is_some_and(|d| d.masks.iter().find(|m| m.id == p.mask).is_some_and(|m| p.comp < m.components.len()));
            if !exists {
                keep_pending = false;
            }
        }
        if !keep_pending {
            self.segmenter.pending = None;
        }
        let mut i = 0;
        while i < self.segmenter.in_flight.len() {
            let valid = match &self.segmenter.in_flight[i].tag {
                Tag::Wait | Tag::Prepare => true,
                Tag::Clicks { photo, mask, comp, .. } | Tag::Detail { photo, mask, comp, .. } | Tag::Subject { photo, mask, comp, .. } => {
                    active == Some(*photo)
                        && self.develop_of(*photo).is_some_and(|d| d.masks.iter().find(|m| m.id == *mask).is_some_and(|m| *comp < m.components.len()))
                }
                Tag::NewPrompt { photo, mask: Some(mask), .. } => {
                    active == Some(*photo) && self.develop_of(*photo).is_some_and(|d| d.masks.iter().any(|m| m.id == *mask))
                }
                Tag::NewPrompt { photo, mask: None, .. } => active == Some(*photo),
                Tag::NewConcept { photo, mask: Some(mask), .. } => {
                    active == Some(*photo) && self.develop_of(*photo).is_some_and(|d| d.masks.iter().any(|m| m.id == *mask))
                }
                Tag::NewConcept { photo, mask: None, .. } => active == Some(*photo),
            };
            if valid {
                i += 1;
            } else {
                let job = self.segmenter.in_flight.remove(i);
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// A worker request about photo `id` (the render job for its model input goes along; the
    /// worker runs it only when its cache doesn't hold the photo). `region` renders that part
    /// of the photo, from a source sharp enough for it (detail passes).
    #[cfg(feature = "sam")]
    fn segment_job(
        &mut self,
        id: PhotoId,
        kind: worker::Kind,
        tag: Tag,
        reply: std::sync::mpsc::Sender<worker::Outcome>,
        region: Option<[f64; 4]>,
    ) -> Result<worker::Job, String> {
        self.segment_job_in(id, kind, tag, reply, region, false)
    }

    /// [`Self::segment_job`]; `without_spots`: the model sees the photo without its Remove spots (the Remove tool's
    /// Object clicks, `fork/object-remove.md`: a spot already over the object would hide it).
    #[cfg(feature = "sam")]
    fn segment_job_in(
        &mut self,
        id: PhotoId,
        kind: worker::Kind,
        tag: Tag,
        reply: std::sync::mpsc::Sender<worker::Outcome>,
        region: Option<[f64; 4]>,
        without_spots: bool,
    ) -> Result<worker::Job, String> {
        // (BiRefNet alone needs no SAM 3: its job carries its own folder)
        let dir = match &kind {
            worker::Kind::BiRefNet { model: birefnet::SelectionModel::BiRefNet, dir, .. } => dir.clone(),
            _ => self.segmenter.model_dir()?,
        };
        // old masks go onto the photo first: the selection that comes back is in its coordinates
        if let Some(cur) = self.develop_of(id).filter(|d| d.masks.iter().any(|m| !m.on_photo)) {
            let mut on = (*cur).clone();
            self.attach_masks(id, &cur, &mut on);
            self.apply_silent(lightcraft_catalog::Op::SetDevelop { id, settings: std::sync::Arc::new(on), label: "Masks".into(), edited: None })
                .map_err(|e| e.to_string())?;
        }
        let (mut key, mut d) = self.segment_key(id).ok_or("no such photo")?;
        if without_spots && !d.spots.is_empty() {
            d.spots.clear();
            key = mix(key, d.hash64() ^ 0x5350_4f54);
        }
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        let missing = match &p.source {
            lightcraft_catalog::Source::File { path } if !std::path::Path::new(path).exists() => Some(path.clone()),
            _ => None,
        };
        let render = match region {
            None => self.preview_job(id, INPUT_EDGE, INPUT_EDGE, false, &d).ok_or("no such photo")?,
            Some(region) => {
                d.crop.geometry =
                    lightcraft_geom::CropGeometry { rect: lightcraft_geom::Rect::new(region[0], region[1], region[2], region[3]), angle: 0.0 };
                d.crop.flip_h = false;
                d.crop.flip_v = false;
                d.geometry.constrain_crop = false;
                let mut job = self.preview_job(id, INPUT_EDGE, INPUT_EDGE, true, &d).ok_or("no such photo")?;
                let frac = (region[2] - region[0]).max(region[3] - region[1]).max(0.01);
                job.level = crate::media::SourceLevel::for_size((INPUT_EDGE as f64 / frac) as usize);
                job.source = self.media.source_ref(&p, job.level);
                job
            }
        };
        // (fork, issue #240: a Subject or Background a BiRefNet job makes is refined on the photo at a finer size)
        let refine = match (&kind, region) {
            (worker::Kind::BiRefNet { .. }, None) => {
                let edge = crate::hair_edges::refine::WORK_EDGE;
                self.preview_job(id, edge, edge, false, &d)
            }
            _ => None,
        };
        // (…and its matte is made from the picture a people parse uses, so the two share one matte)
        let plain = match (&kind, region) {
            (worker::Kind::BiRefNet { .. }, None) => {
                let edge = crate::people_parsing::PARSE_EDGE;
                self.preview_job(id, edge, edge, false, &lightcraft_develop::DevelopSettings::default())
            }
            _ => None,
        };
        let job_id = self.segmenter.next_job_id;
        self.segmenter.next_job_id = self.segmenter.next_job_id.wrapping_add(1).max(1);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let remote = self.segmenter.remote_endpoint();
        Ok(worker::Job {
            refine,
            plain,
            detail: Vec::new(),
            id: job_id,
            cancel,
            dir,
            key: if region.is_some() { 0 } else { key },
            render: Some(render),
            missing,
            kind,
            tag,
            reply,
            remote,
            runtime: self.model_runtimes.get(crate::model_runtime::SAM3),
            activity: Some(self.activity.clone()),
        })
    }

    /// Send a request and wait for its result (CLI, MCP, tests; or when not in background mode).
    #[cfg(feature = "sam")]
    fn segment_wait(&mut self, id: PhotoId, kind: worker::Kind, region: Option<[f64; 4]>) -> Result<Option<SegMask>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let job = self.segment_job(id, kind, Tag::Wait, tx, region)?;
        self.segmenter.worker.submit(job)?;
        match rx.recv_timeout(WAIT) {
            Ok(o) => {
                // (shown by the next poll in an app; logged anyway)
                self.segmenter.keep_refined(&o);
                self.segmenter.messages.extend(o.notice);
                o.result
            }
            Err(_) => Err("the AI model took too long; try again".into()),
        }
    }

    /// Send a request whose result [`Session::segment_poll`] applies.
    #[cfg(feature = "sam")]
    fn segment_queue(&mut self, id: PhotoId, kind: worker::Kind, tag: Tag, region: Option<[f64; 4]>) -> Result<(), String> {
        let reply = self.segmenter.results.0.clone();
        let detail = matches!(kind, worker::Kind::Detail { .. });
        let job = self.segment_job(id, kind, tag.clone(), reply, region)?;
        let job_id = job.id;
        let cancel = job.cancel.clone();
        self.segmenter.worker.submit(job)?;
        let task = self.activity.start_background("segment", activity_label(&tag), crate::activity::Cancel::Flag(cancel.clone()));
        self.segmenter.in_flight.push(InFlight { id: job_id, tag, cancel, detail, _task: task });
        Ok(())
    }

    /// Start preparing photo `id` for AI masks (load the model, encode the image), so the first
    /// click is fast. In background mode it returns at once.
    pub fn segment_prepare(&mut self, id: PhotoId) -> Result<(), String> {
        self.segmenter.model_dir()?;
        #[cfg(feature = "sam")]
        {
            if self.segmenter.background {
                return self.segment_queue(id, worker::Kind::Prepare, Tag::Prepare, None);
            }
            self.segment_wait(id, worker::Kind::Prepare, None)?;
        }
        #[cfg(not(feature = "sam"))]
        let _ = id;
        Ok(())
    }

    /// Segment the object `clicks` pick on photo `id`, waiting for the model.
    pub fn segment_clicks(&mut self, id: PhotoId, clicks: &[Click]) -> Result<SegMask, String> {
        self.segmenter.model_dir()?;
        if !clicks.iter().any(|c| c.include) {
            return Err("click the object to include it first".into());
        }
        if clicks.len() > MAX_CLICKS {
            return Err(format!("an Object selection takes at most {MAX_CLICKS} clicks"));
        }
        #[cfg(feature = "sam")]
        {
            self.segment_wait(id, worker::Kind::Clicks(Self::model_clicks(clicks)), None)?.ok_or_else(|| "nothing was selected".to_string())
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = id;
            Err("AI masks are not available in this build".into())
        }
    }

    /// Segment everything `text` describes on photo `id`, waiting for the model; `Ok(None)`
    /// when nothing matches.
    pub fn segment_text(&mut self, id: PhotoId, text: &str) -> Result<Option<SegMask>, String> {
        self.segmenter.model_dir()?;
        let text = text.trim();
        if text.is_empty() {
            return Err("describe what to select".into());
        }
        #[cfg(feature = "sam")]
        {
            self.segment_wait(id, worker::Kind::Text(text.to_string()), None)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = id;
            Err("AI masks are not available in this build".into())
        }
    }

    /// Segment a Subject, Sky or Background selection on photo `id` (the looser score, the main subject only where
    /// the concept says so), waiting for the model; `Ok(None)` when nothing matches.
    pub(crate) fn segment_concept(&mut self, id: PhotoId, concept: &crate::cmd::concepts::Concept) -> Result<Option<SegMask>, String> {
        #[cfg(feature = "sam")]
        {
            let (kind, _) = self.concept_kind(concept)?;
            self.segment_wait(id, kind, None)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, concept);
            Err("AI masks are not available in this build".into())
        }
    }

    /// Background mode: queue Object clicks for component `comp` of mask `mask`; the result
    /// (clicks and segmentation, one undo step) is applied by [`Session::segment_poll`].
    pub(crate) fn segment_clicks_later(&mut self, id: PhotoId, mask: u32, comp: usize, hint: Vec<Point>, exclude: Vec<Point>) -> Result<(), String> {
        self.segmenter.model_dir()?;
        if hint.is_empty() {
            return Err("click the object to include it first".into());
        }
        #[cfg(feature = "sam")]
        {
            let clicks = Self::model_clicks(&clicks_of(&hint, &exclude));
            let tag = Tag::Clicks { photo: id, mask, comp, hint: hint.clone(), exclude: exclude.clone() };
            self.segment_queue(id, worker::Kind::Clicks(clicks), tag, None)?;
            self.segmenter.pending = Some(PendingClicks { photo: id, mask, comp, hint, exclude });
            Ok(())
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, mask, comp, exclude);
            Err("AI masks are not available in this build".into())
        }
    }

    /// Background mode: queue a Describe selection; the mask (or component, combined by `op`
    /// into mask `mask`) is created by [`Session::segment_poll`] when something matches.
    pub(crate) fn segment_text_later(&mut self, id: PhotoId, mask: Option<u32>, op: &str, name: Option<String>, text: &str) -> Result<(), String> {
        self.segmenter.model_dir()?;
        let text = text.trim();
        if text.is_empty() {
            return Err("describe what to select".into());
        }
        #[cfg(feature = "sam")]
        {
            let tag = Tag::NewPrompt { photo: id, mask, op: op.to_string(), name, text: text.to_string() };
            self.segment_queue(id, worker::Kind::Text(text.to_string()), tag, None)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, mask, op, name);
            Err("AI masks are not available in this build".into())
        }
    }

    /// Background mode: queue a Subject, Sky or Background selection; the mask (or component, combined by `op` into
    /// mask `mask`) is created by [`Session::segment_poll`] when SAM 3 has found it. `text` is what the model is asked
    /// for (`concept`; Background turns the result around).
    pub(crate) fn segment_concept_later(
        &mut self,
        id: PhotoId,
        mask: Option<u32>,
        op: &str,
        name: Option<String>,
        concept: &crate::cmd::concepts::Concept,
    ) -> Result<(), String> {
        #[cfg(feature = "sam")]
        {
            let (kind, model) = self.concept_kind(concept)?;
            let tag = Tag::NewConcept { photo: id, mask, op: op.to_string(), name, kind: concept.kind.to_string(), invert: concept.invert, model };
            self.segment_queue(id, kind, tag, None)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, mask, op, name, concept);
            Err("AI masks are not available in this build".into())
        }
    }

    /// Find Subject for component `comp` of mask `mask` on photo `id` (an Object without clicks):
    /// the photo's main people / animals ([`worker::Kind::Subject`]), those the camera's `focus`
    /// falls on when the file records it; when the subject is neither, the object at `point`. In
    /// background mode [`Session::segment_poll`] applies the result; otherwise it is applied
    /// before this returns.
    pub(crate) fn segment_subject_later(&mut self, id: PhotoId, mask: u32, comp: usize, point: Point, focus: Option<Point>) -> Result<(), String> {
        self.segmenter.model_dir()?;
        #[cfg(feature = "sam")]
        {
            let kind = worker::Kind::Subject { focus: focus.map(|p| (p.x as f32, p.y as f32)) };
            let tag = Tag::Subject { photo: id, mask, comp, point };
            if self.segmenter.background {
                return self.segment_queue(id, kind, tag, None);
            }
            let seg = self.segment_wait(id, kind, None)?;
            let mut polled = Polled::default();
            self.apply_outcome(worker::Outcome { id: 0, tag, result: Ok(seg), superseded: false, notice: None, detail: Vec::new() }, &mut polled);
            match polled.messages.into_iter().next() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (id, mask, comp, point, focus);
            Err("AI masks are not available in this build".into())
        }
    }

    /// Start a zoomed-in pass for component `comp` of mask `mask` on photo `id` (an Object or
    /// Describe selection): the photo around the selection is rendered at a higher resolution
    /// and segmented again. In background mode [`Session::segment_poll`] applies the result;
    /// otherwise it is applied before this returns. `Ok(false)` when there is nothing to
    /// refine (no selection yet, or it is most of the photo) or a pass is running already.
    pub fn segment_detail(&mut self, id: PhotoId, mask: u32, comp: usize) -> Result<bool, String> {
        self.segmenter.model_dir()?;
        let d = self.develop_of(id).ok_or("no such photo")?;
        let shape = d.masks.iter().find(|m| m.id == mask).and_then(|m| m.components.get(comp)).map(|c| c.shape.clone()).ok_or("no such mask")?;
        let (seg, prompt) = match &shape {
            // (a document with absurdly many clicks isn't run through the model)
            MaskShape::Object { hint, exclude, seg: Some(seg), .. } if !hint.is_empty() && hint.len() + exclude.len() <= MAX_CLICKS => {
                (seg.clone(), Prompt::Clicks(clicks_of(hint, exclude)))
            }
            MaskShape::Prompt { text, seg: Some(seg), .. } => (seg.clone(), Prompt::Text(text.clone())),
            MaskShape::Subject { seg: Some(seg), .. } => (seg.clone(), Prompt::Concept(crate::cmd::concepts::SUBJECT.into(), true)),
            MaskShape::Sky { seg: Some(seg), .. } => (seg.clone(), Prompt::Concept(crate::cmd::concepts::SKY.into(), false)),
            _ => return Ok(false),
        };
        // (the zoomed-in pass is SAM 3's; a part another model made keeps its own edges)
        if seg.model.is_some() {
            return Ok(false);
        }
        let p = self.catalog.photo(id).ok_or("no such photo")?;
        let (pw, ph) = (f64::from(p.width.max(1)), f64::from(p.height.max(1)));
        let aspect = if d.orientation.swaps_axes() { ph / pw } else { pw / ph };
        let Some(region) = detail_region(&seg, aspect) else { return Ok(false) };
        #[cfg(feature = "sam")]
        {
            let kind = worker::Kind::Detail { prompt, region };
            let tag = Tag::Detail { photo: id, mask, comp, shape: strip_detail(shape) };
            if self.segmenter.background {
                if self.segmenter.detail_busy() {
                    return Ok(false);
                }
                self.segment_queue(id, kind, tag, Some(region))?;
            } else {
                let patch = self.segment_wait(id, kind, Some(region))?;
                let outcome = worker::Outcome { id: 0, tag, result: Ok(patch), superseded: false, notice: None, detail: Vec::new() };
                let mut polled = Polled::default();
                self.apply_outcome(outcome, &mut polled);
            }
            Ok(true)
        }
        #[cfg(not(feature = "sam"))]
        {
            let _ = (prompt, region, mask);
            Ok(false)
        }
    }

    /// Apply finished background requests (to masks that haven't changed since) and collect
    /// their messages. Cheap when nothing is pending: call it every frame.
    pub fn segment_poll(&mut self) -> Polled {
        self.reconcile_segmenter();
        let mut polled = Polled { messages: std::mem::take(&mut self.segmenter.messages), ..Default::default() };
        // Object clicks of the Remove tool (fork, `fork/object-remove.md`)
        polled.changed |= self.poll_object_jobs(&mut polled.messages);
        // AI mask parts found again on the photos a preset, paste or sync reached (fork, `fork/adaptive-masks.md`)
        polled.changed |= self.adaptive_poll();
        #[cfg(feature = "sam")]
        {
            // (before draining: a worker that is idle now has sent everything)
            let idle = !self.segmenter.busy();
            let done: Vec<worker::Outcome> = self.segmenter.results.1.try_iter().collect();
            for o in done {
                self.apply_outcome(o, &mut polled);
            }
            // clicks whose request was lost (the worker restarted) aren't pending forever
            if idle {
                self.segmenter.pending = None;
            }
        }
        polled
    }

    #[cfg(feature = "sam")]
    fn apply_outcome(&mut self, mut o: worker::Outcome, polled: &mut Polled) {
        if o.id != 0 {
            let was_tracked = self.segmenter.in_flight.iter().any(|j| j.id == o.id);
            self.segmenter.in_flight.retain(|j| j.id != o.id);
            if !was_tracked {
                // Job was explicitly cancelled before its outcome arrived; ignore silently.
                return;
            }
        }
        if o.superseded {
            return;
        }
        polled.messages.extend(o.notice);
        match o.tag {
            Tag::Wait => {}
            Tag::Prepare => {
                if let Err(e) = o.result {
                    polled.messages.push(e);
                }
            }
            Tag::Clicks { photo, mask, comp, hint, exclude } => {
                let latest = PendingClicks { photo, mask, comp, hint, exclude };
                if self.segmenter.pending.as_ref() != Some(&latest) {
                    return; // newer clicks are on their way
                }
                self.segmenter.pending = None;
                let seg = match o.result {
                    Ok(Some(seg)) => seg,
                    Ok(None) => return,
                    Err(e) => return polled.messages.push(e),
                };
                let Some(d) = self.develop_of(photo) else { return };
                let mut d = (*d).clone();
                let Some(c) = d.masks.iter_mut().find(|m| m.id == mask).and_then(|m| m.components.get_mut(comp)) else { return };
                let MaskShape::Object { edge, .. } = c.shape else { return };
                c.shape = MaskShape::Object { hint: latest.hint, exclude: latest.exclude, seg: Some(seg), detail: vec![], edge };
                match self.set_develop(photo, d, "Object Mask") {
                    Ok(()) => {
                        polled.changed = true;
                        polled.refine = Some(mask);
                    }
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
            Tag::NewPrompt { photo, mask, op, name, text } => {
                let seg = match o.result {
                    Ok(Some(seg)) => seg,
                    Ok(None) => return polled.messages.push(format!("Nothing matching “{text}” was found in this photo.")),
                    Err(e) => return polled.messages.push(e),
                };
                if self.active() != Some(photo) {
                    return polled.messages.push(format!("The photo changed before “{text}” was found; try again."));
                }
                let Ok(seg) = serde_json::to_value(seg) else { return };
                let mut p = serde_json::json!({"kind": "prompt", "text": text, "seg": seg});
                if let Some(n) = name {
                    p["name"] = serde_json::json!(n);
                }
                let r = match mask {
                    None => self.execute("mask.add", &p),
                    Some(m) => {
                        if !self.develop_of(photo).is_some_and(|d| d.masks.iter().any(|x| x.id == m)) {
                            return polled.messages.push("The mask was removed before the selection was found.".into());
                        }
                        self.active_mask = Some(m);
                        p["op"] = serde_json::json!(op);
                        self.execute("mask.addComponent", &p)
                    }
                };
                match r {
                    Ok(_) => {
                        polled.changed = true;
                        polled.refine = self.active_mask;
                    }
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
            Tag::NewConcept { photo, mask, op, name, kind, invert, .. } => {
                self.segmenter.refined = match (&o.result, o.detail.is_empty()) {
                    (Ok(Some(seg)), false) => Some((seg.clone(), std::mem::take(&mut o.detail))),
                    _ => None,
                };
                let seg = match o.result {
                    Ok(Some(mut seg)) => {
                        if invert {
                            let Some(inv) = seg.invert() else {
                                return polled.messages.push(format!("The {kind} selection could not be turned around; try again."));
                            };
                            seg = inv;
                        }
                        seg
                    }
                    Ok(None) => return polled.messages.push(crate::cmd::concepts::not_found(&kind)),
                    Err(e) => return polled.messages.push(e),
                };
                if self.active() != Some(photo) {
                    return polled.messages.push(format!("The photo changed before {kind} was found; try again."));
                }
                let Ok(seg_val) = serde_json::to_value(seg) else {
                    return polled.messages.push(format!("The {kind} selection could not be stored; try again."));
                };
                let mut p = serde_json::json!({"kind": kind, "seg": seg_val});
                if let Some(n) = name {
                    p["name"] = serde_json::json!(n);
                }
                let r = match mask {
                    None => self.execute("mask.add", &p),
                    Some(m) => {
                        if !self.develop_of(photo).is_some_and(|d| d.masks.iter().any(|x| x.id == m)) {
                            return polled.messages.push("The mask was removed before the selection was found.".into());
                        }
                        self.active_mask = Some(m);
                        p["op"] = serde_json::json!(op);
                        self.execute("mask.addComponent", &p)
                    }
                };
                match r {
                    Ok(_) => {
                        polled.changed = true;
                        polled.refine = self.active_mask;
                    }
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
            Tag::Subject { photo, mask, comp, point } => {
                let found = match o.result {
                    Ok(found) => found,
                    Err(e) => return polled.messages.push(e),
                };
                let Some(d) = self.develop_of(photo) else { return };
                let mut d = (*d).clone();
                let Some(c) = d.masks.iter_mut().find(|m| m.id == mask).and_then(|m| m.components.get_mut(comp)) else { return };
                // still the Object waiting for it (not clicked or replaced meanwhile)
                let MaskShape::Object { hint, seg: None, edge, .. } = &c.shape else { return };
                if !hint.is_empty() {
                    return;
                }
                let edge = *edge;
                let seg = match found {
                    Some(seg) => seg,
                    // neither a person nor an animal: the object at the point
                    None if self.segmenter.background => {
                        if let Err(e) = self.segment_clicks_later(photo, mask, comp, vec![point], Vec::new()) {
                            polled.messages.push(e);
                        }
                        return;
                    }
                    None => {
                        match self.segment_clicks(photo, &clicks_of(&[point], &[])) {
                            Ok(seg) => {
                                c.shape = MaskShape::Object { hint: vec![point], exclude: Vec::new(), seg: Some(seg), detail: Vec::new(), edge }
                            }
                            Err(e) => return polled.messages.push(e),
                        }
                        if let Err(e) = self.set_develop(photo, d, "Find Subject") {
                            polled.messages.push(e.to_string());
                        } else {
                            polled.changed = true;
                        }
                        return;
                    }
                };
                let text = lightcraft_segment::SUBJECT_CONCEPTS.join(", ");
                c.shape = MaskShape::Prompt { text, seg: Some(seg), detail: Vec::new(), edge };
                match self.set_develop(photo, d, "Find Subject") {
                    Ok(()) => polled.changed = true,
                    Err(e) => polled.messages.push(e.to_string()),
                }
            }
            Tag::Detail { photo, mask, comp, shape } => {
                let patch = match o.result {
                    Ok(Some(p)) => p,
                    Ok(None) => return,
                    Err(e) => return log::warn!("SAM 3 detail pass: {e}"),
                };
                let Some(d) = self.develop_of(photo) else { return };
                let mut d = (*d).clone();
                let Some(c) = d.masks.iter_mut().find(|m| m.id == mask).and_then(|m| m.components.get_mut(comp)) else { return };
                if strip_detail(c.shape.clone()) != shape {
                    return; // clicked again meanwhile: a newer pass will follow
                }
                match &mut c.shape {
                    MaskShape::Object { detail, .. }
                    | MaskShape::Prompt { detail, .. }
                    | MaskShape::Subject { detail, .. }
                    | MaskShape::Sky { detail, .. }
                    | MaskShape::Background { detail, .. } => *detail = vec![patch],
                    _ => return,
                }
                match self.set_develop(photo, d, "AI Mask Detail") {
                    Ok(()) => polled.changed = true,
                    Err(e) => log::warn!("AI mask detail: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "sam")]
    #[test]
    fn a_worker_notice_reaches_the_user_with_the_result() {
        // the job after which the model runs on the CPU (the GPU failed) says so
        let mut s = Session::with_demo();
        let notice = Some(worker::GPU_FAILED.to_string());
        s.segmenter
            .results
            .0
            .send(worker::Outcome { id: 0, tag: Tag::Prepare, result: Ok(None), superseded: false, notice, detail: Vec::new() })
            .unwrap();
        let polled = s.segment_poll();
        assert_eq!(polled.messages, vec![worker::GPU_FAILED.to_string()]);
        assert!(s.segment_poll().messages.is_empty(), "once");
    }

    /// A session whose model folder looks installed (the worker is never asked in these tests: results are sent by hand).
    #[cfg(feature = "sam")]
    fn session_with_a_model(name: &str) -> (Session, std::path::PathBuf) {
        let mut s = Session::with_demo();
        s.segmenter.background = true;
        let dir = std::env::temp_dir().join(format!("lc-test-sam-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"\x10\x00\x00\x00\x00\x00\x00\x00{}").unwrap();
        std::fs::write(dir.join("vocab.json"), b"{}").unwrap();
        std::fs::write(dir.join("merges.txt"), b"#version: 0.2\n").unwrap();
        s.segmenter.dir = Some(dir.clone());
        assert!(s.segmenter.installed());
        (s, dir)
    }

    #[cfg(feature = "sam")]
    fn concept_outcome(photo: PhotoId, mask: Option<u32>, kind: &str, invert: bool, seg: SegMask) -> worker::Outcome {
        let tag = Tag::NewConcept { photo, mask, op: "add".into(), name: None, kind: kind.into(), invert, model: birefnet::SelectionModel::Sam3 };
        worker::Outcome { id: 0, tag, result: Ok(Some(seg)), superseded: false, notice: None, detail: Vec::new() }
    }

    /// Subject, Sky and Background wait for SAM 3 like Describe: no rough mask shows meanwhile, the work has a row in
    /// the activity stack, and the mask appears with its stored segmentation (Background turned around).
    #[cfg(feature = "sam")]
    #[test]
    fn concept_masks_are_added_when_sam_has_found_them() {
        let (mut s, dir) = session_with_a_model("concept");
        let photo = s.active().unwrap();
        for kind in ["subject", "sky", "background"] {
            let r = s.execute("mask.add", &serde_json::json!({"kind": kind})).unwrap();
            assert_eq!(r["pending"], true, "{kind} is queued in background mode");
        }
        assert_eq!(s.segmenter.in_flight.len(), 3, "three jobs queued");
        assert!(s.develop_of(photo).unwrap().masks.is_empty(), "no mask (and no rough one) before SAM 3 has found it");
        let rows: Vec<_> = s.activity.list().into_iter().filter(|t| t.kind == "segment").collect();
        assert_eq!(rows.len(), 3, "each job has a row in the activity stack: {rows:?}");

        let seg = SegMask::from_logits(4, &[5.0; 16]);
        s.segmenter.results.0.send(concept_outcome(photo, None, "subject", false, seg.clone())).unwrap();
        let polled = s.segment_poll();
        assert!(polled.changed);
        let d = s.develop_of(photo).unwrap();
        let MaskShape::Subject { seg: Some(stored), .. } = &d.masks[0].components[0].shape else {
            panic!("expected a Subject with its segmentation")
        };
        assert_eq!(stored.side, 4);
        assert!(s.segmenter.in_flight.iter().all(|j| j.id != 0), "the finished job is gone");

        // Background: the subject turned around
        s.segmenter.results.0.send(concept_outcome(photo, None, "background", true, seg.clone())).unwrap();
        s.segment_poll();
        let d = s.develop_of(photo).unwrap();
        let MaskShape::Background { seg: Some(stored), .. } = &d.masks[1].components[0].shape else {
            panic!("expected a Background with its segmentation")
        };
        assert_eq!(stored.logits().unwrap().iter().map(|l| l.signum()).collect::<Vec<_>>(), vec![-1.0; 16], "everything but the subject");

        // deleting every mask cancels what is still running
        s.execute("mask.deleteAll", &serde_json::json!({})).unwrap();
        assert!(!s.segmenter.busy() && !s.segmenter.analyzing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing found: a message, never a rough mask. A job whose mask was deleted meanwhile is cancelled.
    #[cfg(feature = "sam")]
    #[test]
    fn concept_masks_report_nothing_found_and_follow_their_mask() {
        let (mut s, dir) = session_with_a_model("concept-none");
        let photo = s.active().unwrap();
        s.execute("mask.add", &serde_json::json!({"kind": "radial"})).unwrap();
        let mid = s.active_mask.unwrap();
        s.execute("mask.addComponent", &serde_json::json!({"id": mid, "op": "add", "kind": "sky"})).unwrap();
        assert_eq!(s.segmenter.in_flight.len(), 1);
        let none = worker::Outcome {
            id: s.segmenter.in_flight[0].id,
            tag: Tag::NewConcept {
                photo,
                mask: Some(mid),
                op: "add".into(),
                name: None,
                kind: "sky".into(),
                invert: false,
                model: birefnet::SelectionModel::Sam3,
            },
            result: Ok(None),
            superseded: false,
            notice: None,
            detail: Vec::new(),
        };
        s.segmenter.results.0.send(none).unwrap();
        let polled = s.segment_poll();
        assert!(!polled.changed && polled.messages.iter().any(|m| m.contains("No sky was found")), "{:?}", polled.messages);
        assert_eq!(s.develop_of(photo).unwrap().masks[0].components.len(), 1, "no rough part was added");

        // the mask is deleted while SAM 3 works: the job is cancelled, its row goes
        s.execute("mask.addComponent", &serde_json::json!({"id": mid, "op": "add", "kind": "subject"})).unwrap();
        assert_eq!(s.segmenter.in_flight.len(), 1);
        s.execute("mask.delete", &serde_json::json!({"id": mid})).unwrap();
        assert!(s.segmenter.in_flight.is_empty(), "cancelled with its mask");
        assert!(s.activity.list().iter().all(|t| t.kind != "segment"), "no row is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⌘Z of a slider change while Sky runs does not cancel the run: only commands that take masks away (or a photo
    /// change) cancel what is still running.
    #[cfg(feature = "sam")]
    #[test]
    fn undoing_a_slider_change_keeps_a_selection_running() {
        let (mut s, dir) = session_with_a_model("undo-slider");
        s.execute("develop.set", &serde_json::json!({"control": "light.exposure", "value": 0.5})).unwrap();
        s.execute("mask.add", &serde_json::json!({"kind": "sky"})).unwrap();
        assert_eq!(s.segmenter.in_flight.len(), 1, "Sky is running");
        s.execute("edit.undo", &serde_json::json!({})).unwrap();
        assert_eq!(s.segmenter.in_flight.len(), 1, "the slider's undo leaves it running");
        // taking the masks away does cancel it: deleting the only mask, or resetting the photo
        s.execute("mask.add", &serde_json::json!({"kind": "radial"})).unwrap();
        s.execute("mask.deleteAll", &serde_json::json!({})).unwrap();
        assert!(s.segmenter.in_flight.is_empty(), "cancelled");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `mask.update` with a Subject, Sky or Background shape asks SAM 3 for it like `mask.add` does (MCP, CLI), so the
    /// part is not left as the rough mask; without the model it still works as before.
    #[cfg(feature = "sam")]
    #[test]
    fn updating_a_part_to_sky_uses_the_model_when_there_is_one() {
        let update = serde_json::json!({"shape": {"kind": "sky"}});
        let (mut s, dir) = session_with_a_model("update-sky");
        s.segmenter.background = false;
        s.execute("mask.add", &serde_json::json!({"kind": "radial"})).unwrap();
        // (the fake model folder can't load: the model is asked, and its error comes back)
        let e = s.execute("mask.update", &update).unwrap_err().to_string();
        assert!(e.to_lowercase().contains("sam") || e.contains("model"), "the model was asked: {e}");
        let _ = std::fs::remove_dir_all(&dir);

        let mut none = Session::with_demo();
        none.segmenter.background = false;
        none.execute("mask.add", &serde_json::json!({"kind": "radial"})).unwrap();
        none.execute("mask.update", &update).unwrap();
        let d = none.develop_of(none.active().unwrap()).unwrap();
        assert!(matches!(d.masks[0].components[0].shape, MaskShape::Sky { seg: None, .. }), "no model: the rough mask, as before");
    }

    /// Find Subject's result: people / animals found become a Describe selection of the waiting
    /// Object (one undo step), not after it was clicked meanwhile; nothing found queues the object
    /// at the point (here: no model, so a message, and the mask still waits).
    #[cfg(feature = "sam")]
    #[test]
    fn find_subject_results_fill_the_waiting_mask() {
        use lightcraft_develop::{Mask, MaskComponent, MaskOp};
        let mut s = Session::with_demo();
        s.segmenter.dir = Some(std::env::temp_dir().join(format!("lc-no-sam3-subject-{}", std::process::id())));
        let id = s.active().unwrap();
        let waiting = || MaskShape::Object { hint: vec![], exclude: vec![], seg: None, detail: vec![], edge: 0.0 };
        let mut d = (*s.develop_of(id).unwrap()).clone();
        d.masks.push(Mask {
            id: 3,
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: waiting() }],
            ..Default::default()
        });
        s.set_develop(id, d, "test").unwrap();
        let point = Point::new(0.4, 0.5);
        let send = |s: &mut Session, result| {
            let tag = Tag::Subject { photo: id, mask: 3, comp: 0, point };
            s.segmenter.results.0.send(worker::Outcome { id: 0, tag, result, superseded: false, notice: None, detail: Vec::new() }).unwrap();
            s.segment_poll()
        };
        let shape = |s: &Session| s.develop_of(id).unwrap().masks[0].components[0].shape.clone();
        // nothing found, no model to click with: said so, and the mask still waits
        let polled = send(&mut s, Ok(None));
        assert!(!polled.messages.is_empty() && shape(&s) == waiting(), "{:?}", polled.messages);
        // people found: a Describe selection of them
        let polled = send(&mut s, Ok(Some(seg_box(20, 5, 5, 15, 15))));
        assert!(polled.changed && polled.messages.is_empty());
        let MaskShape::Prompt { text, seg: Some(_), .. } = shape(&s) else { panic!("{:?}", shape(&s)) };
        assert_eq!(text, "person, animal");
        // a result for a mask that changed meanwhile is dropped
        let before = shape(&s);
        assert!(!send(&mut s, Ok(Some(seg_box(20, 0, 0, 4, 4)))).changed);
        assert_eq!(shape(&s), before);
        // an error reaches the user
        assert_eq!(send(&mut s, Err("boom".into())).messages, vec!["boom".to_string()]);
    }

    fn seg_box(side: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> SegMask {
        let l: Vec<f32> =
            (0..side * side).map(|i| if (x0..x1).contains(&(i % side)) && (y0..y1).contains(&(i / side)) { 8.0 } else { -8.0 }).collect();
        SegMask::from_logits(side, &l)
    }

    #[test]
    fn detail_region_frames_the_selection_with_a_margin() {
        // a small object in the middle of a square photo
        let r = detail_region(&seg_box(100, 40, 45, 50, 55), 1.0).unwrap();
        assert!(r[0] < 0.4 && r[2] > 0.5 && r[1] < 0.45 && r[3] > 0.55, "contains it: {r:?}");
        assert!(r[2] - r[0] < 0.25 && r[3] - r[1] < 0.25, "zoomed in: {r:?}");
        assert!(r.iter().all(|v| (0.0..=1.0).contains(v)));
        // at the corner: shifted inside the photo, not cut
        let r = detail_region(&seg_box(100, 0, 0, 5, 5), 1.5).unwrap();
        assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[0] < 1e-9 && r[1] < 1e-9);
        // a thin object stays at most 2:1 (in photo pixels, 1.5:1 photo)
        let r = detail_region(&seg_box(100, 10, 50, 90, 52), 1.5).unwrap();
        let (w, h) = ((r[2] - r[0]) * 1.5, r[3] - r[1]);
        assert!(w / h <= 2.0 + 1e-9, "{w} × {h}");
        // a hostile aspect ratio stays sane
        assert!(detail_region(&seg_box(100, 40, 45, 50, 55), f64::NAN).is_some_and(|r| r.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn no_region_for_nothing_or_most_of_the_photo() {
        assert!(detail_region(&seg_box(50, 0, 0, 0, 0), 1.0).is_none());
        assert!(detail_region(&seg_box(50, 2, 2, 48, 48), 1.0).is_none());
        let damaged = SegMask { side: 50, data: "?".into(), rect: None, model: None, grid: None };
        assert!(detail_region(&damaged, 1.0).is_none());
    }
}
