//! A loupe job that converts, fits and hands on only what its render changed (issue #111).
//!
//! The GPU renderer says which rectangles of its output differ from the view's last render (and which render that
//! was, by its frame number: [`lightcraft_gpu::FrameNote`]). The job keeps, with the view's stages, what it made of
//! each render on the host: the image converted for the display, its sRGB twin (the view cache's and the histogram's)
//! and the image fitted to the drawn size. The next job starts from those copies and converts and fits only the
//! rectangles that changed. Both steps are exact per rectangle: the display conversions map each pixel on its own, and
//! the Lanczos fit gives each output pixel from fixed taps in a fixed order ([`fit_rects`]), so the result has the
//! same bits as converting and fitting the whole image. The job's result says which rectangles of its picture changed
//! ([`Changed`]), so the window uploads only those.

use std::sync::Arc;

use lightcraft_raster::resample::{Filter, Pixel, fit_size, weights};
use lightcraft_raster::{Histogram, Rgba8};

/// `[x0, y0, x1, y1)` in pixels.
pub type Rect = [usize; 4];

/// What a render says it changed ([`lightcraft_gpu::FrameNote`]).
#[cfg(not(target_arch = "wasm32"))]
use lightcraft_gpu::FrameNote;

/// The web renders on the CPU, which says nothing about what changed: the same shape, never made.
#[cfg(target_arch = "wasm32")]
struct FrameNote {
    frame: u64,
    changed: Option<(u64, Vec<Rect>)>,
}

/// A clock read for a `LIGHTCRAFT_PROFILE` timing line: the time now when profiling, else `None`, so that nothing reads
/// a clock otherwise (and `web_time`: the web has no `std::time::Instant`).
pub fn clock() -> Option<web_time::Instant> {
    lightcraft_pipeline::profiling().then(web_time::Instant::now)
}

/// Milliseconds from `a` to `b` (0 without both).
pub fn ms(a: Option<web_time::Instant>, b: Option<web_time::Instant>) -> f64 {
    a.zip(b).map_or(0.0, |(a, b)| b.saturating_duration_since(a).as_secs_f64() * 1e3)
}

/// Which picture of a view a job's image is, and the rectangles where it differs from an earlier one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    /// The render this image was made from ([`lightcraft_gpu::FrameNote::frame`]).
    pub frame: u64,
    /// The earlier picture (its `frame`) this one equals outside `rects`.
    pub from: Option<u64>,
    pub rects: Vec<Rect>,
}

/// What a job kept of one render on the host: always what the next job checks it against, and an image only where the
/// next job starts from it.
struct Kept {
    /// The display it was converted for (`None`: not converted).
    display: Option<u64>,
    /// The render's size.
    size: (usize, usize),
    /// The sRGB twin of the converted image, when one was made (the view cache's).
    srgb: Option<Arc<Rgba8>>,
    /// The image converted for `display` ([`Patch::convert`] starts from it; not kept when nothing was converted).
    converted: Option<Arc<Rgba8>>,
    /// Whether the picture was fitted to the drawn size.
    fitted: bool,
    /// The fit, when it was made on the host ([`Patch::fit`] starts from it).
    fit: Option<Arc<Rgba8>>,
    /// The fit was made on the device (its bits differ from the CPU's within a level of rounding).
    device_fit: bool,
}

impl Kept {
    /// Bytes of the images it holds.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn bytes(&self) -> usize {
        [&self.srgb, &self.converted, &self.fit].into_iter().flatten().map(|i| i.data.len() * 4).sum()
    }
}

/// The images a job made of one render that the next job starts from ([`Patch::finish`]): the image converted for the
/// display (before any fit), its sRGB twin, and the fit made on the host.
pub(super) struct Made {
    pub srgb: Option<Arc<Rgba8>>,
    pub converted: Option<Arc<Rgba8>>,
    pub fit: Option<Arc<Rgba8>>,
}

/// The job's work on one render, between the readback and the result.
pub(super) struct Patch {
    note: Option<FrameNote>,
    from: Option<Arc<Kept>>,
    /// Rectangles of the render that changed, when `from` holds the earlier render's images.
    rects: Option<Vec<Rect>>,
}

