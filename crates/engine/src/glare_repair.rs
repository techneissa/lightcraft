//! Fork feature (`fork/glare-repair.md`, issue #74): Portrait ▸ Glasses Glare's Repair Glare.
//!
//! One run repaints the lenses of one face, or of every face with glasses, and keeps the result as each face's
//! [`Patch`] (a gain per pixel and a feathered lens mask over a square crop around the glasses). The face store hands
//! patches to every render ([`store::Repairs::attach`]); the Glasses Glare feature blends them in by its Amount, so a
//! slider drag never runs a model and the preview and the export match.
//!
//! - **Repaint (FLUX)**: FLUX.1 Fill dev (GGUF Q4_K_S with the Turbo LoRA, `lightcraft-flux`) on a 768 px crop, over
//!   the interior of each lens where glare hides the eye and the glints of the others. Two to three minutes a face
//!   on a Mac with a fast GPU, 14 GB while it runs (21–25 GB at the VAE's peak: 32 GB of memory or more). The weights are the user's (FLUX.1 [dev] is non-commercial); the folder is
//!   `ArtCraft/models/flux-fill` (or `LIGHTCRAFT_FLUX_DIR`). The model is loaded per run and freed at its end.
//! - **Fill Glints (LaMa)**: the Remove panel's LaMa over the glints only, in seconds.
//!
//! The run is a row in the activity stack with its progress (one step of FLUX is one unit) and ✕; it holds the shared
//! GPU lock while FLUX runs (LaMa takes it itself). Results are kept on disk (`<cache>/glare-repair`), so a restart
//! finds them again. Never panics: a missing model, a face without glasses, a failed or cancelled run each end with a
//! message the panel shows ([`GlareRepair::message`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::input::Patch;
use lightcraft_develop::retouch::{Face, NormRect};
use lightcraft_geom::Orientation;
use lightcraft_raster::{Plane, Rgb32f, Rgba8};

use crate::activity::{Activity, Cancel};

pub mod lens;
pub mod store;

pub use store::{Method, Repairs, Saved};

/// The prompt FLUX repaints with (a fixed description: a face's own description changed little in the trials).
pub const PROMPT: &str = "Close-up photo of a person wearing glasses, looking ahead with open eyes, in soft, even lighting. Clean \
transparent eyeglass lenses with no glare and no reflections. Natural, realistic eyes with detailed irises and eyelashes, natural skin \
texture and the same soft lighting as the rest of the face. Sharp photograph.";
/// FLUX.1 Fill's guidance.
const GUIDANCE: f64 = 30.0;
/// Below this linear value a gain is measured against this floor instead (a dark pupil would otherwise make a huge
/// gain out of a little noise).
const GAIN_FLOOR: f32 = 0.02;
/// What the other jobs waiting for the GPU call a running repaint.
const GPU_NAME: &str = "Glare Repair";

/// Where the FLUX.1 Fill files are looked for: a non-empty `LIGHTCRAFT_FLUX_DIR`; else on the Mac the folder shared
/// with the other Craft apps, `~/Library/Application Support/ArtCraft/models/flux-fill`; else `<config>/models/flux-fill`.
pub fn default_flux_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("LIGHTCRAFT_FLUX_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    if let Some(shared) = crate::shared_models::default_shared_models_dir() {
        return Some(shared.join("flux-fill"));
    }
    crate::config::config_dir().map(|c| c.join("models").join("flux-fill"))
}

/// The memory FLUX needs: its peak is 21–25 GB (the VAE decode) on top of what the Mac and the app already use.
pub const FLUX_MIN_MEMORY: usize = 32 << 30;

/// The message for a Mac with `total` bytes of memory that is too small to repaint with FLUX; `None` when it is
/// big enough, or when the platform does not tell (a Mac always does).
pub fn flux_memory_check(total: Option<usize>) -> Option<String> {
    let total = total?;
    (total < FLUX_MIN_MEMORY)
        .then(|| format!("Repaint needs a Mac with 32 GB of memory or more (this one has {} GB); use Fill Glints", total.div_ceil(1 << 30)))
}

/// What a running repair is doing, for the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub photo: PhotoId,
    /// The faces being repaired (indices of the photo's faces).
    pub faces: Vec<usize>,
    pub method: Method,
}

