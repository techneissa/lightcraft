//! The loupe's finished full-size picture (issue #178): the photo with every edit applied, at its own size, which
//! zoom and pan draw from without rendering anything.
//!
//! Two copies of the photo: the edit is what renders while the photo is being edited (the whole-frame render and
//! the zoom window, [`crate::region`], drafts during a drag); the finished picture is made once the edit has settled,
//! and is what every zoom past Fit and every pan shows from then on.
//!
//! - **When.** Only at rest: no slider drag, no brush stroke or pointer held, no pinch, no photo being stepped
//!   through, and the loupe's own renders done (the screen is sharp first). A look already on disk is read at once.
//!   A render is made only for a photo zoomed into or panned since it was opened (#199): only zoom and pan draw from
//!   the picture, so photos stepped past while culling (at Fit, or at a zoom kept from the photo before) or edited at
//!   Fit render none. It waits [`SETTLE_SECS`] at rest after the zoom or the last edit, so a quick series of edits
//!   renders none. It runs on a render worker at the
//!   lowest priority ([`PRIORITY`]); the next edit drops it when it is still queued and throws its result away when
//!   it is running.
//! - **What.** The whole frame at 100 %, in one render (bands cost about twice as much, [`build::render`]). An edit,
//!   another photo or the closed loupe stops a build that has not started its render, and a build that was stopped
//!   cuts and writes nothing. One build runs at a time. On the worker the
//!   picture is cut into textures of at most [`TILE`] px (one upload per frame), with smaller levels about 1.41×
//!   apart made with an area filter ([`build::levels`]), so a view from Fit to 100 % is shrunk by the GPU by less
//!   than 1.41× (no aliasing). Each texture carries one pixel of its neighbours, so magnified views blend across its
//!   edges. It holds a row in the activity stack while it renders ("Preparing the full-size view"; ✕ stops it).
//! - **Never the old edit.** The picture is drawn only while its key is the photo's key now ([`picture_job`]: the
//!   settings, the file, the size, crop, profiles, denoise, proof, hidden wheels and the display profile), and it is
//!   freed the moment the key changes. Until the new one is there, zoom and pan work as before it.
//! - **Disk.** The finished picture is kept in the preview cache folder ([`lightcraft_preview::DiskCache::put_full`],
//!   lossless) under the loupe's view key and the render's key, so going back to a photo reads it instead of
//!   rendering. Its pixels are the display's (the render is made for the display, wide gamut included); another
//!   display has another key.
//! - **Memory.** One picture, of the photo in the loupe, at most the stage budget ([`crate::render::Renderer::
//!   stage_budget`]); freed when another photo opens, when the loupe has been closed a while, or when the library or
//!   the display changes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Rect, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::RenderJob;
use lightcraft_engine::media::QuickSource;
use lightcraft_preview::Hasher128;

use crate::LightcraftApp;
use crate::render::Slot;

mod build;
pub use build::{AtOnce, Built, Plan, TileImage, levels, work};

/// The largest texture of the picture (px a side, before its one-pixel border): about 16 MB, one upload per frame.
pub const TILE: usize = 2048;
/// The most a level is shrunk by the GPU: levels are 1.41× apart (and a little rounding).
const MAX_SHRINK: f32 = 1.42;
/// The activity row's text while a picture renders.
pub const ROW: &str = "Preparing the full-size view";
/// The pool priority of its job: below every other render, thumbnails and prefetches included.
pub const PRIORITY: u32 = 2;
/// How long the loupe stays at rest before the picture is rendered (s): a quick series of edits renders none.
pub const SETTLE_SECS: f64 = 1.0;
/// Changes when the picture's file or its key changes meaning.
const FORMAT: u64 = 1;

/// A texture of the picture on the GPU.
struct Tile {
    level: u8,
    rect: [usize; 4],
    /// The part of the texture that is `rect` (without the border).
    uv: Rect,
    tex: egui::TextureHandle,
    bytes: usize,
    pixels: Option<Arc<egui::ColorImage>>,
}

/// The finished picture of the photo in the loupe.
struct Picture {
    photo: PhotoId,
    key: u64,
    levels: Vec<[usize; 2]>,
    tiles: Vec<Tile>,
    /// Textures still to upload (one per frame, those on screen first).
    waiting: Vec<TileImage>,
}

