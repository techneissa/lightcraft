//! File ▸ Export Animated GIF: the selected photos, in view order, as the frames of one GIF. Each
//! photo is rendered with its edits straight from the original (a raw included: no JPEG in
//! between), fitted on black on a canvas whose shape is the first photo's, and quantised to its
//! own 256 colours, dithered by default ([`lightcraft_codecs::animated_gif`]).
//!
//! [`prepare`] reads the session (render jobs, no pixels) and [`Prepared::run`] does the work on
//! any thread, one frame at a time, with progress and a cancel flag. The file is written as
//! `<file>.part` and renamed when complete, so a cancelled or failed export leaves nothing behind.
//! A photo that can't be rendered is skipped with a warning.

use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::PhotoId;
use lightcraft_codecs::animated_gif::GifWriter;
use lightcraft_pipeline::{OutputDepth, OutputSpace};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::Session;
use crate::media::RenderJob;
use crate::originals::OriginalGuard;
use crate::slideshow_export::{ExportRun, PartFile, Progress, fit, sized};

/// Long side of the frames, in pixels. GIFs of photos grow fast: 1000 px frames run to about
/// 0.5–1 MB each.
pub const SIZE_MIN: usize = 64;
pub const SIZE_MAX: usize = 4096;
/// Seconds each photo stays up (GIF counts in hundredths; browsers show anything under 0.02 s
/// for 0.1 s).
pub const SECONDS_MIN: f64 = 0.02;
pub const SECONDS_MAX: f64 = 60.0;
/// The most photos an export takes.
pub const MAX_FRAMES: usize = 1000;

/// The export's settings (the dialog's state, and `export.animatedGif`'s params without `path`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GifSettings {
    /// Long side of the frames, in pixels.
    pub size: usize,
    /// Seconds each photo stays up.
    pub seconds: f64,
    /// Play forever (else once).
    #[serde(rename = "loop")]
    pub loop_forever: bool,
    /// Dither each frame to its palette (smooth gradients; slightly bigger files).
    pub dither: bool,
}

impl Default for GifSettings {
    fn default() -> Self {
        GifSettings { size: 800, seconds: 1.0, loop_forever: true, dither: true }
    }
}

impl GifSettings {
    /// Each frame's delay in hundredths of a second.
    pub fn delay(&self) -> u16 {
        (self.seconds.clamp(SECONDS_MIN, SECONDS_MAX) * 100.0).round() as u16
    }
}

/// `export.animatedGif`'s params, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub settings: GifSettings,
    pub path: String,
    pub ids: Option<Vec<PhotoId>>,
}

const KEYS: &str = "path, size, seconds, loop, dither, ids, wait";

impl Request {
    pub fn parse(p: &Value) -> Result<Request, String> {
        let o: &Map<String, Value> = p.as_object().ok_or("params must be an object")?;
        if let Some(k) = o.keys().find(|k| !KEYS.split(", ").any(|x| x == k.as_str())) {
            return Err(format!("unknown key `{k}` (known: {KEYS})"));
        }
        let mut s = GifSettings::default();
        let path = o.get("path").and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).ok_or("missing `path`")?.to_string();
        let set = |k: &str| o.get(k).filter(|v| !v.is_null());
        if let Some(v) = set("size") {
            s.size = v
                .as_u64()
                .filter(|n| (SIZE_MIN as u64..=SIZE_MAX as u64).contains(n))
                .ok_or_else(|| format!("`size` must be a whole number from {SIZE_MIN} to {SIZE_MAX}, not {v}"))? as usize;
        }
        if let Some(v) = set("seconds") {
            s.seconds = v
                .as_f64()
                .filter(|x| (SECONDS_MIN..=SECONDS_MAX).contains(x))
                .ok_or_else(|| format!("`seconds` must be a number from {SECONDS_MIN} to {SECONDS_MAX}, not {v}"))?;
        }
        for (k, field) in [("loop", &mut s.loop_forever), ("dither", &mut s.dither)] {
            if let Some(v) = set(k) {
                *field = v.as_bool().ok_or_else(|| format!("`{k}` must be true or false, not {v}"))?;
            }
        }
        if let Some(v) = o.get("wait").filter(|v| !v.is_null() && !v.is_boolean()) {
            return Err(format!("`wait` must be true or false, not {v}"));
        }
        let ids = match o.get("ids") {
            None | Some(Value::Null) => None,
            Some(Value::Array(a)) => {
                let ids: Option<Vec<PhotoId>> = a.iter().map(|v| v.as_u64().map(PhotoId)).collect();
                Some(ids.ok_or("`ids` must be an array of photo ids")?)
            }
            Some(v) => return Err(format!("`ids` must be an array of photo ids, not {v}")),
        };
        Ok(Request { settings: s, path, ids })
    }
}

struct Frame {
    name: String,
    job: Box<RenderJob>,
}