#[cfg(not(target_arch = "wasm32"))]
fn kept_of(stages: Option<&lightcraft_pipeline::StageCache>, frame: u64) -> Option<Arc<Kept>> {
    lightcraft_gpu::host_of(stages?, frame)?.downcast::<Kept>().ok()
}

#[cfg(target_arch = "wasm32")]
fn kept_of(_: Option<&lightcraft_pipeline::StageCache>, _: u64) -> Option<Arc<Kept>> {
    None
}

/// Keep `kept` with `stages` as what the job made of render `frame`, counted in the stages' bytes.
#[cfg(not(target_arch = "wasm32"))]
fn keep(stages: &lightcraft_pipeline::StageCache, frame: u64, kept: Kept) {
    let bytes = kept.bytes();
    lightcraft_gpu::keep_host(stages, frame, Arc::new(kept), bytes);
}

#[cfg(target_arch = "wasm32")]
fn keep(_: &lightcraft_pipeline::StageCache, _: u64, _: Kept) {}

impl Patch {
    /// Forget what an earlier render on this thread said (before a render).
    pub(super) fn clear() {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = lightcraft_gpu::take_frame_note();
    }

    /// What the render just made on this thread says, and the kept images of the render it started from, when `raw` (its
    /// readback) has that render's size and `display` is the one they were converted for.
    pub(super) fn after_render(stages: Option<&lightcraft_pipeline::StageCache>, raw: &Rgba8, display: Option<u64>) -> Patch {
        #[cfg(not(target_arch = "wasm32"))]
        let note = lightcraft_gpu::take_frame_note();
        #[cfg(target_arch = "wasm32")]
        let note: Option<FrameNote> = None;
        let from = note.as_ref().and_then(|n| n.changed.as_ref()).and_then(|(from, _)| kept_of(stages, *from));
        let from = from.filter(|k| k.display == display && k.size == (raw.width, raw.height));
        let rects = from.as_ref().and(note.as_ref()).and_then(|n| n.changed.as_ref()).map(|(_, r)| r.clone());
        Patch { note, from, rects }
    }

    /// `raw` converted for `display` (in place) and, with `full_srgb`, its sRGB twin, from the kept images where nothing
    /// changed: what `display::finish_render` makes of the whole image. `None` when there is nothing to start from.
    pub(super) fn convert(&self, raw: &mut Rgba8, display: &crate::display::Display, full_srgb: bool) -> Option<(Option<Rgba8>, Option<Histogram>)> {
        let (from, rects) = (self.from.as_ref()?, self.rects.as_ref()?);
        if full_srgb && from.srgb.is_none() {
            return None;
        }
        let warn = |e: String| log::warn!("display profile: {e}");
        let mut converted = (**from.converted.as_ref()?).clone();
        let mut srgb = if full_srgb { from.srgb.as_ref().map(|s| (**s).clone()) } else { None };
        for r in rects {
            let part = cut(raw, r);
            if let Some(s) = srgb.as_mut() {
                paste(s, r, &display.profile.to_srgb(&part).map_err(warn).ok()?);
            }
            let mut c = part;
            display.profile.correct(&mut c).map_err(warn).ok()?;
            paste(&mut converted, r, &c);
        }
        let histogram = match &srgb {
            Some(s) => Some(Histogram::of_srgb8(s)),
            None => display.profile.to_srgb(&crate::display::histogram_sample(raw)).map_err(warn).ok().map(|s| Histogram::of_srgb8(&s)),
        };
        *raw = converted;
        Some((srgb, histogram))
    }