struct Inner {
    activity: Activity,
    dir: Mutex<Option<PathBuf>>,
    repairs: Repairs,
    /// One repair at a time.
    run: Mutex<()>,
    running: Mutex<Option<Running>>,
    message: Mutex<Option<String>>,
    /// Runs that kept at least one repair, until the session turns the faces' Amount up ([`GlareRepair::take_finished`]).
    finished: Mutex<Vec<Finished>>,
}

/// A run that kept repairs: the photo, the key boxes of the faces repaired and whether every face was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub photo: PhotoId,
    pub key_boxes: Vec<NormRect>,
    pub all_faces: bool,
}

/// Repair Glare of a session. Cheap to clone; every clone is the same.
#[derive(Clone)]
pub struct GlareRepair {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything a repair needs, set up on the calling thread.
pub struct Job {
    pub photo: PhotoId,
    /// The faces key of the photo (`Session::retouch_key`): repairs are kept under it.
    pub key: String,
    /// The photo with default settings, before its crop, unrotated (the frame people parsing renders in).
    pub render: crate::RenderJob,
    pub orient: Orientation,
    /// The faces to repair, with their index.
    pub faces: Vec<(usize, Face)>,
    /// The run was asked for every face (not one): the Amount shown is the global one where it hides the repair.
    pub all_faces: bool,
    pub method: Method,
    /// Which part of the lenses FLUX repaints (LaMa: the glints).
    pub lenses: lens::Lenses,
    /// The LaMa fill (for [`Method::Lama`]).
    pub lama: crate::lama_fill::LamaFill,
    /// Called once the repairs are kept (the face store attaches them to the photo's faces).
    pub done: Box<dyn FnOnce() + Send>,
}

impl GlareRepair {
    pub fn new(activity: Activity, repairs: Repairs) -> Self {
        GlareRepair {
            inner: Arc::new(Inner {
                activity,
                dir: Mutex::new(None),
                repairs,
                run: Mutex::new(()),
                running: Mutex::new(None),
                message: Mutex::new(None),
                finished: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Look for FLUX in `dir` (`None`: nowhere) and keep repairs on disk in `cache` (`None`: memory only).
    pub fn set_dirs(&self, dir: Option<PathBuf>, cache: Option<PathBuf>) {
        *lock(&self.inner.dir) = dir;
        self.inner.repairs.set_dir(cache);
    }

    /// The folder the FLUX files are looked for in.
    pub fn dir(&self) -> Option<PathBuf> {
        lock(&self.inner.dir).clone()
    }

    /// The FLUX files, or why they can't be used.
    pub fn weights(&self) -> Result<lightcraft_flux::Weights, String> {
        let dir = self.dir().ok_or("no folder for the FLUX.1 Fill model")?;
        lightcraft_flux::Weights::in_dir(&dir).map_err(|e| e.to_string())
    }

    /// Why this Mac can't repaint with FLUX (too little memory), or `None` when it can.
    pub fn memory_check(&self) -> Option<String> {
        flux_memory_check(crate::memory::total_ram())
    }

    /// The runs that kept repairs since the last call (the session shows them: Amount, in one undo step).
    pub fn take_finished(&self) -> Vec<Finished> {
        std::mem::take(&mut *lock(&self.inner.finished))
    }

    #[cfg(test)]
    pub(crate) fn mark_running_for_test(&self) {
        *lock(&self.inner.running) = Some(Running { photo: PhotoId(0), faces: vec![0], method: Method::Flux });
    }

    #[cfg(test)]
    pub(crate) fn note_finished(&self, f: Finished) {
        lock(&self.inner.finished).push(f);
    }

    /// Whether a run kept repairs the session has not shown yet.
    pub fn has_finished(&self) -> bool {
        !lock(&self.inner.finished).is_empty()
    }

    pub fn repairs(&self) -> &Repairs {
        &self.inner.repairs
    }

    /// The repair running now.
    pub fn running(&self) -> Option<Running> {
        lock(&self.inner.running).clone()
    }

    /// Why the last repair failed or found nothing (cleared when one starts).
    pub fn message(&self) -> Option<String> {
        lock(&self.inner.message).clone()
    }

    /// Run `job` on a worker thread with a row in the activity stack. An error when a repair already runs.
    pub fn start(&self, job: Job) -> Result<(), String> {
        {
            let mut running = lock(&self.inner.running);
            if running.is_some() {
                return Err("a glare repair is already running".into());
            }
            *running = Some(Running { photo: job.photo, faces: job.faces.iter().map(|(i, _)| *i).collect(), method: job.method });
        }
        *lock(&self.inner.message) = None;
        let label = match job.method {
            Method::Flux => "Repairing Glare (FLUX)",
            Method::Lama => "Filling Glints (LaMa)",
        };
        let guard = self.inner.activity.start_background("retouch.glare", label, Cancel::Yes);
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new().name("glare-repair".into()).spawn(move || {
            let stop = guard.cancel_flag().unwrap_or_default();
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&inner, job, &guard, &stop)));
            let message = match run {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("the glare repair stopped on an internal error".into()),
            };
            if let Some(m) = &message {
                log::warn!("glare repair: {m}");
            }
            *lock(&inner.message) = message;
            *lock(&inner.running) = None;
            drop(guard);
            // FLUX and its decode leave gigabytes the allocator keeps: give them back, whether the run worked or not
            crate::memory::release();
        });
        if let Err(e) = spawned {
            *lock(&self.inner.running) = None;
            return Err(format!("could not start the glare repair: {e}"));
        }
        Ok(())
    }
}

/// One face's crop: the area it covers, the picture (display-encoded sRGB, 0…1) and its linear values, `n × n`.
struct Crop {
    area: NormRect,
    srgb: Rgb32f,
    linear: Rgb32f,
}

/// The `n × n` crop of `img` (8-bit sRGB) over normalised `area`, sampled bilinearly from a box filter of the source
/// when it is larger (each crop pixel averages the source pixels under it); edges are held.
fn crop(img: &Rgba8, area: &NormRect, n: usize) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || img.data.len() != w * h {
        return Rgb32f::new(n, n);
    }
    let lin = |x: isize, y: isize| -> [f32; 3] {
        let (x, y) = (x.clamp(0, w as isize - 1) as usize, y.clamp(0, h as isize - 1) as usize);
        let p = img.data.get(y * w + x).copied().unwrap_or_default();
        [0, 1, 2].map(|c| lightcraft_color::transfer::decode_srgb8(p[c]))
    };
    let step = (area.width() * w as f64 / n as f64).max(area.height() * h as f64 / n as f64);
    let taps = step.ceil().clamp(1.0, 8.0) as isize;
    Rgb32f::from_fn(n, n, |x, y| {
        let (sx, sy) =
            ((area.x0 + (x as f64 + 0.5) / n as f64 * area.width()) * w as f64, (area.y0 + (y as f64 + 0.5) / n as f64 * area.height()) * h as f64);
        // average a taps × taps grid over the source pixels this crop pixel covers (linear light), then encode
        let mut sum = [0f32; 3];
        for j in 0..taps {
            for i in 0..taps {
                let ox = sx + ((i as f64 + 0.5) / taps as f64 - 0.5) * step;
                let oy = sy + ((j as f64 + 0.5) / taps as f64 - 0.5) * step;
                let p = lin(ox.floor() as isize, oy.floor() as isize);
                for c in 0..3 {
                    sum[c] += p[c];
                }
            }
        }
        let k = (taps * taps) as f32;
        sum.map(|v| lightcraft_color::transfer::linear_to_srgb(v / k).clamp(0.0, 1.0))
    })
}

fn to_linear(img: &Rgb32f) -> Rgb32f {
    Rgb32f { width: img.width, height: img.height, data: img.data.iter().map(|p| p.map(lightcraft_color::transfer::srgb_to_linear)).collect() }
}

/// The gain that turns `orig` into `repaired` where `alpha` is above 0; 1 elsewhere. Both are display-encoded sRGB
/// (0…1): the gain is measured in linear Rec. 2020, the space the retouching step works in, so it multiplies that
/// step's channels. A warm photo's blue (or any weak channel) would make a wild gain out of little: each channel is
/// measured against a floor of a twentieth of the pixel's luminance, and kept within 2.5× of the luminance's own gain,
/// so the repair's colour comes through without the noise of a near-empty channel.
pub fn gain(orig: &Rgb32f, repaired: &Rgb32f, alpha: &Plane) -> Rgb32f {
    const SPREAD: f32 = 2.5;
    let m = lightcraft_color::SRGB.to_space(&lightcraft_color::WORKING);
    let to2020 =
        |p: [f32; 3]| m.apply_f32(p.map(|v| lightcraft_color::transfer::srgb_to_linear(if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })));
    Rgb32f::from_fn(orig.width, orig.height, |x, y| {
        let i = y * orig.width + x;
        if alpha.data.get(i).copied().unwrap_or(0.0) <= 0.0 {
            return [1.0; 3];
        }
        let o = to2020(orig.data.get(i).copied().unwrap_or([1.0; 3]));
        let r = to2020(repaired.data.get(i).copied().unwrap_or([1.0; 3]));
        let (yo, yr) = (lightcraft_color::luminance_2020(o).max(GAIN_FLOOR), lightcraft_color::luminance_2020(r).max(0.0));
        let gy = yr.max(GAIN_FLOOR * 0.25) / yo;
        let floor = 0.05 * yo;
        [0, 1, 2].map(|c| {
            let g = (r[c].max(0.0) + floor) / (o[c].max(0.0) + floor);
            let g = g.clamp(gy / SPREAD, gy * SPREAD);
            if g.is_finite() { g.clamp(0.0, lightcraft_develop::retouch::input::GAIN_MAX) } else { 1.0 }
        })
    })
}

