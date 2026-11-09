//! The full-size crops Stray Hair searches (`fork/retouch-hair.md`): a head's pixels cut from the original, not from
//! the 2048 px people-parse picture, in which a strand is thinner than a pixel.
//!
//! The crop is rendered like the parse picture (the photo with default settings, so the masks fit), as a window of the
//! full-size frame ([`lightcraft_pipeline::PixelWindow`]): the original is decoded once and every head cut from it.

use lightcraft_color::transfer::decode_srgb8;
use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::NormRect;
use lightcraft_develop::retouch::hair::PhotoRect;
use lightcraft_develop::retouch::wrinkles::Crop;
use lightcraft_geom::Orientation;
use lightcraft_pipeline::{DeepSamples, OutputDepth, PixelWindow, RenderRequest};
use lightcraft_raster::Rgb32f;

use super::oriented;
use crate::{RenderJob, Session};

/// Cuts crops of one photo's original, as the faces' frame has it (turned by the photo's orientation).
pub(crate) struct Cropper {
    job: RenderJob,
    /// The original's size as stored (before the orientation).
    frame: (usize, usize),
    orient: Orientation,
}

impl Session {
    /// A [`Cropper`] for photo `id`, set up on the calling thread (it runs on a worker). `None` for a photo that has
    /// gone or has no size.
    pub(crate) fn retouch_cropper(&mut self, id: lightcraft_catalog::PhotoId) -> Option<Cropper> {
        let p = self.catalog.photo(id)?;
        let (w, h, orient) = (p.width as usize, p.height as usize, p.develop.orientation);
        if w == 0 || h == 0 {
            return None;
        }
        let mut job = self.preview_job(id, w, h, false, &DevelopSettings::default())?;
        // (a crop is never kept: not as the photo's preview, not in the thumbnail cache)
        job.cache = None;
        job.view_cache = None;
        job.info.retouch = None;
        Some(Cropper { job, frame: (w, h), orient })
    }
}

impl Cropper {
    /// The size of the photo as the faces see it (turned by the orientation).
    pub(crate) fn photo_size(&self) -> (usize, usize) {
        if self.orient.swaps_axes() { (self.frame.1, self.frame.0) } else { self.frame }
    }

    /// The pixels of `rect` (in the turned photo's pixels) in linear light, turned like the part masks; `None` when the
    /// render fails or comes back another size.
    pub(crate) fn crop(&self, rect: PhotoRect) -> Option<Rgb32f> {
        let (pw, ph) = self.photo_size();
        let (w, h) = (rect[2].checked_sub(rect[0])?, rect[3].checked_sub(rect[1])?);
        if w == 0 || h == 0 || rect[2] > pw || rect[3] > ph {
            return None;
        }
        // the window in the stored frame: the rect's first and last pixel centres, turned back
        let back = self.orient.inverse();
        let a = back.map(rect[0] as f64 + 0.5, rect[1] as f64 + 0.5, pw as f64, ph as f64);
        let b = back.map(rect[2] as f64 - 0.5, rect[3] as f64 - 0.5, pw as f64, ph as f64);
        let (x0, y0) = (a.0.min(b.0).floor().max(0.0) as usize, a.1.min(b.1).floor().max(0.0) as usize);
        let (x1, y1) = (a.0.max(b.0).floor().max(0.0) as usize + 1, a.1.max(b.1).floor().max(0.0) as usize + 1);
        let window = PixelWindow { x: x0, y: y0, w: x1.saturating_sub(x0), h: y1.saturating_sub(y0) };
        let mut job = self.job.clone();
        job.request.window = Some(window);
        // (its own key: a window is not the whole frame's picture)
        job.key = lightcraft_preview::Hasher128::new()
            .u64(job.key)
            .str("stray-hair-crop")
            .u64(window.x as u64)
            .u64(window.y as u64)
            .u64(window.w as u64)
            .u64(window.h as u64)
            .finish()
            .0 as u64;
        let image = job.run().rendered.ok()?.image;
        if (image.width, image.height) != (window.w, window.h) {
            return None;
        }
        let linear = Rgb32f::from_fn(image.width, image.height, |x, y| {
            let p = image.data.get(y * image.width + x).copied().unwrap_or([0, 0, 0, 255]);
            [decode_srgb8(p[0]), decode_srgb8(p[1]), decode_srgb8(p[2])]
        });
        let turned = oriented(&linear, self.orient);
        ((turned.width, turned.height) == (w, h)).then_some(turned)
    }