/// The build running now (one at a time).
struct Running {
    photo: PhotoId,
    key: u64,
    /// A render (else a look on disk).
    render: bool,
    cancel: Arc<AtomicBool>,
    /// The store stopped it (another look, photo, the closed loupe), as opposed to the activity row's ✕.
    by_us: bool,
    /// Its row in the activity stack, while it renders.
    _row: Option<lightcraft_engine::activity::TaskGuard>,
}

/// The photo open in the loupe, the zoom and pan it opened with, and whether it has been zoomed into since: the zoom
/// or the pan changed while the loupe was past Fit. Only such a photo gets a picture rendered: zoom and pan are all
/// that draw from it, and a zoom kept from the photo before, while photos are stepped through, does not count.
struct Opened {
    photo: PhotoId,
    zoom: crate::state::Zoom,
    pan: (f32, f32),
    zoomed: bool,
}

/// What is known about the picture's making, for `ui.inspect` and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Pictures rendered (not read from disk).
    pub rendered: u64,
    /// Builds that came back after they were stopped, or for a closed loupe: nothing of them was kept.
    pub dropped: u64,
    /// Pictures let go to free memory.
    pub freed_for_memory: u64,
    /// Pictures read from disk.
    pub from_disk: u64,
    /// Looks on disk that found nothing.
    pub disk_misses: u64,
    /// Frames the loupe drew from the picture.
    pub drawn: u64,
}

/// The loupe's finished full-size picture and what is being made ([`crate::render::Renderer::full`]).
#[derive(Default)]
pub struct Store {
    /// Make pictures (off in the crate's tests unless a test turns it on; never on the web).
    pub enabled: bool,
    picture: Option<Picture>,
    /// The build running (or queued) now.
    running: Option<Running>,
    /// Keys with nothing on disk, and keys whose render gave no picture (not asked for again).
    disk_missed: Option<u64>,
    refused: Option<u64>,
    /// The key the loupe has been at rest with, and since when (the host's clock, s).
    rest: Option<(u64, f64)>,
    /// The photo open in the loupe and how it was zoomed when it opened ([`Opened`]).
    opened: Option<Opened>,
    /// The level and the part of the photo (fractions `[x0, y0, x1, y1]`) the loupe showed last: uploaded first.
    last_view: Option<(u8, [f32; 4])>,
    /// The loupe's last frame was drawn from the picture.
    pub on_screen: bool,
    /// Its builds running at once (never more than one).
    pub at_once: Arc<AtOnce>,
    /// Bytes of its pictures still being written to disk ([`Plan::writing`]).
    pub writing: Arc<std::sync::atomic::AtomicUsize>,
    /// Make the next build's cut fail (tests).
    #[cfg(test)]
    pub fail_next: bool,
    pub stats: Stats,
}

impl Store {
    /// On in the app (`LIGHTCRAFT_FULL_PICTURE=0` turns it off, to compare with the loupe without it), off in the
    /// crate's tests and on the web.
    pub fn new() -> Self {
        let off = std::env::var("LIGHTCRAFT_FULL_PICTURE").is_ok_and(|v| v.trim() == "0");
        Store { enabled: !off && !cfg!(test) && !cfg!(target_arch = "wasm32"), ..Default::default() }
    }

    /// Let go of the picture and stop the build running (it stops before its render, or its result is dropped).
    pub fn clear(&mut self) {
        self.picture = None;
        self.cancel();
        self.rest = None;
        self.last_view = None;
    }