/// Hashes `parts` (FNV-1a, a rotation between parts), never 0.
fn stamp(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in *p {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h = h.rotate_left(7);
    }
    h | 1
}

/// The four sides of `r` as bytes, for [`stamp`].
fn rect_bytes(r: &NormRect) -> Vec<u8> {
    [r.x0, r.y0, r.x1, r.y1].iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// What a stopped run says.
const STOPPED: &str = "Glare repair stopped";

/// One face to repair: its index, the crop around its glasses and the mask FLUX (or LaMa) repaints.
struct Work {
    index: usize,
    face: Face,
    crop: Crop,
    mask: Vec<bool>,
}

/// Where a run keeps what it makes.
struct Keep<'a> {
    inner: &'a Inner,
    key: &'a str,
    photo: PhotoId,
    method: Method,
    all_faces: bool,
}

/// The crop and repaint mask of each face; the faces that give none are named in the second list.
fn plan(image: &Rgba8, faces: Vec<(usize, Face)>, method: Method, choice: lens::Lenses) -> (Vec<Work>, Vec<String>) {
    let n = lens::SIDE;
    let mut work = Vec::new();
    let mut skipped = Vec::new();
    for (index, face) in faces {
        let Some((area, eye_px)) = lens::crop_area(&face, image.width, image.height) else {
            skipped.push(format!("face {}: no glasses found", index + 1));
            continue;
        };
        let srgb = crop(image, &area, n);
        let linear = to_linear(&srgb);
        let glasses = lens::glasses(&face, &area, n, eye_px);
        let lenses = lens::lenses(&linear, &srgb, &glasses, n);
        let which = if method == Method::Lama { lens::Lenses::Glints } else { choice };
        let mask = lens::repaint_mask(&lenses, n, which);
        if !mask.iter().any(|m| *m) {
            skipped.push(format!("face {}: no glare on its glasses", index + 1));
            continue;
        }
        work.push(Work { index, face, crop: Crop { area, srgb, linear }, mask });
    }
    (work, skipped)
}

