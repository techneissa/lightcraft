//! Fork feature (`fork/glare-repair.md`, issue #74): Portrait ▸ Glasses Glare's Repair Glare.
//!
//! One run repaints the lenses of one face, or of every face with glasses, and keeps the result as each face's
//! [`Patch`] (a gain per pixel and a feathered lens mask over a square crop around the glasses). The face store hands
//! patches to every render ([`store::Repairs::attach`]); the Glasses Glare feature blends them in by its Amount, so a
//! slider drag never runs a model and the preview and the export match.
//!
//! - **Repaint (FLUX)**: FLUX.1 Fill dev (GGUF Q4_K_S with the Turbo LoRA, `lightcraft-flux`) on a 768 px crop, over
//!   the interior of each lens where glare hides the eye and the glints of the others. Two to three minutes a face
//!   on an M4 Pro, 14 GB while it runs. The weights are the user's (FLUX.1 [dev] is non-commercial); the folder is
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
        let guard = self.inner.activity.start("retouch.glare", label, Cancel::Yes);
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

fn run(inner: &Inner, job: Job, guard: &crate::activity::TaskGuard, stop: &Arc<AtomicBool>) -> Result<(), String> {
    let _one = lock(&inner.run);
    let Job { key, render, orient, faces, method, lenses: lenses_choice, lama, done, .. } = job;
    guard.detail("Rendering");
    let image = render.run().rendered?.image;
    let image = crate::retouch_input::parts::oriented(&image, orient);
    if stop.load(Ordering::Relaxed) {
        return Err("Glare repair stopped".into());
    }
    let n = lens::SIDE;
    // every face's crop and repaint mask first: a face without glare stops nothing
    let mut work = Vec::new();
    let mut skipped = Vec::new();
    for (i, face) in faces {
        let Some((area, eye_px)) = lens::crop_area(&face, image.width, image.height) else {
            skipped.push(format!("face {}: no glasses found", i + 1));
            continue;
        };
        let srgb = crop(&image, &area, n);
        let linear = to_linear(&srgb);
        let glasses = lens::glasses(&face, &area, n, eye_px);
        let lenses = lens::lenses(&linear, &srgb, &glasses, n);
        let which = if method == Method::Lama { lens::Lenses::Glints } else { lenses_choice };
        let mask = lens::repaint_mask(&lenses, n, which);
        if !mask.iter().any(|m| *m) {
            skipped.push(format!("face {}: no glare on its glasses", i + 1));
            continue;
        }
        work.push((i, face, Crop { area, srgb, linear }, mask));
    }
    if work.is_empty() {
        return Err(skipped.join("; "));
    }
    let made = match method {
        Method::Flux => flux(inner, &work, guard, stop)?,
        Method::Lama => lama_fill(&lama, &work, guard, stop)?,
    };
    for ((_, face, c, mask), repaired) in work.iter().zip(made) {
        let alpha = lens::feather(mask, n);
        let gain = gain(&c.srgb, &repaired, &alpha);
        let kb = face.key_bounds();
        // (the content itself: render caches are keyed by it, so a repair made again with other pixels is redrawn)
        let content: Vec<u8> = gain.data.iter().flatten().chain(&alpha.data).flat_map(|v| v.to_bits().to_le_bytes()).collect();
        let st = stamp(&[key.as_bytes(), method.name().as_bytes(), &kb.x0.to_le_bytes(), &kb.y0.to_le_bytes(), &c.area.x0.to_le_bytes(), &content]);
        let patch = Patch { area: c.area, gain, alpha, stamp: st };
        if let Err(e) = inner.repairs.add(&key, Saved { key_box: kb, method, patch: Arc::new(patch) }) {
            log::warn!("glare repair: kept in memory only: {e}");
        }
    }
    done();
    if skipped.is_empty() { Ok(()) } else { Err(format!("Repaired; {}", skipped.join("; "))) }
}

