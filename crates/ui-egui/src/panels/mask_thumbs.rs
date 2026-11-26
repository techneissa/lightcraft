//! The small coverage pictures of the Masking panel's list (fork issue #401): what each mask of the open photo selects,
//! white where selected, made from the pipeline's own mask planes (as the overlay is) on a worker.

use std::sync::Arc;

use egui::{ColorImage, TextureHandle, TextureOptions};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask};
use lightcraft_pipeline::masks::{MaskWant, PlanesAsk};

use crate::LightcraftApp;
use crate::render::Slot;

/// The long edge of a thumbnail, in pixels (28 points at 2x).
const EDGE: usize = 64;
/// The source the masks are read from: the preview level the loupe already holds.
const SOURCE_EDGE: usize = 1024;
const TASK: &str = "Mask Thumbnails";

/// What a set of thumbnails shows: the photo, its masks' shapes and its framing. Names, visibility and adjustments
/// change none of them.
#[derive(Clone, PartialEq)]
struct Key {
    /// (the same id in another library is another photo)
    library: u64,
    photo: PhotoId,
    framing: Box<(lightcraft_develop::Crop, lightcraft_develop::Geometry, lightcraft_geom::Orientation)>,
    masks: Vec<Mask>,
}

impl Key {
    fn of(library: u64, photo: PhotoId, d: &DevelopSettings) -> Key {
        let masks = d
            .masks
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.name = String::new();
                m.visible = true;
                m.adjust = LocalAdjustments { amount: 100.0, ..LocalAdjustments::default() };
                m.decontaminate = 0.0;
                m.before_invert = Vec::new();
                m
            })
            .collect();
        Key { library, photo, framing: Box::new((d.crop, d.geometry.clone(), d.orientation)), masks }
    }
}

#[derive(Default)]
pub(crate) struct MaskThumbs {
    made: Option<(Key, Vec<(u32, TextureHandle)>)>,
    pending: Option<Key>,
    /// The settings `made` was last found up to date with: while the photo keeps them, nothing is compared.
    current: Option<(PhotoId, Arc<DevelopSettings>)>,
}

impl MaskThumbs {
    /// The thumbnail of mask `id` of `photo` (the last one made, while a newer one is on its way).
    pub(crate) fn get(&self, photo: PhotoId, id: u32) -> Option<&TextureHandle> {
        let (key, textures) = self.made.as_ref()?;
        (key.photo == photo).then(|| textures.iter().find(|(m, _)| *m == id).map(|(_, t)| t))?
    }
}

/// Make the thumbnails of `photo` anew when its masks changed: one job at a time, never while the pointer is down (a
/// stroke or a drag in progress) and only once the loupe has its picture, whose source this then reads.
pub(crate) fn update(app: &mut LightcraftApp, ctx: &egui::Context, photo: PhotoId, d: &Arc<DevelopSettings>) {
    if let Some(key) = &app.mask_list.thumbs.pending {
        if app.tasks.is_running(TASK) {
            return;
        }
        // the worker died: not tried again for these masks
        app.mask_list.thumbs.made = Some((key.clone(), Vec::new()));
        app.mask_list.thumbs.pending = None;
    }
    if app.mask_list.thumbs.current.as_ref().is_some_and(|(p, s)| *p == photo && Arc::ptr_eq(s, d)) {
        return;
    }
    let key = Key::of(app.session.library_generation(), photo, d);
    if app.mask_list.thumbs.made.as_ref().is_some_and(|(made, _)| *made == key) {
        app.mask_list.thumbs.current = Some((photo, d.clone()));
        return;
    }
    if ctx.input(|i| i.pointer.any_down()) || app.renderer.is_pending(Slot::Main) {
        return;
    }
    let Some(job) = app.session.render_job(photo, SOURCE_EDGE, SOURCE_EDGE, false, true) else { return };
    let mut settings = DevelopSettings::clone(d);
    settings.masks.clone_from(&key.masks);
    let work = move || -> Result<Vec<(u32, ColorImage)>, String> {
        let source = job.source.load_source()?;
        let src = source.image_for(settings.denoise_amount());
        let info = source.info_or(job.info.clone());
        Ok(planes(&src, &info, &settings))
    };
    app.mask_list.thumbs.pending = Some(key);
    let started = crate::tasks::spawn(app, TASK, None, work, |app, ctx, result| {
        let Some(key) = app.mask_list.thumbs.pending.take() else { return };
        // (a photo that can't be read keeps the kind icons; the loupe says why)
        let textures = result
            .unwrap_or_default()
            .into_iter()
            .map(|(id, image)| (id, ctx.load_texture(format!("mask thumbnail {id}"), image, TextureOptions::LINEAR)))
            .collect();
        app.mask_list.thumbs.made = Some((key, textures));
        ctx.request_repaint();
    });
    if let Err(e) = started {
        log::warn!("{e}");
    }
}

/// Each mask's coverage over `src`, as a grey picture.
fn planes(src: &Arc<lightcraft_raster::Rgb32f>, info: &lightcraft_pipeline::SourceInfo, settings: &DevelopSettings) -> Vec<(u32, ColorImage)> {
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