    /// Stop the build running, if one is.
    pub fn cancel(&mut self) {
        if let Some(r) = self.running.as_mut() {
            r.by_us = true;
            r.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The build's job was dropped before it started: none is running.
    pub fn job_dropped(&mut self) {
        self.running = None;
    }

    /// Whether a build of photo `id` is rendering (it decodes the original if it isn't held).
    pub fn renders(&self, id: PhotoId) -> bool {
        self.running.as_ref().is_some_and(|r| r.render && r.photo == id)
    }

    /// Whether a picture is held or being made.
    pub fn holds_anything(&self) -> bool {
        self.picture.is_some() || self.running.is_some()
    }

    /// Let go of the picture to free memory (its disk copy stays). It is not asked for again until the look changes:
    /// memory that is short now would be short again.
    pub fn free_for_memory(&mut self) {
        if let Some(p) = self.picture.take() {
            self.refused = Some(p.key);
            self.stats.freed_for_memory += 1;
        }
    }

    /// The photo and key of the picture held, if one is.
    pub fn held(&self) -> Option<(PhotoId, u64)> {
        self.picture.as_ref().map(|p| (p.photo, p.key))
    }

    /// Whether a picture is held with all its textures up.
    pub fn ready(&self) -> bool {
        self.picture.as_ref().is_some_and(|p| p.waiting.is_empty())
    }

    /// Bytes of the textures uploaded and waiting.
    pub fn bytes(&self) -> usize {
        self.picture
            .as_ref()
            .map_or(0, |p| p.tiles.iter().map(|t| t.bytes).sum::<usize>() + p.waiting.iter().map(|t| t.image.pixels.len() * 4).sum::<usize>())
    }

    /// A build of key `key` for `photo` came back (`built`: its picture, if it made one). `keep`: it is the request
    /// now and the loupe is open; else, or when it was stopped, nothing of it is kept.
    pub fn finished(&mut self, photo: PhotoId, key: u64, built: Option<Built>, keep: bool) {
        let run = self.running.take();
        let stopped = run.as_ref().is_none_or(|r| r.key != key || r.photo != photo || r.cancel.load(Ordering::Relaxed));
        if !keep || stopped {
            // (the activity row's ✕ on this look: not built again until it changes)
            if let Some(r) = run.filter(|r| r.key == key && r.cancel.load(Ordering::Relaxed) && !r.by_us) {
                self.refused = Some(r.key);
            }
            if built.is_some() || stopped {
                self.stats.dropped += 1;
            }
            return;
        }
        let Some(built) = built else {
            match run {
                Some(r) if !r.render => {
                    self.stats.disk_misses += 1;
                    self.disk_missed = Some(key);
                }
                _ => self.refused = Some(key),
            }
            return;
        };
        if built.from_disk {
            self.stats.from_disk += 1;
        } else {
            self.stats.rendered += 1;
        }
        self.picture = Some(Picture { photo, key, levels: built.levels, tiles: Vec::new(), waiting: built.tiles });
    }

    /// Upload one waiting texture (the part on screen first). Returns whether more wait.
    pub fn upload_some(&mut self, ctx: &egui::Context, keep_pixels: bool) -> bool {
        let Some(p) = self.picture.as_mut() else { return false };
        let wanted = |t: &TileImage| match self.last_view {
            Some((level, view)) => (t.level != level, !overlaps(t, &p.levels, view)),
            None => (t.level != 0, false),
        };
        let Some(i) = p.waiting.iter().enumerate().min_by_key(|(_, t)| wanted(t)).map(|(i, _)| i) else { return false };
        let t = p.waiting.swap_remove(i);
        let [w, h] = t.image.size;
        let uv = Rect::from_min_max(
            pos2(t.border[0] as f32 / w as f32, t.border[1] as f32 / h as f32),
            pos2((t.border[0] + t.rect[2]) as f32 / w as f32, (t.border[1] + t.rect[3]) as f32 / h as f32),
        );
        let tex = ctx.load_texture(format!("Full{}:{}:{}", t.level, t.rect[0], t.rect[1]), t.image.clone(), egui::TextureOptions::LINEAR);
        let pixels = keep_pixels.then(|| t.image.clone());
        p.tiles.push(Tile { level: t.level, rect: t.rect, uv, tex, bytes: w * h * 4, pixels });
        !p.waiting.is_empty()
    }

    /// What `ui.inspect` → `memory.fullPicture` says: the picture held and how pictures were made.
    pub fn to_json(&self) -> serde_json::Value {
        let p = self.picture.as_ref();
        serde_json::json!({
            "photo": p.map(|p| p.photo.0),
            "size": p.and_then(|p| p.levels.first()),
            "textures": p.map_or(0, |p| p.tiles.len()),
            "waiting": p.map_or(0, |p| p.waiting.len()),
            "bytes": self.bytes(),
            "running": self.running.as_ref().map(|r| if r.render { "render" } else { "disk" }),
            "rendered": self.stats.rendered,
            "fromDisk": self.stats.from_disk,
            "diskMisses": self.stats.disk_misses,
            "framesDrawn": self.stats.drawn,
            "dropped": self.stats.dropped,
            "freedForMemory": self.stats.freed_for_memory,
        })
    }

    /// CPU copies of the textures (headless screenshots), by texture id.
    pub fn cpu_textures(&self) -> Vec<(egui::TextureId, Arc<egui::ColorImage>)> {
        self.picture.iter().flat_map(|p| p.tiles.iter()).filter_map(|t| Some((t.tex.id(), t.pixels.clone()?))).collect()
    }

    /// The level to draw at `drawn` px: the smallest that is at least as big, else the full size (magnified).
    /// `None` when that level would be shrunk by more than [`MAX_SHRINK`] (a canvas smaller than when it was made).
    fn level_for(levels: &[[usize; 2]], drawn: f32) -> Option<u8> {
        let mut best = 0;
        for (i, &[w, h]) in levels.iter().enumerate() {
            if w.max(h) as f32 >= drawn * 0.999 {
                best = i;
            }
        }
        let long = levels.get(best).map_or(0, |&[w, h]| w.max(h)) as f32;
        (long <= drawn * MAX_SHRINK || best == 0 && long <= drawn).then_some(best as u8).or((long <= drawn).then_some(0))
    }

    /// The level to draw `photo` with `key` at `drawn_long` px, where `view` (fractions of the photo) is on screen:
    /// `Some` only when the picture is that look and every texture of the view is up.
    fn ready_for(&mut self, photo: PhotoId, key: u64, drawn_long: f32, view: [f32; 4]) -> Option<u8> {
        let p = self.picture.as_ref().filter(|p| p.photo == photo && p.key == key)?;
        let level = Self::level_for(&p.levels, drawn_long)?;
        self.last_view = Some((level, view));
        let size = *p.levels.get(level as usize)?;
        let covered = covered(p.tiles.iter().filter(|t| t.level == level).map(|t| t.rect), size, view);
        let waiting = p.waiting.iter().any(|t| t.level == level && overlaps(t, &p.levels, view));
        (covered && !waiting).then_some(level)
    }
}

/// Whether the textures `rects` of a `size` level cover `view` (fractions of it).
fn covered(rects: impl Iterator<Item = [usize; 4]>, size: [usize; 2], view: [f32; 4]) -> bool {
    let to = |f: f32, n: usize| (f.clamp(0.0, 1.0) * n as f32) as f64;
    let (vx0, vy0, vx1, vy1) = (to(view[0], size[0]), to(view[1], size[1]), to(view[2], size[0]), to(view[3], size[1]));
    let area: f64 = rects
        .map(|r| {
            let w = ((r[0] + r[2]) as f64).min(vx1) - (r[0] as f64).max(vx0);
            let h = ((r[1] + r[3]) as f64).min(vy1) - (r[1] as f64).max(vy0);
            w.max(0.0) * h.max(0.0)
        })
        .sum();
    // (textures never overlap: covered when their parts in the view add up to it)
    area + 0.5 >= (vx1 - vx0).max(0.0) * (vy1 - vy0).max(0.0)
}

/// Whether texture `t` shows part of `view` (fractions of the photo).
fn overlaps(t: &TileImage, levels: &[[usize; 2]], view: [f32; 4]) -> bool {
    let Some(&[w, h]) = levels.get(t.level as usize) else { return false };
    let r = t.rect;
    let (x0, y0) = (r[0] as f32 / w as f32, r[1] as f32 / h as f32);
    let (x1, y1) = ((r[0] + r[2]) as f32 / w as f32, (r[1] + r[3]) as f32 / h as f32);
    x0 < view[2] && x1 > view[0] && y0 < view[3] && y1 > view[1]
}

/// The render the picture of photo `id` is, as the loupe shows the photo now: the whole frame at `size` (its own
/// size), for the display in use, proofed and with the wheels hidden as the view is; no overlay. Its key is the
/// picture's. `None`: no such photo, or an HDR edit (the picture is 8-bit).
pub fn picture_job(app: &mut LightcraftApp, id: PhotoId, size: [usize; 2], with_crop: bool) -> Option<RenderJob> {
    let job = app.session.render_job(id, size[0], size[1], false, with_crop)?;
    if job.settings.hdr.enabled {
        return None;
    }
    let mut job = dress(app, id, job);
    job.key = Hasher128::new().str("full").u64(job.key).u64(app.renderer.display_id().unwrap_or(0)).u64(FORMAT).finish().0 as u64;
    Some(job)
}

/// `job` as the loupe shows the photo: for the display in use, proofed and with the wheels hidden as the view is.
fn dress(app: &LightcraftApp, id: PhotoId, job: RenderJob) -> RenderJob {
    let proof = app.ui.soft_proof.then_some(app.ui.proof);
    job.with_hidden_grading(app.ui.grading_hidden_for(id)).with_proof(proof).with_display(app.renderer.display().cloned())
}

/// What the loupe tells [`update`] about this frame.
pub struct Ask {
    pub id: PhotoId,
    pub with_crop: bool,
    /// One view of the photo, no overlay, tint, hover or proof-free diagnostic: what the picture can stand for.
    pub eligible: bool,
    /// Zoomed past what the whole-frame render holds (the picture is drawn only then).
    pub zoomed: bool,
    /// Something moves: a slider drag, a pointer held, a pinch, photos stepped through.
    pub busy: bool,
    /// The photo's own size (the picture's).
    pub native: [usize; 2],
    pub texture_side: usize,
    /// The photo as drawn, and the part of the canvas it is seen in.
    pub img_rect: Rect,
    pub visible: Rect,
    pub ppp: f32,
    /// The host's clock (s).
    pub now: f64,
}

/// Once per loupe frame: let go of a picture that is not the photo's look any more, ask for one when the loupe is at
/// rest, and say which level to draw (`Some`) when the picture covers the view.
pub fn update(app: &mut LightcraftApp, ctx: &egui::Context, a: &Ask) -> Option<u8> {
    app.renderer.full.on_screen = false;
    if !app.renderer.full.enabled || app.renderer.has_offload() {
        return None;
    }
    let store = &app.renderer.full;
    if store.held().is_some_and(|(photo, _)| photo != a.id) || store.running.as_ref().is_some_and(|r| r.photo != a.id) {
        app.renderer.cancel_full();
    }
    let (zoom, pan) = (app.ui.zoom, app.ui.pan);
    let store = &mut app.renderer.full;
    match store.opened.as_mut() {
        Some(o) if o.photo == a.id => o.zoomed |= a.zoomed && (o.zoom != zoom || o.pan != pan),
        _ => store.opened = Some(Opened { photo: a.id, zoom, pan, zoomed: false }),
    }
    if !a.eligible {
        app.renderer.full.rest = None;
        return None;
    }
    let job = picture_job(app, a.id, a.native, a.with_crop)?;
    let key = job.key;
    let store = &app.renderer.full;
    // another look: the old picture is never drawn again, and what was being made for it stops
    if store.held().is_some_and(|(_, k)| k != key) || store.running.as_ref().is_some_and(|r| r.key != key && !r.by_us) {
        app.renderer.cancel_full();
    }
    let drawn_long = a.img_rect.width().max(a.img_rect.height()) * a.ppp;
    let view = fractions(a.img_rect, a.visible);
    let level = if a.zoomed { app.renderer.full.ready_for(a.id, key, drawn_long, view) } else { None };
    if level.is_some() {
        app.renderer.full.stats.drawn += 1;
        app.renderer.full.on_screen = true;
    }
    ask(app, ctx, a, job, level.is_some());
    level
}

/// Ask for the picture when the loupe is at rest and none is held or coming: from disk at once, rendered after
/// [`SETTLE_SECS`].
fn ask(app: &mut LightcraftApp, ctx: &egui::Context, a: &Ask, job: RenderJob, drawn: bool) {
    let key = job.key;
    let loupe_busy = [Slot::Main, Slot::Region, Slot::RegionBefore, Slot::Alpha, Slot::RegionAlpha, Slot::Before, Slot::Hover, Slot::BrushFull]
        .iter()
        .any(|s| app.renderer.is_pending(*s));
    // (the loupe's own picture of the photo is up: rendered, or its view render kept from before)
    let sharp = app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == a.id && t.quick != Some(QuickSource::Embedded));
    let store = &mut app.renderer.full;
    if a.busy || loupe_busy || !sharp {
        store.rest = None;
        return;
    }
    // (one build at a time: a stopped one is waited for)
    if drawn || store.held().is_some() || store.running.is_some() || store.refused == Some(key) {
        return;
    }
    let since = match store.rest {
        Some((k, since)) if k == key => since,
        _ => {
            store.rest = Some((key, a.now));
            a.now
        }
    };
    // (the picture's textures within the stage budget; the CPU copies an agent's headless screenshots read are not
    // counted, as for every other texture)
    let fit = fit_long(a);
    let px: usize = levels(a.native[0], a.native[1], fit).iter().map(|[w, h]| w * h).sum();
    // (a full-size copy still being written to disk is memory too)
    let bytes = px.saturating_mul(4).saturating_add(app.renderer.full.writing.load(Ordering::SeqCst));
    if bytes > app.renderer.stage_budget() {
        // (once the copy on its way to disk is written, the build may fit)
        if app.renderer.full.writing.load(Ordering::SeqCst) > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        return;
    }
    let disk = app.session.view_cache_key(a.id, a.with_crop).and_then(|view| {
        let rendered = app.session.media.rendered.clone();
        rendered.disk()?;
        let k = Hasher128::new().u64(view.0 as u64).u64((view.0 >> 64) as u64).str("full").u64(key).u64(FORMAT).finish();
        Some((rendered, k))
    });
    let store = &mut app.renderer.full;
    let render = disk.is_none() || store.disk_missed == Some(key);
    // (a render only for a photo zoomed into: one stepped past or only seen whole renders nothing, and the settle time
    // counts from the zoom)
    if render && !store.opened.as_ref().is_some_and(|o| o.photo == a.id && o.zoomed) {
        store.rest = None;
        return;
    }
    if render && a.now - since < SETTLE_SECS {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((SETTLE_SECS - (a.now - since)).max(0.01)));
        return;
    }
    // the original is being read ahead (#182): the build waits for it rather than decoding it a second time, and then
    // renders from it
    if render && app.full_decode.coming_for(app.ui.right, a.id) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }
    let tile = TILE.min(a.texture_side.saturating_sub(2)).max(16);
    let cancel = Arc::new(AtomicBool::new(false));
    let row = render.then(|| app.session.activity.start_background("full-picture", ROW, lightcraft_engine::activity::Cancel::Flag(cancel.clone())));
    app.renderer.full.running = Some(Running { photo: a.id, key, render, cancel: cancel.clone(), by_us: false, _row: row });
    let store = &mut app.renderer.full;
    let (at_once, writing) = (store.at_once.clone(), store.writing.clone());
    #[cfg(test)]
    let fail_cut = std::mem::take(&mut store.fail_next);
    let plan = Plan {
        disk,
        size: a.native,
        tile,
        min_edge: fit,
        render,
        cancel,
        at_once,
        writing,
        #[cfg(test)]
        fail_cut,
    };
    app.renderer.request_full(job, plan);
}

