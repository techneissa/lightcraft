//! The starting colour a raw's decode fits to its camera JPEG (`camera_preview::starting_colour_choice`: the colour
//! matrix, tone curve and hue/saturation table), kept for the session, so later decodes of the same file reuse it
//! (issue #220). A photo looked at while culling is decoded up to four times (its grid thumbnail, its loupe preview,
//! the full size for zoom and the full-size view), and the fit cost 87 ms of each.
//!
//! The key holds everything the fit reads: the file's bytes, the photo's colour option and local profile key, whether
//! single outlier photosites were replaced, whether a reduced copy of the raw image was developed, and the lens
//! correction applied to the sensor proxy. The fit itself is deterministic, so a hit gives exactly what a new fit
//! would. Memory only, bounded by count: an entry is a few matrices and curves and at most one hue/saturation table.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use lightcraft_color::Xy;
use lightcraft_raw::ColorData;
use lightcraft_raw::color::CameraTransform;

use crate::camera_preview::{CameraLook, LocalOutcome};
use crate::media::LoadOptions;

/// What the fit gives: the as-shot white, the camera transform at it, the fitted look and the local profile's outcome.
pub(super) type Fitted = (Xy, CameraTransform, Option<CameraLook>, LocalOutcome);

/// Fits kept at once. A culling session revisits the photos it has just seen (the full size after the loupe, the
/// full-size view after a zoom), so a few hundred recent ones cover it; each is a few kilobytes (at most ~40 KB with
/// a hue/saturation table).
const CAPACITY: usize = 256;

/// Bytes per piece of the file hashed side by side for the key.
const PIECE: usize = 4 << 20;

/// A kept fit, and the raw's colour tags after it when the fit changed them (a camera without a fitted look gets its
/// measured spectral matrices written into `raw.color`).
struct Entry {
    key: u128,
    fitted: Fitted,
    colour: Option<ColorData>,
}

static FITS: Mutex<VecDeque<Arc<Entry>>> = Mutex::new(VecDeque::new());

/// Fits made through [`fitted`] under each key (tests count them).
#[cfg(test)]
static MADE: Mutex<Vec<(u128, usize)>> = Mutex::new(Vec::new());

/// The key of a fit of `bytes` (the whole file) with the inputs that are not in the file.
pub(super) fn key(bytes: &[u8], options: LoadOptions, outliers: bool, reduced: bool, lens: Option<&lightcraft_develop::EmbeddedLens>) -> u128 {
    use rayon::prelude::*;
    let t = web_time::Instant::now();
    // the whole file, in pieces hashed side by side (a 50 MB raw in a few milliseconds): CRC-32 per piece, and the
    // header (camera, serial number, capture time) through the keyed hash as well
    let pieces: Vec<u32> = bytes.par_chunks(PIECE).map(crc32fast::hash).collect();
    let mut h = lightcraft_preview::Hasher128::new();
    h.u64(bytes.len() as u64).update(bytes.get(..bytes.len().min(1 << 16)).unwrap_or_default());
    for p in pieces {
        h.u64(u64::from(p));
    }
    h.str("options").u64(options.colour as u64);
    match options.profile_key {
        Some(k) => h.u64(1).u64(k as u64).u64((k >> 64) as u64),
        None => h.u64(0),
    };
    h.u64(u64::from(outliers)).u64(if outliers { crate::files::hot_pixel_limit().to_bits() } else { 0 }).u64(u64::from(reduced));
    // the lens correction (the file's own, or a database profile that can change while the app runs)
    h.str(&lens.and_then(|l| serde_json::to_string(l).ok()).unwrap_or_default());
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] camera look fit key: {:.1} ms for {} MB", t.elapsed().as_secs_f64() * 1e3, bytes.len() >> 20);
    }
    h.finish().0
}

/// The fit of `raw` kept under `key`, else `fit(raw)` kept there. A kept fit puts back the colour tags the fit wrote
/// into `raw`; `None` for `key` runs `fit` and keeps nothing.
pub(super) fn fitted(raw: &mut lightcraft_raw::RawImage, key: Option<u128>, fit: impl FnOnce(&mut lightcraft_raw::RawImage) -> Fitted) -> Fitted {
    let Some(key) = key else { return fit(raw) };
    if let Some(kept) = get(key) {
        if let Some(colour) = &kept.colour {
            raw.color = colour.clone();
        }
        if lightcraft_pipeline::profiling() {
            eprintln!("[profile] camera look fit: reused");
        }
        return kept.fitted.clone();
    }
    let before = raw.color.clone();
    let t = web_time::Instant::now();
    let fitted = fit(raw);
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] camera look fit: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    #[cfg(test)]
    {
        let mut made = MADE.lock().unwrap_or_else(PoisonError::into_inner);
        match made.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => made.push((key, 1)),
        }
    }
    let colour = (raw.color != before).then(|| raw.color.clone());
    put(Entry { key, fitted: fitted.clone(), colour });
    fitted
}

fn get(key: u128) -> Option<Arc<Entry>> {
    let mut fits = FITS.lock().unwrap_or_else(PoisonError::into_inner);
    let at = fits.iter().position(|e| e.key == key)?;
    // the most recently used last
    let e = fits.remove(at)?;
    fits.push_back(e.clone());
    Some(e)
}

fn put(entry: Entry) {
    let mut fits = FITS.lock().unwrap_or_else(PoisonError::into_inner);
    // (two decodes of one file at once both fit: the later one replaces the earlier, which is the same)
    fits.retain(|e| e.key != entry.key);
    fits.push_back(Arc::new(entry));
    while fits.len() > CAPACITY {
        fits.pop_front();
    }
}

/// How many fits were made under `key` (tests).
#[cfg(test)]
pub(crate) fn made(key: u128) -> usize {
    MADE.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|(k, _)| *k == key).map_or(0, |(_, n)| *n)
}

/// Forget the kept fit under `key` (tests: the decode with nothing kept, to compare with).
#[cfg(test)]
pub(crate) fn forget(key: u128) {
    FITS.lock().unwrap_or_else(PoisonError::into_inner).retain(|e| e.key != key);
}

/// Whether a fit is kept under `key` (tests).
#[cfg(test)]
pub(crate) fn kept(key: u128) -> bool {
    FITS.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|e| e.key == key)
}

#[cfg(test)]
#[path = "look_cache_tests.rs"]
mod tests;