fn run(inner: &Inner, job: Job, guard: &crate::activity::TaskGuard, stop: &Arc<AtomicBool>) -> Result<(), String> {
    let _one = lock(&inner.run);
    let Job { photo, key, render, orient, faces, all_faces, method, lenses, lama, done, .. } = job;
    guard.detail("Rendering");
    let image = render.run().rendered?.image;
    let image = crate::retouch_input::parts::oriented(&image, orient);
    if stop.load(Ordering::Relaxed) {
        return Err(STOPPED.into());
    }
    // every face's crop and repaint mask first: a face without glare stops nothing
    let (work, skipped) = plan(&image, faces, method, lenses);
    if work.is_empty() {
        return Err(skipped.join("; "));
    }
    let keep = Keep { inner, key: &key, photo, method, all_faces };
    execute(&keep, &work, skipped, done, &mut |on_face| match method {
        Method::Flux => flux(inner, &work, guard, stop, on_face),
        Method::Lama => lama_fill(&lama, &work, guard, stop, on_face),
    })
}

/// Runs `paint` (which gives each repaired face to its callback as it finishes) and keeps every face the moment it
/// is made, so a stop at the second face keeps the first. When any face was kept, the faces are attached again
/// (`done`) and the run is noted for the session ([`GlareRepair::take_finished`]), which turns the Amount up: a run
/// that stops, finds nothing or fails to start leaves the Amount and the undo history alone.
fn execute(
    keep: &Keep<'_>,
    work: &[Work],
    skipped: Vec<String>,
    done: Box<dyn FnOnce() + Send>,
    paint: &mut dyn FnMut(&mut dyn FnMut(usize, Rgb32f)) -> Result<(), String>,
) -> Result<(), String> {
    let mut kept: Vec<NormRect> = Vec::new();
    let mut on_face = |k: usize, repaired: Rgb32f| {
        if let Some(w) = work.get(k) {
            kept.push(store_face(keep, w, &repaired));
        }
    };
    let result = paint(&mut on_face);
    if !kept.is_empty() {
        done();
        lock(&keep.inner.finished).push(Finished { photo: keep.photo, key_boxes: kept.clone(), all_faces: keep.all_faces });
    }
    match result {
        Ok(()) if skipped.is_empty() => Ok(()),
        Ok(()) => Err(format!("Repaired; {}", skipped.join("; "))),
        Err(e) if kept.is_empty() => Err(e),
        Err(e) => Err(format!("{e}; the {} finished before it {} kept", kept.len(), if kept.len() == 1 { "was" } else { "were" })),
    }
}