/// The photo's long edge as drawn at Fit in the loupe's view (px): the smallest level kept is the first at least
/// this long.
fn fit_long(a: &Ask) -> usize {
    let [w, h] = a.native;
    let s = (a.visible.width() * a.ppp / w.max(1) as f32).min(a.visible.height() * a.ppp / h.max(1) as f32).min(1.0);
    let long = w.max(h) as f32 * s;
    if long.is_finite() { long.max(1.0) as usize } else { w.max(h) }
}

/// The part of `img` (the photo as drawn) inside `visible`, as fractions `[x0, y0, x1, y1]` of the photo.
fn fractions(img: Rect, visible: Rect) -> [f32; 4] {
    let (w, h) = (img.width().max(1e-3), img.height().max(1e-3));
    let v = visible.intersect(img);
    [(v.min.x - img.min.x) / w, (v.min.y - img.min.y) / h, (v.max.x - img.min.x) / w, (v.max.y - img.min.y) / h]
}

/// Draw the picture's `level` in `img` (the photo as drawn), clipped by the painter.
pub fn draw(app: &LightcraftApp, p: &egui::Painter, img: Rect, level: u8) {
    let Some(pic) = app.renderer.full.picture.as_ref() else { return };
    let Some(&[w, h]) = pic.levels.get(level as usize) else { return };
    let (sx, sy) = (img.width() / w.max(1) as f32, img.height() / h.max(1) as f32);
    let clip = p.clip_rect();
    for t in pic.tiles.iter().filter(|t| t.level == level) {
        let r = t.rect;
        let dst = Rect::from_min_size(img.min + vec2(r[0] as f32 * sx, r[1] as f32 * sy), vec2(r[2] as f32 * sx, r[3] as f32 * sy));
        if dst.intersects(clip) {
            p.image(t.tex.id(), dst, t.uv, Color32::WHITE);
        }
    }
}

#[cfg(test)]
mod tests;
