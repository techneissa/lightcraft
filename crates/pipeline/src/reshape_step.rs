//! Fork feature (`fork/retouch-reshape.md`): Face Reshape in the render's geometry.
//!
//! The Portrait tab's Face Reshape sliders become a warp of the photo before its lens corrections
//! ([`lightcraft_develop::retouch::reshape::Field`], put into the frame's [`crate::optics::Warp`] by
//! [`crate::frame_for`]). Everything that reads the photo through the warp then follows the reshaped face: the one
//! resample, local masks drawn on the photo (Select People's among them), the retouch step's faces and part masks
//! (`retouch_step::PhotoView`), the raw's local tone map. Spots, and masks kept after the warp, stay where they are
//! drawn, as they do under Upright.
//!
//! Off is exact: with the Portrait tab's eye off, every face's Face Reshape at 0, or no face with a face mesh, there
//! is no field and the frame is the frame without it.

use std::borrow::Cow;
use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::retouch::reshape::{Field, Reshape};
use lightcraft_develop::retouch::{Feature, MAX_FACES, overrides};
use lightcraft_geom::{Affine, Point};
use lightcraft_raster::{Rgb32f, par_rows};

use crate::SourceInfo;
use crate::geometry::Frame;
use crate::optics::Warp;
use crate::retouch_step::FrameKeyMap;

/// The Face Reshape warp of `s` for the faces of `info` on `frame` (built without a reshape); `None` when nothing
/// moves.
pub fn field(info: &SourceInfo, s: &DevelopSettings, frame: &Frame) -> Option<Arc<Field>> {
    field_of(info, s, frame, None)
}

/// [`field`] of face `only` alone (a place in the photo's faces; `None`: every face), the others left in place: the
/// ⌥-drag map measures how far each face moves (`crate::retouch_effect`).
pub fn field_of(info: &SourceInfo, s: &DevelopSettings, frame: &Frame, only: Option<usize>) -> Option<Arc<Field>> {
    if !lightcraft_develop::retouch::active(s) {
        return None;
    }
    let r = &s.retouch;
    if r.global.reshape.is_off() && r.faces.iter().all(|f| f.settings.reshape.is_off()) {
        return None;
    }
    let input = info.retouch.as_deref()?;
    let faces = input.faces.get(..input.faces.len().min(MAX_FACES))?;
    let keys = FrameKeyMap::new(frame, s.orientation);
    let own = overrides::resolve_in(r, faces, Some(&keys));
    // faces found after the lens corrections and transform: their landmarks taken back onto the photo
    let photo = frame.for_mask(true);
    let landmarks: Vec<Cow<'_, [(f64, f64)]>> = faces
        .iter()
        .map(|f| {
            if input.on_photo {
                Cow::Borrowed(f.landmarks.as_slice())
            } else {
                Cow::Owned(
                    f.landmarks
                        .iter()
                        .map(|&(x, y)| {
                            let p = photo.norm_to_mask(Point::new(x, y));
                            (p.x, p.y)
                        })
                        .collect(),
                )
            }
        })
        .collect();
    // (a face `only` leaves out keeps Face Reshape off)
    let still = Reshape::default();
    let list: Vec<(&[(f64, f64)], &Reshape)> = landmarks
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let settings = own.get(i).copied().flatten().and_then(|o| r.faces.get(o)).map_or(&r.global, |o| &o.settings);
            (l.as_ref(), if only.is_none_or(|o| o == i) { &settings.reshape } else { &still })
        })
        .collect();
    Field::build(&list, frame.ow, frame.oh).map(Arc::new)
}

/// Redo the pixels of `out` (sampled from `base` without the reshape: `o2t` maps its pixels to the photo's, `sx`,
/// `sy` the photo's to `base`'s) that lie in the faces' areas, through `warp`'s reshape: everything else the reshape
/// leaves in place, so a slider drag resamples the faces only.
pub(crate) fn resample_faces(out: &mut Rgb32f, base: &Rgb32f, sx: f64, sy: f64, warp: &Warp, o2t: Affine) {
    let Some(field) = &warp.reshape else { return };
    let Some(t2o) = o2t.inverse() else { return };
    let (w, h) = (out.width, out.height);
    for [x0, y0, x1, y1] in field.areas() {
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| t2o.apply(Point::new(x * warp.w, y * warp.h)));
        let (lo_x, hi_x) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.x), b.max(p.x)));
        let (lo_y, hi_y) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p.y), b.max(p.y)));
        if !(lo_x.is_finite() && hi_x.is_finite() && lo_y.is_finite() && hi_y.is_finite()) {
            continue;
        }
        let clamp = |v: f64, max: usize| v.clamp(0.0, max as f64) as usize;
        let (px0, px1) = (clamp(lo_x.floor() - 1.0, w), clamp(hi_x.ceil() + 1.0, w));
        let (py0, py1) = (clamp(lo_y.floor() - 1.0, h), clamp(hi_y.ceil() + 1.0, h));
        if px0 >= px1 || py0 >= py1 {
            continue;
        }
        let Some(rows) = out.data.get_mut(py0 * w..py1 * w) else { continue };
        par_rows(rows, w, |y, row| {
            for (x, px) in row.iter_mut().enumerate().take(px1).skip(px0) {
                let g = o2t.apply(Point::new(x as f64 + 0.5, (y + py0) as f64 + 0.5));
                let q = warp.reshaped(g);
                *px = base.sample_bilinear((q.x * sx) as f32, (q.y * sy) as f32);
            }
        });
    }
}

#[cfg(test)]
#[path = "reshape_step/tests.rs"]
mod tests;
