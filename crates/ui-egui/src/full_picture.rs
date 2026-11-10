//! The loupe's finished full-size picture (issue #178): the photo with every edit applied, at its own size, which
//! zoom and pan draw from without rendering anything.
//!
//! Two copies of the photo: the edit is what renders while the photo is being edited (the whole-frame render and
//! the zoom window, [`crate::region`], drafts during a drag); the finished picture is made once the edit has settled,
//! and is what every zoom past Fit and every pan shows from then on.
//!
//! - **When.** Only at rest: no slider drag, no brush stroke or pointer held, no pinch, no photo being stepped
//!   through, and the loupe's own renders done (the screen is sharp first). A look already on disk is read at once;
//!   a render waits [`SETTLE_SECS`] more, so a quick series of edits renders none. It runs on a render worker at
//!   the lowest priority ([`PRIORITY`]); the next edit drops it when it is still queued and throws its result away
//!   when it is running.
//! - **What.** One render of the whole frame at 100 %, like an export: no window, so no seams. On the worker it is
//!   cut into textures of at most [`TILE`] px (the GPU's limit, and one upload per frame), with a half and a quarter
//!   size level ([`levels`]) so a view at 30–100 % is never minified by more than 2×. Each texture carries one
//!   pixel of its neighbours, so magnified views blend across its edges.
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

