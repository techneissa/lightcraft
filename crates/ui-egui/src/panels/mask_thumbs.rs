//! The small coverage pictures of the Masking panel's list (fork issue #401): what each mask of the open photo selects,
//! white where selected, made on a worker from the pipeline's mask planes (as the overlay is). Each mask's picture is
//! kept until that mask's own shape or the photo's framing changes. Nothing here runs a model or decodes a file, and
//! the work follows a small proxy sampled from the source in memory, not the photo's size.

use std::sync::Arc;

use egui::{ColorImage, TextureHandle, TextureOptions};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskShape};
use lightcraft_engine::media::{DecodedSource, SourceRef};
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk};
use lightcraft_raster::Rgb32f;

use crate::LightcraftApp;
use crate::render::Slot;

/// The long edge of a thumbnail, in pixels (28 points at 2x).
const EDGE: usize = 64;
/// The long edge of the proxy the masks are evaluated over: a crop down to an eighth still fills a thumbnail.
const PROXY_EDGE: usize = 512;
/// Render sizes that lead to each level a decoded source can be held at: thumbnail (which is also cut from a held
/// preview or original), preview and full.
const ASKS: [usize; 3] = [EDGE, 640, 1 << 15];
const TASK: &str = "Mask Thumbnails";

/// What every thumbnail of a photo depends on.
#[derive(Clone, PartialEq)]
struct Framing {
    /// (the same id in another library is another photo)
    library: u64,
    photo: PhotoId,
    crop: lightcraft_develop::Crop,
    geometry: lightcraft_develop::Geometry,
    orientation: lightcraft_geom::Orientation,
}

struct Thumb {
    id: u32,
    /// The shape it was made from; `None`: made for another framing, so due again.
    shape: Option<Mask>,
    /// `None`: the row shows the mask's kind icon.
    texture: Option<TextureHandle>,
}

#[derive(Default)]
pub(crate) struct MaskThumbs {
    framing: Option<Framing>,
    made: Vec<Thumb>,
    /// The shapes the worker is on.
    pending: Option<Vec<Mask>>,
    /// The settings `made` was last found up to date with: while the photo keeps them, nothing is compared.
    current: Option<(PhotoId, Arc<DevelopSettings>)>,
}

impl MaskThumbs {
    /// The thumbnail of mask `id` of `photo` (the last one made, while a newer one is on its way).
    pub(crate) fn get(&self, photo: PhotoId, id: u32) -> Option<&TextureHandle> {
        self.framing.as_ref().filter(|f| f.photo == photo)?;
        self.made.iter().find(|t| t.id == id)?.texture.as_ref()
    }

    fn put(&mut self, shape: Mask, texture: Option<TextureHandle>) {
        let thumb = Thumb { id: shape.id, shape: Some(shape), texture };
        match self.made.iter_mut().find(|t| t.id == thumb.id) {
            Some(t) => *t = thumb,
            None => self.made.push(thumb),
        }
    }

    /// The worker's answer for the shapes it was given (one it made nothing for keeps the kind icon).
    fn finish(&mut self, ctx: &egui::Context, mut pictures: Vec<(u32, ColorImage)>) {
        for shape in self.pending.take().unwrap_or_default() {
            let picture = pictures.iter().position(|(id, _)| *id == shape.id).map(|i| pictures.swap_remove(i).1);
            let texture = picture.map(|p| ctx.load_texture(format!("mask thumbnail {}", shape.id), p, TextureOptions::LINEAR));
            self.put(shape, texture);
        }
    }
}

/// A mask as far as its coverage goes: names, visibility and adjustments change none of it.
fn shape_of(m: &Mask) -> Mask {
    let mut m = m.clone();
    m.name = String::new();
    m.visible = true;
    m.adjust = LocalAdjustments { amount: 100.0, ..LocalAdjustments::default() };
    m.decontaminate = 0.0;
    m.before_invert = Vec::new();
    m
}

/// Whether a picture can be made of `m` from what is already there: not while a selection is awaited, nor for an AI
/// part that holds no result.
fn drawable(m: &Mask) -> bool {
    let no_result = |shape: &MaskShape| match shape {
        MaskShape::Object { seg, detail, .. }
        | MaskShape::Prompt { seg, detail, .. }
        | MaskShape::Subject { seg, detail, .. }
        | MaskShape::Sky { seg, detail, .. }
        | MaskShape::Background { seg, detail, .. }
        | MaskShape::People { seg, detail, .. } => seg.is_none() && detail.is_empty(),
        MaskShape::Landscape { .. } => true,
        _ => false,
    };
    !m.components.is_empty() && !m.is_waiting() && !m.components.iter().any(|c| no_result(&c.shape))
}

