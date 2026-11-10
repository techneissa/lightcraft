//! The worker's side of the finished full-size picture ([`super`]): read it from disk, or render it band by band,
//! make its smaller levels and cut every level into textures.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, vec2};
use lightcraft_engine::RenderJob;
use lightcraft_engine::media::{RenderResult, SourceRef};
use lightcraft_preview::{Hash128, PreviewCache};
use lightcraft_raster::Rgba8;

/// One band of the frame to render: its job renders the band with `margin` rows of context above and below
/// (fewer at the frame's edges), and the band itself is rows `rows.0..rows.1` of the frame.
pub struct Band {
    pub job: RenderJob,
    pub rows: (usize, usize),
    /// The first row of the frame the job renders.
    pub top: usize,
}

/// What the worker is asked to do (made on the UI thread).
pub struct Plan {
    /// The file it is kept in (`None`: no library, nothing on disk).
    pub disk: Option<(Arc<PreviewCache>, Hash128)>,
    /// The picture's size as asked (width, height): what is read from disk is at most a pixel off it.
    pub size: [usize; 2],
    /// The largest texture.
    pub tile: usize,
    /// The smallest level kept is the first at least this long (the photo's long edge as drawn at Fit): smaller
    /// views are drawn from the whole-frame render.
    pub min_edge: usize,
    /// The bands to render, top to bottom, when it isn't on disk (empty: only look on disk). One band with no
    /// margin is the whole frame in one render.
    pub bands: Vec<Band>,
    /// Set when the picture is no longer wanted (an edit, another photo, the loupe closed, the activity row's ✕):
    /// the build stops at the next band and nothing is cut or written.
    pub cancel: Arc<AtomicBool>,
    /// The builds of this store running now, and the most that ever ran at once (never two).
    pub at_once: Arc<AtOnce>,
    /// Bytes of full-size pictures waiting to be written to disk (counted in the memory a new build may take).
    pub writing: Arc<std::sync::atomic::AtomicUsize>,
    /// Make the cut fail (tests: a build that panics).
    #[cfg(test)]
    pub fail_cut: bool,
}

/// Builds running at once ([`Plan::at_once`]).
#[derive(Default)]
pub struct AtOnce {
    pub now: std::sync::atomic::AtomicUsize,
    pub most: std::sync::atomic::AtomicUsize,
}

/// One texture of the picture as the worker makes it.
pub struct TileImage {
    /// 0: full size; each next level is about 1.41× smaller.
    pub level: u8,
    /// The part of the level it shows, `[x, y, w, h]` in the level's pixels.
    pub rect: [usize; 4],
    /// The neighbours' pixels it also holds: left, top, right, bottom.
    pub border: [usize; 4],
    pub image: Arc<egui::ColorImage>,
}

/// The picture, cut into textures on the worker.
pub struct Built {
    /// The size of each level (width, height), level 0 first.
    pub levels: Vec<[usize; 2]>,
    pub tiles: Vec<TileImage>,
    pub from_disk: bool,
}

/// The worker's part: the picture from disk, else rendered band by band, kept on disk and cut into textures. `job`
/// stands for the whole picture (its key and photo; the result of a picture read from disk or not made).
///
/// A panic inside is a failed build (the activity row ends, and the next look builds), not a dead worker.
pub fn work(job: RenderJob, plan: Plan) -> (RenderResult, Option<Built>) {
    let at_once = plan.at_once.clone();
    let now = at_once.now.fetch_add(1, Ordering::SeqCst) + 1;
    at_once.most.fetch_max(now, Ordering::SeqCst);
    let failed = job.clone();
    let out = lightcraft_engine::guard::catch("the full-size view", || make(job, plan)).unwrap_or_else(|e| (failed.failed(e), None));
    at_once.now.fetch_sub(1, Ordering::SeqCst);
    out
}

fn make(job: RenderJob, plan: Plan) -> (RenderResult, Option<Built>) {
    let [w, h] = plan.size;
    let cancelled = || plan.cancel.load(Ordering::Relaxed);
    if let Some((cache, key)) = &plan.disk
        && let Some(disk) = cache.disk()
        && let Some(img) = disk.get_full(*key, (w.saturating_add(1), h.saturating_add(1))).filter(|i| near(i, plan.size))
    {
        if cancelled() {
            return (job.failed("cancelled".into()), None);
        }
        return (job.failed("read from disk".into()), Some(cut(&img, &plan, true)));
    }
    if plan.bands.is_empty() {
        return (job.failed("not on disk".into()), None);
    }
    let (first, img) = render(&plan);
    // (the result stands for the whole picture: its request, key and photo, with the original the first band decoded)
    let first = match first {
        Some(mut r) => {
            r.request_id = job.request_id;
            r.key = job.key;
            r.photo = job.photo;
            r
        }
        None => job.failed("cancelled".into()),
    };
    // (an edit or another photo while it rendered: nothing of it is cut or kept)
    let Some(img) = img.filter(|_| !cancelled()) else { return (first, None) };
    let built = cut(&img, &plan, false);
    if cancelled() {
        return (first, None);
    }
    if let Some((cache, key)) = plan.disk.clone() {
        keep_on_disk(cache, key, img, plan.writing.clone());
    }
    (first, Some(built))
}