/// Keeps `repaired` as `w`'s face's patch (memory and disk); the face's key box.
fn store_face(keep: &Keep<'_>, w: &Work, repaired: &Rgb32f) -> NormRect {
    let n = lens::SIDE;
    let alpha = lens::feather(&w.mask, n);
    let gain = gain(&w.crop.srgb, repaired, &alpha);
    let kb = w.face.key_bounds();
    // (the content itself: render caches are keyed by it, so a repair made again with other pixels is redrawn)
    let content: Vec<u8> = gain.data.iter().flatten().chain(&alpha.data).flat_map(|v| v.to_bits().to_le_bytes()).collect();
    let st = stamp(&[keep.key.as_bytes(), keep.method.name().as_bytes(), &rect_bytes(&kb), &rect_bytes(&w.crop.area), &content]);
    let patch = Patch { area: w.crop.area, gain, alpha, stamp: st };
    if let Err(e) = keep.inner.repairs.add(keep.key, Saved { key_box: kb, method: keep.method, patch: Arc::new(patch) }) {
        log::warn!("glare repair: kept in memory only: {e}");
    }
    kb
}

/// Paints each face in turn with `paint(k, work)`, giving it to `on_face` as soon as it is made; a stop asked for
/// is looked at before every face.
fn paint_all(
    work: &[Work],
    stop: &AtomicBool,
    on_face: &mut dyn FnMut(usize, Rgb32f),
    paint: &mut dyn FnMut(usize, &Work) -> Result<Rgb32f, String>,
) -> Result<(), String> {
    for (k, w) in work.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return Err(STOPPED.into());
        }
        on_face(k, paint(k, w)?);
    }
    Ok(())
}

fn flux_error(e: lightcraft_flux::Error) -> String {
    match e {
        lightcraft_flux::Error::Cancelled => STOPPED.to_string(),
        e => e.to_string(),
    }
}