    /// The part of the photo inside `rect` (normalised, in the turned photo's frame) in floating-point linear light, at
    /// most `max_side` pixels on its longer side (never enlarged): rendered at that size straight from the original with
    /// the default settings (no retouching), as a 32-bit window (`OutputDepth::F32Linear`), so Wrinkles' search sees
    /// the shadows' steps an 8-bit picture quantises away. `None` when the render fails or comes back another size.
    pub(crate) fn crop_float(&self, rect: NormRect, max_side: usize) -> Option<Crop> {
        if !rect.is_valid() {
            return None;
        }
        let (pw, ph) = self.photo_size();
        let (fw, fh) = self.frame;
        // the rect in the stored frame (turned back), as a share of it
        let back = self.orient.inverse();
        let m = back.map_norm_rect(lightcraft_geom::Rect { x0: rect.x0, y0: rect.y0, x1: rect.x1, y1: rect.y1 });
        let (sx0, sy0, sx1, sy1) = (m.x0.clamp(0.0, 1.0), m.y0.clamp(0.0, 1.0), m.x1.clamp(0.0, 1.0), m.y1.clamp(0.0, 1.0));
        // the longer side of the crop, in the photo's pixels, sets how far the render is reduced
        let long = ((rect.x1 - rect.x0) * pw as f64).max((rect.y1 - rect.y0) * ph as f64);
        let scale = if long.is_finite() && long > 0.0 { (max_side as f64 / long).min(1.0) } else { return None };
        let (ow, oh) = lightcraft_raster::resample::fit_size(
            fw,
            fh,
            ((fw as f64 * scale).round() as usize).max(1),
            ((fh as f64 * scale).round() as usize).max(1),
        );
        let (x0, y0) = ((sx0 * ow as f64).floor() as usize, (sy0 * oh as f64).floor() as usize);
        let (x1, y1) = (((sx1 * ow as f64).ceil() as usize).min(ow), ((sy1 * oh as f64).ceil() as usize).min(oh));
        let window = PixelWindow { x: x0, y: y0, w: x1.checked_sub(x0)?, h: y1.checked_sub(y0)? };
        if window.w == 0 || window.h == 0 {
            return None;
        }
        let mut job = self.job.clone();
        job.request = RenderRequest { max_w: ow, max_h: oh, depth: OutputDepth::F32Linear, window: Some(window), ..job.request };
        job.key = lightcraft_preview::Hasher128::new()
            .u64(job.key)
            .str("wrinkles-float-crop")
            .u64(ow as u64)
            .u64(oh as u64)
            .u64(window.x as u64)
            .u64(window.y as u64)
            .u64(window.w as u64)
            .u64(window.h as u64)
            .finish()
            .0 as u64;
        let deep = job.run().rendered.ok()?.deep?;
        let DeepSamples::F32(samples) = &deep.samples else { return None };
        if (deep.width, deep.height) != (window.w, window.h) || samples.len() != window.w * window.h * 3 {
            return None;
        }
        let linear = Rgb32f { width: window.w, height: window.h, data: samples.as_chunks::<3>().0.to_vec() };
        let img = oriented(&linear, self.orient);
        // what it covers, as a share of the stored frame, turned like the faces
        let covered = self.orient.map_norm_rect(lightcraft_geom::Rect {
            x0: window.x as f64 / ow as f64,
            y0: window.y as f64 / oh as f64,
            x1: (window.x + window.w) as f64 / ow as f64,
            y1: (window.y + window.h) as f64 / oh as f64,
        });
        Some(Crop { img, rect: NormRect::new(covered.x0, covered.y0, covered.x1, covered.y1) })
    }
}