    /// `img` fitted to `max` as `fit_for_display` does, from the kept fitted image where nothing changed; with the
    /// rectangles of the fitted image that changed. `None` when there is nothing to start from.
    pub(super) fn fit(&self, img: &Rgba8, max: (usize, usize)) -> Option<(Rgba8, Vec<Rect>)> {
        let (from, rects) = (self.from.as_ref()?, self.rects.as_ref()?);
        let fitted = from.fit.as_ref().filter(|_| !from.device_fit)?;
        let size = fit_size(img.width, img.height, max.0.max(1), max.1.max(1));
        if (fitted.width, fitted.height) != size {
            return None;
        }
        if fit_whole(rects, (img.width, img.height)) {
            let out = crate::media::fit_for_display(img, max.0, max.1);
            let changed = fitted_rects((img.width, img.height), (out.width, out.height), rects);
            return Some((out, changed));
        }
        let mut out = (**fitted).clone();
        let changed = fit_rects(img, &mut out, rects)?;
        Some((out, changed))
    }

    /// Whether this job keeps what it makes for the next one ([`Self::finish`]): the render is numbered and the view's
    /// stages hold it. Only then is a copy of the picture worth making.
    pub(super) fn keeps(&self, stages: Option<&lightcraft_pipeline::StageCache>) -> bool {
        self.note.is_some() && stages.is_some()
    }

    /// Keep what this job made of the render (`made`, when [`Self::keeps`]: the images the next job starts from) for
    /// the next job, and say which picture the result is: `rects` (in the result's pixels) where it differs from the
    /// last one. `fitted`: whether the picture was fitted (`Some(true)` on the device), `size`: the render's size.
    pub(super) fn finish(
        self,
        stages: Option<&lightcraft_pipeline::StageCache>,
        (display, size, fitted): (Option<u64>, (usize, usize), Option<bool>),
        made: Option<Made>,
        rects: Option<Vec<Rect>>,
    ) -> Option<Changed> {
        let note = self.note?;
        let device_fit = fitted == Some(true);
        // (a picture fitted another way than the last one differs from it in rounding everywhere)
        let same_fit = self.from.as_ref().is_some_and(|k| k.fitted == fitted.is_some() && k.device_fit == device_fit);
        let rects = rects.filter(|_| same_fit);
        if let (Some(st), Some(m)) = (stages, made) {
            // (the next job converts from it only when it converts, and fits on the host from a host fit only)
            let converted = m.converted.filter(|_| display.is_some());
            let kept = Kept { display, size, srgb: m.srgb, converted, fitted: fitted.is_some(), fit: m.fit.filter(|_| !device_fit), device_fit };
            keep(st, note.frame, kept);
        }
        let from = note.changed.as_ref().map(|(from, _)| *from).filter(|_| rects.is_some());
        Some(Changed { frame: note.frame, from, rects: rects.unwrap_or_default() })
    }

    /// The render's changed rectangles when the job started from the kept images.
    pub(super) fn rects(&self) -> Option<&[Rect]> {
        self.rects.as_deref()
    }
}

/// The pixels of `img` in `r`, as an image.
fn cut(img: &Rgba8, r: &Rect) -> Rgba8 {
    let (x0, x1) = (r[0].min(img.width), r[2].min(img.width));
    let (y0, y1) = (r[1].min(img.height), r[3].min(img.height));
    let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        data.extend_from_slice(img.data.get(y * img.width + x0..y * img.width + x1).unwrap_or_default());
    }
    Rgba8 { width: x1 - x0, height: y1 - y0, data }
}

/// `part` (cut from `r`) put back into `img`.
fn paste(img: &mut Rgba8, r: &Rect, part: &Rgba8) {
    let w = img.width;
    for row in 0..part.height {
        let y = r[1] + row;
        let (d, s) = (img.data.get_mut(y * w + r[0]..y * w + r[0] + part.width), part.data.get(row * part.width..(row + 1) * part.width));
        if let (Some(d), Some(s)) = (d, s) {
            d.copy_from_slice(s);
        }
    }
}

/// Whether a fit after a change of `rects` of a `w × h` picture is the whole fit: when they cover more than half of it,
/// `fit_for_display` (on every core) makes the same bits sooner than [`fit_rects`] (on one).
fn fit_whole(rects: &[Rect], (w, h): (usize, usize)) -> bool {
    let area = rects.iter().map(|r| r[2].saturating_sub(r[0]).saturating_mul(r[3].saturating_sub(r[1]))).fold(0usize, usize::saturating_add);
    area.saturating_mul(2) > w.saturating_mul(h)
}