/// An export ready to run: everything it needs from the session.
pub struct Prepared {
    pub request: Request,
    frames: Vec<Frame>,
    pub width: usize,
    pub height: usize,
    guard: OriginalGuard,
    /// Problems found while preparing (photos not in the library).
    pub warnings: Vec<String>,
}

/// `ids` in the order the view shows them; ids it doesn't show keep their order, after them.
fn in_view_order(s: &mut Session, mut ids: Vec<PhotoId>) -> Vec<PhotoId> {
    let rank: std::collections::HashMap<PhotoId, usize> = s.visible_cloned().into_iter().enumerate().map(|(i, id)| (id, i)).collect();
    ids.sort_by_key(|id| rank.get(id).copied().unwrap_or(usize::MAX));
    ids.dedup();
    ids
}

/// Read the session: the photos (the request's `ids`, else the selection), their render jobs and
/// the canvas size.
pub fn prepare(s: &mut Session, p: &Value) -> Result<Prepared, String> {
    let request = Request::parse(p)?;
    let ids = match &request.ids {
        Some(ids) => ids.clone(),
        None => in_view_order(s, s.selected()),
    };
    if ids.is_empty() {
        return Err("select the photos for the GIF first".into());
    }
    if ids.len() > MAX_FRAMES {
        return Err(format!("an animated GIF takes at most {MAX_FRAMES} photos; {} are selected", ids.len()));
    }
    // the canvas takes the first photo's shape; the others fit inside it
    let ratio = ids.iter().find_map(|id| s.slide_photo_aspect(*id)).unwrap_or(1.5);
    let (width, height) = sized(ratio, request.settings.size);
    let mut warnings = Vec::new();
    let mut frames = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(name) = s.catalog.photo(id).map(|p| p.file_name.clone()) else {
            warnings.push(format!("photo {} is not in the library; skipped", id.0));
            continue;
        };
        match s.export_job(id, width, height, OutputSpace::Srgb, OutputDepth::U8) {
            Ok(job) => frames.push(Frame { name, job: Box::new(job) }),
            Err(e) => warnings.push(format!("{name}: {e}; skipped")),
        }
    }
    if frames.is_empty() {
        return Err("none of the photos can be exported".into());
    }
    s.check_write_target(&request.path)?;
    Ok(Prepared { request, frames, width, height, guard: s.original_guard(), warnings })
}

const CANCELLED: &str = "Animated GIF export cancelled";

impl Prepared {
    /// Photos to export.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Write the GIF. `cancel` is checked between frames.
    pub fn run(mut self, progress: Progress, cancel: &AtomicBool) -> Result<Value, String> {
        let st = self.request.settings.clone();
        let (w, h) = (self.width, self.height);
        let target = PathBuf::from(&self.request.path);
        let (part, file) = PartFile::new(&target)?;
        let mut gif = GifWriter::new(BufWriter::new(file), w, h, st.loop_forever).map_err(|e| e.to_string())?;
        let total = self.frames.len() as u64;
        let mut warnings = std::mem::take(&mut self.warnings);
        let mut skipped = 0;
        for (i, frame) in std::mem::take(&mut self.frames).into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            progress(i as u64, total, &frame.name);
            let img = match frame.job.run().rendered {
                Ok(r) => fit(r.image, w, h),
                Err(e) => {
                    warnings.push(format!("{}: {e}; skipped", frame.name));
                    skipped += 1;
                    continue;
                }
            };
            gif.add(&img.data, st.delay(), st.dither).map_err(|e| e.to_string())?;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let frames = gif.frames();
        if frames == 0 {
            return Err("no photo could be rendered".into());
        }
        let mut file = gif.finish().map_err(|e| e.to_string())?.into_inner().map_err(|e| format!("GIF: {}", e.error()))?;
        file.flush().map_err(|e| format!("GIF: {e}"))?;
        file.sync_all().map_err(|e| format!("GIF: {e}"))?;
        let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
        drop(file);
        part.commit(&self.guard)?;
        progress(total, total, "");
        Ok(json!({
            "format": "gif",
            "path": self.request.path,
            "width": w,
            "height": h,
            "frames": frames,
            "skipped": skipped,
            "seconds": frames as f64 * f64::from(st.delay()) / 100.0,
            "loop": st.loop_forever,
            "dither": st.dither,
            "bytes": bytes,
            "warnings": warnings,
        }))
    }
}

/// Run `prepared` to the end, recording progress and the outcome in `run` (and the row).
pub fn run_to_end(prepared: Prepared, run: &ExportRun, task: &crate::activity::TaskGuard) {
    let handle = task.handle();
    let progress = |done: u64, total: u64, detail: &str| {
        run.done.store(done, Ordering::Relaxed);
        run.total.store(total, Ordering::Relaxed);
        handle.progress(done, total);
        handle.detail(detail);
    };
    // a panic in a renderer or the encoder is an error, never a crash (the part file goes with it)
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepared.run(&progress, &run.cancel)))
        .unwrap_or_else(|_| Err("animated GIF export failed unexpectedly".into()));
    *run.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(r);
    run.finished.store(true, Ordering::Relaxed);
}