use egui::{Color32, Rect, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::RenderJob;
use lightcraft_engine::media::{QuickSource, RenderResult};
use lightcraft_preview::{Hash128, Hasher128, PreviewCache};
use lightcraft_raster::Rgba8;

use crate::LightcraftApp;
use crate::render::Slot;

/// The largest texture of the picture (px a side, before its one-pixel border): about 16 MB, one upload per frame.
pub const TILE: usize = 2048;
/// A smaller level is made while it is at least this long: views that need less are drawn from the whole-frame
/// render (Fit) anyway.
const LEVEL_MIN_EDGE: usize = 2048;
/// The pool priority of its job: below every other render, thumbnails and prefetches included.
pub const PRIORITY: u32 = 2;
/// How long the loupe stays at rest before the picture is rendered (s): a quick series of edits renders none.
pub const SETTLE_SECS: f64 = 1.0;
/// Changes when the picture's file or its key changes meaning.
const FORMAT: u64 = 1;

/// What the worker is asked to do (made on the UI thread).
#[derive(Clone)]
pub struct Plan {
    /// The file it is kept in (`None`: no library, nothing on disk).
    pub disk: Option<(Arc<PreviewCache>, Hash128)>,
    /// The picture's size as asked (width, height): what is read from disk is at most a pixel off it.
    pub size: [usize; 2],
    /// The largest texture.
    pub tile: usize,
    /// Render it when it isn't on disk (`false`: only look on disk).
    pub render: bool,
}

/// One texture of the picture as the worker makes it.
pub struct TileImage {
    /// 0: full size; 1: half; 2: a quarter…
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

/// The worker's part: the picture from disk, else (`plan.render`) rendered, kept on disk and cut into textures.
pub fn work(job: RenderJob, plan: &Plan) -> (RenderResult, Option<Built>) {
    let max = (plan.size[0].saturating_add(1), plan.size[1].saturating_add(1));
    let near = |w: usize, h: usize| w.abs_diff(plan.size[0]) <= 1 && h.abs_diff(plan.size[1]) <= 1;
    if let Some((cache, key)) = &plan.disk
        && let Some(disk) = cache.disk()
        && let Some(img) = disk.get_full(*key, max).filter(|img| near(img.width, img.height))
    {
        return (job.failed("read from disk".into()), Some(cut(&img, plan.tile, true)));
    }
    if !plan.render {
        return (job.failed("not on disk".into()), None);
    }
    let mut r = lightcraft_engine::memory::in_background(|| job.run());
    let built = match &mut r.rendered {
        Ok(x) if x.deep.is_none() && near(x.image.width, x.image.height) => {
            let img = std::mem::replace(&mut x.image, Rgba8 { width: 0, height: 0, data: Vec::new() });
            let built = cut(&img, plan.tile, false);
            if let Some((cache, key)) = plan.disk.clone() {
                keep_on_disk(cache, key, img);
            }
            Some(built)
        }
        _ => None,
    };
    (r, built)
}

/// Write the picture to the disk cache off the worker: the textures go to the screen first.
#[cfg(not(target_arch = "wasm32"))]
fn keep_on_disk(cache: Arc<PreviewCache>, key: Hash128, img: Rgba8) {
    let spawned = std::thread::Builder::new().name("lc-full-write".into()).spawn(move || {
        if let Some(disk) = cache.disk() {
            disk.put_full(key, &img);
        }
    });
    if spawned.is_err() {
        log::warn!("full-size picture: could not start its disk write");
    }
}

#[cfg(target_arch = "wasm32")]
fn keep_on_disk(_: Arc<PreviewCache>, _: Hash128, _: Rgba8) {}

/// The sizes of the picture's levels: the full size, then each half of the one before while that is at least
/// [`LEVEL_MIN_EDGE`] long.
pub fn levels(w: usize, h: usize) -> Vec<[usize; 2]> {
    let mut out = vec![[w, h]];
    while let Some(&[lw, lh]) = out.last() {
        let next = [lw.div_ceil(2), lh.div_ceil(2)];
        if next[0].max(next[1]) < LEVEL_MIN_EDGE || out.len() >= 8 {
            break;
        }
        out.push(next);
    }
    out
}

/// Half of `img` (2 × 2 average; an odd last row or column is its own pair).
fn half(img: &Rgba8) -> Rgba8 {
    let (w, h) = (img.width, img.height);
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let px = |x: usize, y: usize| img.data.get(y.min(h - 1) * w + x.min(w - 1)).copied().unwrap_or([0; 4]);
    let mut data = Vec::with_capacity(nw * nh);
    for y in 0..nh {
        for x in 0..nw {
            let q = [px(2 * x, 2 * y), px(2 * x + 1, 2 * y), px(2 * x, 2 * y + 1), px(2 * x + 1, 2 * y + 1)];
            let c = |i: usize| ((q.iter().map(|p| u32::from(p[i])).sum::<u32>() + 2) / 4) as u8;
            data.push([c(0), c(1), c(2), c(3)]);
        }
    }
    Rgba8 { width: nw, height: nh, data }
}

/// Cut `img` and its smaller levels into textures of at most `tile` px a side, each with a pixel of its neighbours.
pub fn cut(img: &Rgba8, tile: usize, from_disk: bool) -> Built {
    let tile = tile.max(16);
    let sizes = levels(img.width, img.height);
    let mut tiles = Vec::new();
    let mut smaller: Option<Rgba8> = None;
    for (level, _) in sizes.iter().enumerate() {
        let current = match (level, smaller.take()) {
            (0, _) => None,
            (_, Some(s)) => Some(s),
            (_, None) => break,
        };
        let src = current.as_ref().unwrap_or(img);
        if src.width == 0 || src.height == 0 {
            break;
        }
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
        if level + 1 < sizes.len() {
            smaller = Some(half(src));
        }
    }
    Built { levels: sizes, tiles, from_disk }
}

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

/// What is known about the picture's making, for `ui.inspect` and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Pictures rendered (not read from disk).
    pub rendered: u64,
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
    /// The photo and key asked for last, and whether it was a render (else a look on disk).
    asked: Option<(PhotoId, u64, bool)>,
    /// Keys with nothing on disk, and keys whose render gave no picture (not asked for again).
    disk_missed: Option<u64>,
    refused: Option<u64>,
    /// The key the loupe has been at rest with, and since when (the host's clock, s).
    rest: Option<(u64, f64)>,
    /// The level and the part of the photo (fractions `[x0, y0, x1, y1]`) the loupe showed last: uploaded first.
    last_view: Option<(u8, [f32; 4])>,
    /// The loupe's last frame was drawn from the picture.
    pub on_screen: bool,
    pub stats: Stats,
}

impl Store {
    /// On in the app (`LIGHTCRAFT_FULL_PICTURE=0` turns it off, to compare with the loupe without it), off in the
    /// crate's tests and on the web.
    pub fn new() -> Self {
        let off = std::env::var("LIGHTCRAFT_FULL_PICTURE").is_ok_and(|v| v.trim() == "0");
        Store { enabled: !off && !cfg!(test) && !cfg!(target_arch = "wasm32"), ..Default::default() }
    }

    /// Let go of the picture and of what is being asked for.
    pub fn clear(&mut self) {
        self.picture = None;
        self.asked = None;
        self.rest = None;
        self.last_view = None;
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

    /// A finished job of key `key` for `photo` (`built`: its picture, if it made one).
    pub fn accept(&mut self, photo: PhotoId, key: u64, built: Option<Built>) {
        let asked = self.asked.take();
        let Some(built) = built else {
            match asked {
                Some((_, k, false)) if k == key => {
                    self.stats.disk_misses += 1;
                    self.disk_missed = Some(key);
                }
                Some((_, k, true)) if k == key => self.refused = Some(key),
                _ => {}
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
            "asked": self.asked.map(|(_, _, render)| if render { "render" } else { "disk" }),
            "rendered": self.stats.rendered,
            "fromDisk": self.stats.from_disk,
            "diskMisses": self.stats.disk_misses,
            "framesDrawn": self.stats.drawn,
        })
    }

    /// CPU copies of the textures (headless screenshots), by texture id.
    pub fn cpu_textures(&self) -> Vec<(egui::TextureId, Arc<egui::ColorImage>)> {
        self.picture.iter().flat_map(|p| p.tiles.iter()).filter_map(|t| Some((t.tex.id(), t.pixels.clone()?))).collect()
    }

    /// The level to draw a picture of `long` px at `drawn` px: the smallest that is at least as big (minified by
    /// less than 2×), else the full size.
    fn level_for(levels: &[[usize; 2]], drawn: f32) -> u8 {
        let mut best = 0;
        for (i, &[w, h]) in levels.iter().enumerate() {
            if w.max(h) as f32 >= drawn {
                best = i;
            }
        }
        best as u8
    }

    /// The level to draw `photo` with `key` at `drawn_long` px, where `view` (fractions of the photo) is on screen:
    /// `Some` only when the picture is that look and every texture of the view is up.
    fn ready_for(&mut self, photo: PhotoId, key: u64, drawn_long: f32, view: [f32; 4]) -> Option<u8> {
        let p = self.picture.as_ref().filter(|p| p.photo == photo && p.key == key)?;
        let level = Self::level_for(&p.levels, drawn_long);
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
    let proof = app.ui.soft_proof.then_some(app.ui.proof);
    let mut job = job.with_hidden_grading(app.ui.grading_hidden_for(id)).with_proof(proof).with_display(app.renderer.display().cloned());
    job.key = Hasher128::new().str("full").u64(job.key).u64(app.renderer.display_id().unwrap_or(0)).u64(FORMAT).finish().0 as u64;
    Some(job)
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
    if store.held().is_some_and(|(photo, _)| photo != a.id) || store.asked.is_some_and(|(photo, ..)| photo != a.id) {
        app.renderer.full.clear();
        app.renderer.forget(Slot::Full);
    }
    if !a.eligible {
        app.renderer.full.rest = None;
        return None;
    }
    let job = picture_job(app, a.id, a.native, a.with_crop)?;
    let key = job.key;
    let store = &mut app.renderer.full;
    // another look: the old picture is never drawn again, and what was being made for it is dropped
    if store.held().is_some_and(|(_, k)| k != key) || store.asked.is_some_and(|(_, k, _)| k != key) {
        store.clear();
        app.renderer.forget(Slot::Full);
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
    if drawn || store.held().is_some() || store.asked.is_some() || store.refused == Some(key) {
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
    let px: usize = levels(a.native[0], a.native[1]).iter().map(|[w, h]| w * h).sum();
    let bytes = px.saturating_mul(4);
    if bytes > app.renderer.stage_budget() {
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
    if render && a.now - since < SETTLE_SECS {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((SETTLE_SECS - (a.now - since)).max(0.01)));
        return;
    }
    let tile = TILE.min(a.texture_side.saturating_sub(2)).max(16);
    store.asked = Some((a.id, key, render));
    let plan = Plan { disk, size: a.native, tile, render };
    app.renderer.request_full(job, plan);
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
