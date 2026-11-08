//! Where a template's cells lie on a page. Inches, origin at the page's top-left corner.
use super::model::{BookSettings, Crop, Template};
use crate::print::layout::{MIN_CELL_IN, Rect};

/// A cell's slot is the template's share of the page inside the margins, less the padding.
/// A full-bleed page is one cell over the whole page.
pub fn cells(template: Template, page: (f64, f64), margin: f64, padding: f64) -> Vec<Rect> {
    let (w, h) = page;
    if template == Template::FullBleed {
        return vec![Rect::new(0.0, 0.0, w, h)];
    }
    let area = Rect::new(margin, margin, (w - 2.0 * margin).max(0.0), (h - 2.0 * margin).max(0.0));
    // portrait and square pages stack their halves; landscape pages put them side by side
    let stack = area.h >= area.w;
    let halves = |r: Rect| -> (Rect, Rect) {
        if stack {
            (Rect::new(r.x, r.y, r.w, r.h / 2.0), Rect::new(r.x, r.y + r.h / 2.0, r.w, r.h / 2.0))
        } else {
            (Rect::new(r.x, r.y, r.w / 2.0, r.h), Rect::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h))
        }
    };
    // the other direction, for the small cells of Three and the rows of Grid4
    let across = |r: Rect| -> (Rect, Rect) {
        if stack {
            (Rect::new(r.x, r.y, r.w / 2.0, r.h), Rect::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h))
        } else {
            (Rect::new(r.x, r.y, r.w, r.h / 2.0), Rect::new(r.x, r.y + r.h / 2.0, r.w, r.h / 2.0))
        }
    };
    let slots = match template {
        Template::One | Template::FullBleed => vec![area],
        Template::Two => {
            let (a, b) = halves(area);
            vec![a, b]
        }
        Template::Three => {
            let (big, rest) = halves(area);
            let (a, b) = across(rest);
            vec![big, a, b]
        }
        Template::Grid4 => {
            let (top, bottom) = (Rect::new(area.x, area.y, area.w, area.h / 2.0), Rect::new(area.x, area.y + area.h / 2.0, area.w, area.h / 2.0));
            let (a, b) = (Rect::new(top.x, top.y, top.w / 2.0, top.h), Rect::new(top.x + top.w / 2.0, top.y, top.w / 2.0, top.h));
            let (c, d) =
                (Rect::new(bottom.x, bottom.y, bottom.w / 2.0, bottom.h), Rect::new(bottom.x + bottom.w / 2.0, bottom.y, bottom.w / 2.0, bottom.h));
            vec![a, b, c, d]
        }
    };
    slots.into_iter().map(|r| Rect::new(r.x + padding, r.y + padding, (r.w - 2.0 * padding).max(0.0), (r.h - 2.0 * padding).max(0.0))).collect()
}

/// The cells of `template` under `settings`.
pub fn cells_for(template: Template, settings: &BookSettings) -> Result<Vec<Rect>, String> {
    let page = settings.page_size()?;
    Ok(cells(template, page, settings.margin, settings.padding))
}

/// Refuses settings that leave a cell of any template smaller than the composer takes.
pub fn check_cells(settings: &BookSettings) -> Result<(), String> {
    for t in Template::ALL {
        for r in cells_for(t, settings)? {
            if !(r.w >= MIN_CELL_IN && r.h >= MIN_CELL_IN) {
                return Err(format!("the margin and padding leave a cell of the {} template under {MIN_CELL_IN} in; lower them", t.name()));
            }
        }
    }
    Ok(())
}

/// The largest rectangle of `aspect` (width / height) centred in `slot`: how a photo shows when
/// the whole photo fits. A cell never shrinks below the composer's smallest, whatever the aspect.
pub fn fitted(slot: Rect, aspect: f64) -> Rect {
    if !(aspect.is_finite() && aspect > 0.0) || slot.w <= 0.0 || slot.h <= 0.0 {
        return slot;
    }
    let (mut w, mut h) = if aspect >= slot.w / slot.h { (slot.w, slot.w / aspect) } else { (slot.h * aspect, slot.h) };
    w = w.max(MIN_CELL_IN).min(slot.w);
    h = h.max(MIN_CELL_IN).min(slot.h);
    Rect::new(slot.x + (slot.w - w) / 2.0, slot.y + (slot.h - h) / 2.0, w, h)
}

/// The part of a photo a [`Crop`] shows in a frame, as fractions of the photo: `(x, y, w, h)`, with
/// the frame's aspect (`slot_aspect`, width over height) as long as the photo's (`photo_aspect`) is
/// known. The preview and the export both take their pixels from it, so they show the same part at
/// any size. A photo or frame of no size shows whole.
pub fn crop_window(photo_aspect: f64, slot_aspect: f64, crop: &Crop) -> (f64, f64, f64, f64) {
    if !(photo_aspect.is_finite() && photo_aspect > 0.0 && slot_aspect.is_finite() && slot_aspect > 0.0) {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let zoom = if crop.zoom.is_finite() { crop.zoom.clamp(1.0, super::model::MAX_ZOOM) } else { 1.0 };
    let (fw, fh) = if photo_aspect >= slot_aspect { (slot_aspect / photo_aspect, 1.0) } else { (1.0, photo_aspect / slot_aspect) };
    let (w, h) = (fw / zoom, fh / zoom);
    let at = |centre: f64, size: f64| (if centre.is_finite() { centre.clamp(0.0, 1.0) } else { 0.5 } - size / 2.0).clamp(0.0, 1.0 - size);
    (at(crop.x, w), at(crop.y, h), w, h)
}