/// FLUX over each face's mask: the prompt's embeddings (from the disk cache, or made and kept), then the model,
/// loaded once for every face, with the GPU held.
fn flux(
    inner: &Inner,
    work: &[Work],
    guard: &crate::activity::TaskGuard,
    stop: &Arc<AtomicBool>,
    on_face: &mut dyn FnMut(usize, Rgb32f),
) -> Result<(), String> {
    if let Some(m) = flux_memory_check(crate::memory::total_ram()) {
        return Err(m);
    }
    let dir = lock(&inner.dir).clone().ok_or("no folder for the FLUX.1 Fill model")?;
    let w = lightcraft_flux::Weights::in_dir(&dir).map_err(|e| e.to_string())?;
    let steps = w.default_steps();
    let total = (steps * work.len()) as u64 + 1;
    let n = lens::SIDE;
    let cancelled = || stop.load(Ordering::Relaxed);
    if let Some(text) = crate::gpu_run::waiting_text() {
        guard.detail(&text);
    }
    let _gpu = crate::gpu_run::turn_for(GPU_NAME);
    let dev = lightcraft_flux::best_device();
    let cache = crate::config::cache_dir().map(|d| d.join("glare-repair"));
    let prompt_file = cache.as_ref().map(|d| d.join(format!("prompt-{:016x}.bin", stamp(&[PROMPT.as_bytes(), &w.stamp().to_le_bytes()]))));
    let prompt = match prompt_file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| lightcraft_flux::Prompt::from_bytes(&b)) {
        Some(p) => p,
        None => {
            guard.detail("Reading the prompt (T5)");
            let p = lightcraft_flux::text::encode(&w, PROMPT, &dev, &cancelled).map_err(flux_error)?;
            if let (Some(d), Some(f)) = (&cache, &prompt_file) {
                let _ = std::fs::create_dir_all(d).and_then(|()| std::fs::write(f, p.to_bytes()));
            }
            p
        }
    };
    guard.progress(1, total);
    if cancelled() {
        return Err(STOPPED.into());
    }
    guard.detail("Loading FLUX.1 Fill");
    let model = lightcraft_flux::FillModel::load(&w, &dev).map_err(flux_error)?;
    paint_all(work, stop, on_face, &mut |k, item| {
        guard.detail(&format!("Repainting Glare (Face {} of {})", k + 1, work.len()));
        let rgb: Vec<f32> = item.crop.srgb.data.iter().flatten().copied().collect();
        let m: Vec<f32> = item.mask.iter().map(|v| if *v { 1.0 } else { 0.0 }).collect();
        let req = lightcraft_flux::Request { rgb: &rgb, mask: &m, side: n, steps, guidance: GUIDANCE, seed: 0, cancel: Some(stop.clone()) };
        let t = std::time::Instant::now();
        let res = model
            .fill(&req, &prompt, &mut |d, _| {
                guard.progress(1 + (k * steps + d) as u64, total);
                !cancelled()
            })
            .map_err(flux_error)?;
        log::info!("glare repair: face {} repainted in {:.1} s", item.index + 1, t.elapsed().as_secs_f64());
        let data: Vec<[f32; 3]> = res.as_chunks::<3>().0.to_vec();
        Ok(Rgb32f { width: n, height: n, data })
    })
}