/// The output indices `[a, b)` whose taps read a source index in `[lo, hi)`.
fn outputs_reading(w: &[(usize, Vec<f32>)], lo: usize, hi: usize) -> (usize, usize) {
    let reads = |(first, ws): &(usize, Vec<f32>)| *first < hi && first + ws.len() > lo;
    let a = w.iter().position(reads).unwrap_or(w.len());
    let b = w.iter().rposition(reads).map_or(a, |i| i + 1);
    (a, b.max(a))
}

/// The rectangles of the fit of `src` into `out` (`fit_for_display`'s Lanczos fit, already its size) that read the
/// source rectangles `changed`, made again in `out` with the same taps in the same order as the whole fit; returned.
/// `None` when the whole fit would run its passes the other way round (a far wider than tall change of shape).
pub(super) fn fit_rects(src: &Rgba8, out: &mut Rgba8, changed: &[Rect]) -> Option<Vec<Rect>> {
    let (sw, sh, w, h) = (src.width, src.height, out.width, out.height);
    // (`resample::resize` goes down first when the across-first intermediate would be far larger)
    if w.saturating_mul(sh) > sw.saturating_mul(h).saturating_mul(4) {
        return None;
    }
    let (wx, wy) = (weights(sw, w, Filter::Lanczos3), weights(sh, h, Filter::Lanczos3));
    let px = |x: usize, y: usize| src.data.get(y * sw + x).map_or([0.0; 4], |p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2]), f32::from(p[3])]);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    let mut made = Vec::new();
    for r in changed {
        let (x0, x1) = outputs_reading(&wx, r[0], r[2]);
        let (y0, y1) = outputs_reading(&wy, r[1], r[3]);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        // the across pass of the source rows the down pass reads, for these columns
        let rows = wy.get(y0..y1).unwrap_or_default();
        let (t0, t1) = (rows.iter().map(|t| t.0).min().unwrap_or(0), rows.iter().map(|t| t.0 + t.1.len()).max().unwrap_or(0));
        let cols = x1 - x0;
        let mut tmp = vec![[0.0f32; 4]; cols * t1.saturating_sub(t0)];
        for (ty, row) in (t0..t1).zip(tmp.chunks_exact_mut(cols)) {
            for (o, (lo, ws)) in row.iter_mut().zip(wx.get(x0..x1).unwrap_or_default()) {
                let mut acc = <[f32; 4]>::zero();
                for (k, wt) in ws.iter().enumerate() {
                    acc = acc.madd(px(lo + k, ty), *wt);
                }
                *o = acc;
            }
        }
        for (y, (lo, ws)) in (y0..y1).zip(rows) {
            let mut row = vec![[0.0f32; 4]; cols];
            for (k, wt) in ws.iter().enumerate() {
                let Some(src_row) = tmp.get((lo + k - t0) * cols..(lo + k - t0 + 1) * cols) else { continue };
                for (o, s) in row.iter_mut().zip(src_row) {
                    *o = o.madd(*s, *wt);
                }
            }
            if let Some(dst) = out.data.get_mut(y * w + x0..y * w + x1) {
                for (d, p) in dst.iter_mut().zip(&row) {
                    *d = [c(p[0]), c(p[1]), c(p[2]), c(p[3])];
                }
            }
        }
        made.push([x0, y0, x1, y1]);
    }
    Some(made)
}

/// The rectangles of a fit of a `sw × sh` image into `w × h` (as [`fit_rects`] makes them) that read `changed`.
pub(super) fn fitted_rects((sw, sh): (usize, usize), (w, h): (usize, usize), changed: &[Rect]) -> Vec<Rect> {
    let (wx, wy) = (weights(sw, w, Filter::Lanczos3), weights(sh, h, Filter::Lanczos3));
    changed
        .iter()
        .filter_map(|r| {
            let (x0, x1) = outputs_reading(&wx, r[0], r[2]);
            let (y0, y1) = outputs_reading(&wy, r[1], r[3]);
            (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
        })
        .collect()
}

#[cfg(test)]
#[path = "dirty_tests.rs"]
mod tests;