fn near(img: &Rgba8, size: [usize; 2]) -> bool {
    img.width.abs_diff(size[0]) <= 1 && img.height.abs_diff(size[1]) <= 1
}

/// Render the bands into one picture, stopping at the first band after a cancel. Returns the first band's result
/// (it carries the original it decoded, for the session's cache) and the picture, if every band made its part.
pub(super) fn render(plan: &Plan) -> (Option<RenderResult>, Option<Rgba8>) {
    let [w, h] = plan.size;
    let mut first: Option<RenderResult> = None;
    let mut out: Option<Rgba8> = None;
    let mut decoded: Option<SourceRef> = None;
    for band in &plan.bands {
        if plan.cancel.load(Ordering::Relaxed) {
            return (first, None);
        }
        let mut job = band.job.clone();
        // (the original the first band decoded serves the others: decoded once)
        if let Some(src) = &decoded {
            job.source = src.clone();
        }
        // A band that reads the file first waits its turn at the memory gate, like other background work, and gives
        // up there once the build is stopped: AI Denoise holds the gate for its whole run, and a build stopped by an
        // edit must not wait for it. Past the gate the load takes its own place without queueing again.
        if !wait_at_gate(&job, &plan.cancel) {
            return (first, None);
        }
        let mut r = job.run();
        if decoded.is_none()
            && let Some(src) = &r.loaded
        {
            decoded = Some(SourceRef::Loaded(Box::new(src.clone())));
        }
        let part = match &mut r.rendered {
            Ok(x) if x.deep.is_none() => std::mem::replace(&mut x.image, Rgba8 { width: 0, height: 0, data: Vec::new() }),
            _ => return (first.or(Some(r)), None),
        };
        // one band, the whole frame: the picture is the render
        if plan.bands.len() == 1 && band.rows == (0, h) && band.top == 0 && near(&part, plan.size) {
            return (first.or(Some(r)), Some(part));
        }
        let skip = band.rows.0.saturating_sub(band.top);
        let rows = band.rows.1.saturating_sub(band.rows.0);
        if part.width != w || part.height < skip + rows {
            return (first.or(Some(r)), None);
        }
        let img = out.get_or_insert_with(|| Rgba8 { width: w, height: h, data: vec![[0; 4]; w * h] });
        let src = part.data.get(skip * w..(skip + rows) * w).unwrap_or_default();
        if let Some(dst) = img.data.get_mut(band.rows.0 * w..band.rows.1 * w)
            && dst.len() == src.len()
        {
            dst.copy_from_slice(src);
        } else {
            return (first.or(Some(r)), None);
        }
        if first.is_none() {
            first = Some(r);
        }
    }
    (first, out)
}

/// What a file load weighs at the memory gate per byte of the file (as the file loader weighs a full-size load).
const GATE_WEIGHT: usize = 6;

/// Wait at the memory gate for `job`'s file load, when it reads a file; `false` when `cancel` was set meanwhile.
fn wait_at_gate(job: &RenderJob, cancel: &AtomicBool) -> bool {
    let SourceRef::File { path, .. } = &job.source else { return true };
    let len = std::fs::metadata(path).map(|m| usize::try_from(m.len()).unwrap_or(usize::MAX)).unwrap_or(0);
    // (held only for the wait: the load takes its own place, as interactive work, once it is this one's turn)
    lightcraft_engine::memory::work_gate().acquire_unless(len.saturating_mul(GATE_WEIGHT), || cancel.load(Ordering::Relaxed)).is_some()
}

/// Write the picture to the disk cache off the worker: the textures go to the screen first. Its bytes count in
/// `writing` until written.
#[cfg(not(target_arch = "wasm32"))]
fn keep_on_disk(cache: Arc<PreviewCache>, key: Hash128, img: Rgba8, writing: Arc<std::sync::atomic::AtomicUsize>) {
    let bytes = img.data.len().saturating_mul(4);
    writing.fetch_add(bytes, Ordering::SeqCst);
    let counted = writing.clone();
    let spawned = std::thread::Builder::new().name("lc-full-write".into()).spawn(move || {
        if let Some(disk) = cache.disk() {
            disk.put_full(key, &img);
        }
        drop(img);
        counted.fetch_sub(bytes, Ordering::SeqCst);
    });
    if spawned.is_err() {
        writing.fetch_sub(bytes, Ordering::SeqCst);
        log::warn!("full-size picture: could not start its disk write");
    }
}

#[cfg(target_arch = "wasm32")]
fn keep_on_disk(_: Arc<PreviewCache>, _: Hash128, _: Rgba8, _: Arc<std::sync::atomic::AtomicUsize>) {}