/// LaMa over each face's glints (linear light, as the Remove panel's fill takes it).
fn lama_fill(
    lama: &crate::lama_fill::LamaFill,
    work: &[Work],
    guard: &crate::activity::TaskGuard,
    stop: &Arc<AtomicBool>,
    on_face: &mut dyn FnMut(usize, Rgb32f),
) -> Result<(), String> {
    use lightcraft_pipeline::learned_fill::{FillRun, LearnedFill};
    if !lama.installed() {
        return Err("LaMa is not installed (Remove panel ▸ Fill ▸ LaMa)".into());
    }
    let n = lens::SIDE;
    let run = FillRun::with_stop(stop.clone());
    paint_all(work, stop, on_face, &mut |k, item| {
        guard.progress(k as u64, work.len() as u64);
        let rgb: Vec<f32> = item.crop.linear.data.iter().flatten().copied().collect();
        let hole: Vec<f32> = item.mask.iter().map(|v| if *v { 1.0 } else { 0.0 }).collect();
        let filled = lama.fill(&run, &rgb, &hole, n, n, false).ok_or_else(|| {
            if stop.load(Ordering::Relaxed) {
                STOPPED.to_string()
            } else {
                lama.message().unwrap_or_else(|| format!("LaMa could not fill face {}", item.index + 1))
            }
        })?;
        let data: Vec<[f32; 3]> =
            filled.as_chunks::<3>().0.iter().map(|p| p.map(|v| lightcraft_color::transfer::linear_to_srgb(v.max(0.0)).clamp(0.0, 1.0))).collect();
        Ok(Rgb32f { width: n, height: n, data })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gain_turns_the_original_into_the_repair_where_the_mask_is() {
        let m = lightcraft_color::SRGB.to_space(&lightcraft_color::WORKING);
        let lin = |p: [f32; 3]| m.apply_f32(p.map(lightcraft_color::transfer::srgb_to_linear));
        let orig = Rgb32f::from_fn(4, 1, |x, _| if x == 1 { [0.9, 0.5, 0.01] } else { [0.8, 0.7, 0.6] });
        let rep = Rgb32f::filled(4, 1, [0.5, 0.4, 0.35]);
        let alpha = Plane::from_fn(4, 1, |x, _| if x < 2 { 1.0 } else { 0.0 });
        let g = gain(&orig, &rep, &alpha);
        // a neutral-ish pixel: the gain turns the original (in Rec. 2020) into the repair
        let (o, r) = (lin(orig.data[0]), lin(rep.data[0]));
        let f = 0.05 * lightcraft_color::luminance_2020(o);
        for c in 0..3 {
            assert!(((o[c] + f) * g.data[0][c] - (r[c] + f)).abs() < 1e-4, "channel {c}: {} vs {}", o[c] * g.data[0][c], r[c]);
            assert!((o[c] * g.data[0][c] - r[c]).abs() < 0.15 * r[c], "channel {c}");
        }
        // a nearly empty channel can't run away: every gain stays near the luminance's
        let gy = lightcraft_color::luminance_2020(lin(rep.data[1])) / lightcraft_color::luminance_2020(lin(orig.data[1]));
        assert!(g.data[1].iter().all(|v| *v <= gy * 2.5 + 1e-4 && *v >= gy / 2.5 - 1e-4), "{:?} vs {gy}", g.data[1]);
        assert_eq!(g.data[3], [1.0; 3]);
        let nan = Rgb32f::filled(4, 1, [f32::NAN; 3]);
        assert!(gain(&orig, &nan, &alpha).data.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn a_crop_samples_its_area_and_holds_the_edges() {
        let img = Rgba8::from_fn(100, 50, |x, _| if x < 50 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let c = crop(&img, &NormRect::new(0.4, 0.0, 0.6, 0.4), 20);
        assert_eq!((c.width, c.height), (20, 20));
        assert!(c.data[0][0] < 0.01 && c.data[19][0] > 0.99);
        // past the photo's edge: the edge pixels
        let c = crop(&img, &NormRect::new(0.9, -0.5, 1.1, 0.5), 8);
        assert!(c.data.iter().all(|p| p[0] > 0.99));
        assert_eq!(crop(&Rgba8::new(0, 0), &NormRect::new(0.0, 0.0, 1.0, 1.0), 4).data.len(), 16);
    }

    // --- runs: what is kept, and when the Amount may turn up ---

    fn repairer() -> GlareRepair {
        let g = GlareRepair::new(Activity::default(), Repairs::default());
        g.set_dirs(None, None);
        g
    }

    fn two_faces() -> Vec<Work> {
        let n = lens::SIDE;
        let item = |index: usize, x: f64| {
            let face = Face::boxed(NormRect::new(x, 0.1, x + 0.2, 0.4));
            let crop =
                Crop { area: NormRect::new(x, 0.1, x + 0.2, 0.3), srgb: Rgb32f::filled(n, n, [0.8; 3]), linear: Rgb32f::filled(n, n, [0.6; 3]) };
            let mut mask = vec![false; n * n];
            mask[n * 10 + 10] = true;
            Work { index, face, crop, mask }
        };
        vec![item(0, 0.1), item(1, 0.6)]
    }

    fn keep<'a>(g: &'a GlareRepair, all: bool) -> Keep<'a> {
        Keep { inner: &g.inner, key: "photo", photo: PhotoId(7), method: Method::Flux, all_faces: all }
    }

    fn done_flag() -> (Box<dyn FnOnce() + Send>, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        (Box::new(move || f.store(true, Ordering::SeqCst)), flag)
    }

    #[test]
    fn a_run_that_finishes_keeps_its_faces_and_is_noted_for_the_amount() {
        let g = repairer();
        let work = two_faces();
        let (done, called) = done_flag();
        let img = Rgb32f::filled(lens::SIDE, lens::SIDE, [0.4; 3]);
        let r = execute(&keep(&g, true), &work, vec![], done, &mut |on_face| {
            on_face(0, img.clone());
            on_face(1, img.clone());
            Ok(())
        });
        assert_eq!(r, Ok(()));
        assert_eq!(g.repairs().of("photo").len(), 2);
        assert!(called.load(Ordering::SeqCst));
        let finished = g.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!((finished[0].photo, finished[0].key_boxes.len(), finished[0].all_faces), (PhotoId(7), 2, true));
        assert!(!g.has_finished(), "taken once");
    }

    #[test]
    fn a_stop_at_the_second_face_keeps_the_first() {
        let g = repairer();
        let work = two_faces();
        let (done, called) = done_flag();
        let img = Rgb32f::filled(lens::SIDE, lens::SIDE, [0.4; 3]);
        let stop = AtomicBool::new(false);
        let r = execute(&keep(&g, false), &work, vec![], done, &mut |on_face| {
            // the real loop: the stop is asked for between faces; here it comes while the first is being made
            paint_all(&work, &stop, on_face, &mut |k, _| {
                if k == 0 {
                    stop.store(true, Ordering::SeqCst);
                }
                Ok(img.clone())
            })
        });
        let message = r.unwrap_err();
        assert!(message.starts_with(STOPPED) && message.contains("1 finished before it was kept"), "{message}");
        assert_eq!(g.repairs().of("photo").len(), 1, "face 1's finished repair is kept");
        assert!(called.load(Ordering::SeqCst), "the faces are attached again");
        assert_eq!(g.take_finished().len(), 1);
    }

    #[test]
    fn a_run_that_keeps_nothing_leaves_no_trace_for_the_amount() {
        let g = repairer();
        let work = two_faces();
        // stopped before the first face, or failed
        for why in [STOPPED, "FLUX compute: out of memory"] {
            let (done, called) = done_flag();
            let r = execute(&keep(&g, false), &work, vec![], done, &mut |_| Err(why.to_string()));
            assert_eq!(r, Err(why.to_string()));
            assert!(!called.load(Ordering::SeqCst));
        }
        assert!(g.repairs().of("photo").is_empty());
        assert!(!g.has_finished() && g.take_finished().is_empty());
        // a stop already asked for never paints
        let stop = AtomicBool::new(true);
        let mut painted = 0;
        let r = paint_all(&work, &stop, &mut |_, _| {}, &mut |_, _| {
            painted += 1;
            Ok(Rgb32f::new(1, 1))
        });
        assert_eq!(r, Err(STOPPED.to_string()));
        assert_eq!(painted, 0);
    }

    #[test]
    fn the_stamp_follows_the_whole_face_box_and_crop() {
        let g = repairer();
        let img = Rgb32f::filled(lens::SIDE, lens::SIDE, [0.4; 3]);
        let stamp_of = |tall: f64| {
            let mut work = two_faces();
            work.truncate(1);
            work[0].face = Face::boxed(NormRect::new(0.1, 0.1, 0.3, 0.4 + tall));
            work[0].crop.area = NormRect::new(0.1, 0.1, 0.3, 0.3 + tall);
            store_face(&keep(&g, false), &work[0], &img);
            g.repairs().of("photo").last().map(|s| s.patch.stamp)
        };
        // only the far sides of the boxes differ (a stamp of x0 alone, or of the corner, would not see it)
        assert_ne!(stamp_of(0.0), stamp_of(0.1));
    }

    #[test]
    fn a_small_mac_is_refused_before_flux_loads() {
        let gb = |n: usize| Some(n << 30);
        let m = flux_memory_check(gb(16)).unwrap();
        assert!(m.contains("32 GB") && m.contains("Fill Glints") && m.contains("16 GB"), "{m}");
        assert!(flux_memory_check(gb(31)).is_some());
        assert_eq!(flux_memory_check(gb(32)), None);
        assert_eq!(flux_memory_check(gb(64)), None);
        // a platform that does not say is not refused
        assert_eq!(flux_memory_check(None), None);
    }
}