/// Only what coverage is made from: the framing, the white balance and exposure the range masks select on, and
/// `masks`. The rest of the edit stays out, so no spot (or its LaMa fill), noise reduction, blur or retouching runs.
fn coverage_settings(d: &DevelopSettings, masks: Vec<Mask>) -> DevelopSettings {
    let d = d.effective();
    let mut s = DevelopSettings {
        process: d.process,
        wb: d.wb,
        optics: d.optics,
        geometry: d.geometry.clone(),
        crop: d.crop,
        orientation: d.orientation,
        masks,
        ..DevelopSettings::default()
    };
    s.light.exposure = d.light.exposure;
    s
}

/// Make the thumbnails that are due on `photo`: one job at a time, never while the pointer is down (a stroke or a drag
/// in progress) and only from a decoded source the loupe has left in memory.
pub(crate) fn update(app: &mut LightcraftApp, ctx: &egui::Context, photo: PhotoId, d: &Arc<DevelopSettings>) {
    let library = app.session.library_generation();
    let thumbs = &mut app.mask_list.thumbs;
    if thumbs.pending.is_some() {
        if app.tasks.is_running(TASK) {
            return;
        }
        // the worker died: not tried again for these shapes
        thumbs.finish(ctx, Vec::new());
    }
    if thumbs.current.as_ref().is_some_and(|(p, s)| *p == photo && Arc::ptr_eq(s, d)) {
        return;
    }
    let framing = Framing { library, photo, crop: d.crop, geometry: d.geometry.clone(), orientation: d.orientation };
    if thumbs.framing.as_ref() != Some(&framing) {
        // (the same photo framed anew keeps its pictures on screen until the new ones come)
        if thumbs.framing.as_ref().is_some_and(|f| (f.library, f.photo) == (library, photo)) {
            thumbs.made.iter_mut().for_each(|t| t.shape = None);
        } else {
            thumbs.made.clear();
        }
        thumbs.framing = Some(framing);
    }
    thumbs.made.retain(|t| d.masks.iter().any(|m| m.id == t.id));
    let mut due = Vec::new();
    for m in &d.masks {
        let shape = shape_of(m);
        if thumbs.made.iter().any(|t| t.id == m.id && t.shape.as_ref() == Some(&shape)) {
            continue;
        }
        if drawable(&shape) {
            due.push(shape);
        } else {
            thumbs.put(shape, None);
        }
    }
    if due.is_empty() {
        thumbs.current = Some((photo, d.clone()));
        return;
    }
    if ctx.input(|i| i.pointer.any_down()) || app.renderer.is_pending(Slot::Main) {
        return;
    }
    // (nothing in memory: the icons stay until the loupe has read the photo)
    let Some((source, header)) = held(app, photo) else { return };
    let settings = coverage_settings(d, due.clone());
    let work = move || {
        let info = source.info_or(header);
        planes(&proxy(&source.image), &info, &settings)
    };
    app.mask_list.thumbs.pending = Some(due);
    let started = crate::tasks::spawn(app, TASK, None, work, |app, ctx, result| {
        app.mask_list.thumbs.finish(ctx, result);
        ctx.request_repaint();
    });
    if let Err(e) = started {
        log::warn!("{e}");
    }
}

/// A decoded source of `photo` the app holds in memory (never one to be read), with the catalog's facts about it.
fn held(app: &mut LightcraftApp, photo: PhotoId) -> Option<(DecodedSource, lightcraft_pipeline::SourceInfo)> {
    ASKS.iter().find_map(|&ask| {
        let job = app.session.render_job(photo, ask, ask, false, true)?;
        let source = match job.source {
            SourceRef::Loaded(source) => *source,
            SourceRef::Fit { from, .. } => *from,
            _ => return None,
        };
        Some((source, job.info))
    })
}

/// Every n-th pixel of `src`, no longer than [`PROXY_EDGE`]: the work follows the proxy's size, whatever the source's
/// (a cropped photo's loupe holds only the original).
fn proxy(src: &Arc<Rgb32f>) -> Arc<Rgb32f> {
    let step = src.width.max(src.height).div_ceil(PROXY_EDGE).max(1);
    if step == 1 {
        return src.clone();
    }
    let mut out = Rgb32f::new(src.width.div_ceil(step), src.height.div_ceil(step));
    for (y, row) in out.data.chunks_mut(out.width.max(1)).enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            if let Some(from) = src.data.get(y * step * src.width + x * step) {
                *px = *from;
            }
        }
    }
    Arc::new(out)
}

/// Each mask's coverage over `src`, as a grey picture.
fn planes(src: &Arc<Rgb32f>, info: &lightcraft_pipeline::SourceInfo, settings: &DevelopSettings) -> Vec<(u32, ColorImage)> {
    let request = lightcraft_pipeline::RenderRequest::fit(EDGE, EDGE);
    let out = lightcraft_pipeline::mask_planes(src, info, settings, &request, None, &PlanesAsk { want: MaskWant::All, known: None });
    out.planes
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, p)| p.width > 0 && p.height > 0 && p.width.checked_mul(p.height) == Some(p.data.len()))
        .map(|(id, p)| {
            let grey: Vec<u8> = p.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
            (id, ColorImage::from_gray([p.width, p.height], &grey))
        })
        .collect()
}