/// The sizes of the picture's levels: the full size, then each about 1.41× smaller than the one before, down to the
/// first that is at least `min_edge` long. A view between two levels is drawn from the larger one, shrunk by the
/// GPU by less than 1.41× (no aliasing); the levels are made with an area filter.
pub fn levels(w: usize, h: usize, min_edge: usize) -> Vec<[usize; 2]> {
    let mut out = vec![[w, h]];
    for k in 1..16 {
        let s = std::f64::consts::SQRT_2.powi(k);
        let next = [((w as f64 / s).round() as usize).max(1), ((h as f64 / s).round() as usize).max(1)];
        if next[0].max(next[1]) < min_edge.max(256) {
            break;
        }
        out.push(next);
    }
    out
}

/// For each of `dst` pixels along an axis of `src`: the first source pixel it covers and the weight of each it
/// covers (the share of the source pixel inside it, summing to 1).
fn area_weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let s = src as f64 / dst.max(1) as f64;
    (0..dst)
        .map(|i| {
            let (a, b) = (i as f64 * s, ((i + 1) as f64 * s).min(src as f64));
            let (j0, j1) = (a.floor() as usize, (b.ceil() as usize).min(src));
            let span = (b - a).max(1e-9);
            let w = (j0..j1).map(|j| ((b.min(j as f64 + 1.0) - a.max(j as f64)).max(0.0) / span) as f32).collect();
            (j0, w)
        })
        .collect()
}

/// `img` shrunk to `w × h` with an area filter (each output pixel the mean of the input it covers), row by row
/// on all cores: no full-size float copy.
pub fn shrink(img: &Rgba8, w: usize, h: usize) -> Rgba8 {
    use rayon::prelude::*;
    let (sw, sh) = (img.width, img.height);
    let (xs, ys) = (area_weights(sw, w), area_weights(sh, h));
    let rows: Vec<Vec<[u8; 4]>> = ys
        .par_iter()
        .map(|(y0, wy)| {
            let mut acc = vec![[0f32; 4]; sw];
            for (dy, wt) in wy.iter().enumerate() {
                let Some(row) = img.data.get((y0 + dy) * sw..(y0 + dy + 1) * sw) else { continue };
                for (a, p) in acc.iter_mut().zip(row) {
                    for c in 0..4 {
                        a[c] += wt * f32::from(p[c]);
                    }
                }
            }
            xs.iter()
                .map(|(x0, wx)| {
                    let mut px = [0f32; 4];
                    for (dx, wt) in wx.iter().enumerate() {
                        let Some(a) = acc.get(x0 + dx) else { continue };
                        for c in 0..4 {
                            px[c] += wt * a[c];
                        }
                    }
                    px.map(|v| v.round().clamp(0.0, 255.0) as u8)
                })
                .collect()
        })
        .collect();
    Rgba8 { width: w, height: h, data: rows.into_iter().flatten().collect() }
}

/// Cut `img` and its smaller levels into textures of at most `plan.tile` px a side, each with a pixel of its
/// neighbours. Stops (fewer levels) when the build is cancelled meanwhile.
pub fn cut(img: &Rgba8, plan: &Plan, from_disk: bool) -> Built {
    #[cfg(test)]
    #[allow(clippy::panic)]
    if plan.fail_cut {
        panic!("a forced failure in the cut");
    }
    let tile = plan.tile.max(16);
    let sizes = levels(img.width, img.height, plan.min_edge);
    let mut tiles = Vec::new();
    let mut previous: Option<Rgba8> = None;
    for (level, &[lw, lh]) in sizes.iter().enumerate() {
        if plan.cancel.load(Ordering::Relaxed) {
            break;
        }
        let made = match level {
            0 => None,
            _ => Some(shrink(previous.as_ref().unwrap_or(img), lw, lh)),
        };
        let src = made.as_ref().unwrap_or(img);
        let (w, h) = (src.width, src.height);
        for y in (0..h).step_by(tile) {
            for x in (0..w).step_by(tile) {
                let (tw, th) = (tile.min(w - x), tile.min(h - y));
                let border = [usize::from(x > 0), usize::from(y > 0), usize::from(x + tw < w), usize::from(y + th < h)];
                let (x0, y0) = (x - border[0], y - border[1]);
                let (x1, y1) = (x + tw + border[2], y + th + border[3]);
                let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0));
                for row in y0..y1 {
                    let line = src.data.get(row * w + x0..row * w + x1).unwrap_or_default();
                    pixels.extend(line.iter().map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])));
                }
                let size = [x1 - x0, y1 - y0];
                let image = egui::ColorImage { size, source_size: vec2(size[0] as f32, size[1] as f32), pixels };
                tiles.push(TileImage { level: level as u8, rect: [x, y, tw, th], border, image: Arc::new(image) });
            }
        }
        if made.is_some() {
            previous = made;
        }
    }
    Built { levels: sizes, tiles, from_disk }
}