/// FLUX over each face's mask: the prompt's embeddings (from the disk cache, or made and kept), then the model,
/// loaded once for every face, with the GPU held.
fn flux(
    inner: &Inner,
    work: &[(usize, Face, Crop, Vec<bool>)],
    guard: &crate::activity::TaskGuard,
    stop: &Arc<AtomicBool>,
) -> Result<Vec<Rgb32f>, String> {
    let dir = lock(&inner.dir).clone().ok_or("no folder for the FLUX.1 Fill model")?;
    let w = lightcraft_flux::Weights::in_dir(&dir).map_err(|e| e.to_string())?;
    let steps = w.default_steps();
    let total = (steps * work.len()) as u64 + 1;
    let n = lens::SIDE;
    let cancelled = || stop.load(Ordering::Relaxed);
    let _gpu = crate::gpu_run::turn();
    let dev = lightcraft_flux::best_device();
    let cache = crate::config::cache_dir().map(|d| d.join("glare-repair"));
    let prompt_file = cache.as_ref().map(|d| d.join(format!("prompt-{:016x}.bin", stamp(&[PROMPT.as_bytes(), &w.stamp().to_le_bytes()]))));
    let prompt = match prompt_file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| lightcraft_flux::Prompt::from_bytes(&b)) {
        Some(p) => p,
        None => {
            guard.detail("Reading the prompt (T5)");
            let p = lightcraft_flux::text::encode(&w, PROMPT, &dev, &cancelled).map_err(|e| e.to_string())?;
            if let (Some(d), Some(f)) = (&cache, &prompt_file) {
                let _ = std::fs::create_dir_all(d).and_then(|()| std::fs::write(f, p.to_bytes()));
            }
            p
        }
    };
    guard.progress(1, total);
    if cancelled() {
        return Err("Glare repair stopped".into());
    }
    guard.detail("Loading FLUX.1 Fill");
    let model = lightcraft_flux::FillModel::load(&w, &dev).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(work.len());
    for (k, (i, _, c, mask)) in work.iter().enumerate() {
        guard.detail(&format!("Face {}", i + 1));
        let rgb: Vec<f32> = c.srgb.data.iter().flatten().copied().collect();
        let m: Vec<f32> = mask.iter().map(|v| if *v { 1.0 } else { 0.0 }).collect();
        let req = lightcraft_flux::Request { rgb: &rgb, mask: &m, side: n, steps, guidance: GUIDANCE, seed: 0 };
        let t = std::time::Instant::now();
        let res = model
            .fill(&req, &prompt, &mut |d, _| {
                guard.progress(1 + (k * steps + d) as u64, total);
                !cancelled()
            })
            .map_err(|e| match e {
                lightcraft_flux::Error::Cancelled => "Glare repair stopped".to_string(),
                e => e.to_string(),
            })?;
        log::info!("glare repair: face {} repainted in {:.1} s", i + 1, t.elapsed().as_secs_f64());
        let data: Vec<[f32; 3]> = res.as_chunks::<3>().0.to_vec();
        out.push(Rgb32f { width: n, height: n, data });
    }
    Ok(out)
}

/// LaMa over each face's glints (linear light, as the Remove panel's fill takes it).
fn lama_fill(
    lama: &crate::lama_fill::LamaFill,
    work: &[(usize, Face, Crop, Vec<bool>)],
    guard: &crate::activity::TaskGuard,
    stop: &Arc<AtomicBool>,
) -> Result<Vec<Rgb32f>, String> {
    use lightcraft_pipeline::learned_fill::{FillRun, LearnedFill};
    if !lama.installed() {
        return Err("LaMa is not installed (Remove panel ▸ Fill ▸ LaMa)".into());
    }
    let n = lens::SIDE;
    let run = FillRun::with_stop(stop.clone());
    let mut out = Vec::with_capacity(work.len());
    for (k, (i, _, c, mask)) in work.iter().enumerate() {
        guard.progress(k as u64, work.len() as u64);
        let rgb: Vec<f32> = c.linear.data.iter().flatten().copied().collect();
        let hole: Vec<f32> = mask.iter().map(|v| if *v { 1.0 } else { 0.0 }).collect();
        let filled = lama.fill(&run, &rgb, &hole, n, n, false).ok_or_else(|| {
            if stop.load(Ordering::Relaxed) {
                "Glare repair stopped".to_string()
            } else {
                lama.message().unwrap_or_else(|| format!("LaMa could not fill face {}", i + 1))
            }
        })?;
        let data: Vec<[f32; 3]> =
            filled.as_chunks::<3>().0.iter().map(|p| p.map(|v| lightcraft_color::transfer::linear_to_srgb(v.max(0.0)).clamp(0.0, 1.0))).collect();
        out.push(Rgb32f { width: n, height: n, data });
    }
    Ok(out)
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
}
