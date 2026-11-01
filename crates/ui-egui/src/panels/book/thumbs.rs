//! Drawing pages and photos: a page as paper with its photos, drawn from the thumbnails the grid
//! already has (a photo's thumbnail is asked for only when its frame is on screen, so a book of
//! 200 pages costs what the visible pages cost).
use egui::{Color32, Painter, Rect, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_engine::book::geometry::{cells_for, crop_window, fitted};
use lightcraft_engine::book::model::{Book, Crop, Template};
use lightcraft_engine::export::{ExportOptions, output_size};
use lightcraft_engine::print::layout::Rect as Inches;

use crate::LightcraftApp;

/// How a photo shows in its frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Show {
    /// The whole photo, centred.
    Fit,
    /// Filling the frame, the overflow cut off.
    Fill,
    /// A [`Crop`].
    Crop(Crop),
}

/// The photo's width over height as it exports, if it has a size.
pub(super) fn aspect(app: &LightcraftApp, id: PhotoId) -> Option<f64> {
    let photo = app.session.catalog.photo(id)?;
    let (w, h) = output_size(photo, &ExportOptions::default());
    (w > 0 && h > 0).then(|| w as f64 / h as f64)
}

fn to_screen(r: &Inches, page: Rect, scale: f32) -> Rect {
    Rect::from_min_size(page.min + vec2(r.x as f32, r.y as f32) * scale, vec2(r.w as f32, r.h as f32) * scale)
}

/// The frames of page `index` laid on `page` (screen points), in reading order.
pub(super) fn page_cells(book: &Book, index: usize, page: Rect) -> Vec<Rect> {
    let (Some(p), Ok((pw, _))) = (book.pages.get(index), book.settings.page_size()) else { return Vec::new() };
    let scale = page.width() / pw as f32;
    cells_for(p.template, &book.settings).unwrap_or_default().iter().map(|c| to_screen(c, page, scale)).collect()
}

/// How the photo of cell `cell` on page `index` shows: its crop (`pending` is a crop not yet saved,
/// for the cell it names), else as the book's settings say.
pub(super) fn show_of(book: &Book, index: usize, cell: usize, pending: Option<(usize, usize, Crop)>) -> Show {
    let Some(p) = book.pages.get(index) else { return Show::Fit };
    if let Some((pg, c, crop)) = pending
        && (pg, c) == (index, cell)
    {
        return Show::Crop(crop);
    }
    match p.crop(cell) {
        Some(c) => Show::Crop(c),
        None if book.settings.fill || p.template == Template::FullBleed => Show::Fill,
        None => Show::Fit,
    }
}

/// Paper, then each frame's photo (or an empty frame) of page `index` on `rect`.
pub(super) fn paint_page(
    app: &mut LightcraftApp,
    painter: &Painter,
    book: &Book,
    index: usize,
    rect: Rect,
    pending: Option<(usize, usize, Crop)>,
    shadow: bool,
) {
    let Some(page) = book.pages.get(index) else { return };
    let bg = book.settings.background;
    if shadow {
        painter.rect_filled(rect.translate(vec2(1.5, 2.0)), 0.0, Color32::from_black_alpha(70));
    }
    painter.rect_filled(rect, 0.0, Color32::from_rgb(bg[0], bg[1], bg[2]));
    if !painter.clip_rect().intersects(rect) {
        return;
    }
    for (c, (cell, photo)) in page_cells(book, index, rect).into_iter().zip(&page.slots).enumerate() {
        match photo {
            Some(id) => paint_photo(app, painter, cell, PhotoId(*id), show_of(book, index, c, pending)),
            None => {
                painter.rect_filled(cell, 0.0, Color32::from_gray(214));
            }
        }
    }
}

/// A photo inside `frame`, from its thumbnail. The thumbnail is asked for only when the frame is
/// in the painter's clip (on screen).
pub(super) fn paint_photo(app: &mut LightcraftApp, painter: &Painter, frame: Rect, id: PhotoId, show: Show) {
    if frame.width() < 1.0 || frame.height() < 1.0 || !painter.clip_rect().intersects(frame) {
        return;
    }
    let Some(aspect) = aspect(app, id) else {
        painter.rect_filled(frame, 0.0, Color32::from_gray(90));
        return;
    };
    // the part of the photo that shows (fractions of it), and where it lands
    let (shown, uv) = match show {
        Show::Fit => {
            let slot = Inches::new(f64::from(frame.left()), f64::from(frame.top()), f64::from(frame.width()), f64::from(frame.height()));
            let f = fitted(slot, aspect);
            (Rect::from_min_size(pos2(f.x as f32, f.y as f32), vec2(f.w as f32, f.h as f32)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)))
        }
        Show::Fill => {
            let (x, y, w, h) = crop_window(aspect, f64::from(frame.width() / frame.height()), &Crop::FILL);
            (frame, uv_rect(x, y, w, h))
        }
        Show::Crop(c) => {
            let (x, y, w, h) = crop_window(aspect, f64::from(frame.width() / frame.height()), &c);
            (frame, uv_rect(x, y, w, h))
        }
    };
    if app.renderer.thumb(id).is_none() {
        // (a thumbnail comes at the size it is shown, to the next bucket up)
        let px = (shown.width().max(shown.height()) * 2.0 / uv.width().max(0.05)).clamp(64.0, 1024.0) as usize;
        crate::panels::grid::request_thumb(app, id, px, 10);
    }
    let clip = painter.with_clip_rect(frame.intersect(painter.clip_rect()));
    match app.renderer.thumb(id).map(|tex| tex.tex.id()) {
        Some(tex) => {
            let mut mesh = egui::Mesh::with_texture(tex);
            let corners = [shown.left_top(), shown.right_top(), shown.right_bottom(), shown.left_bottom()];
            let uvs = [uv.left_top(), uv.right_top(), uv.right_bottom(), uv.left_bottom()];
            for (pos, uv) in corners.into_iter().zip(uvs) {
                mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
            }
            mesh.indices.extend([0, 1, 2, 0, 2, 3]);
            clip.add(egui::Shape::mesh(mesh));
        }
        // (the page is paper in every theme: a fixed grey while the thumbnail comes)
        None => {
            clip.rect_filled(shown, 0.0, Color32::from_gray(52));
        }
    }
}

fn uv_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32))
}

/// The crop after the photo was dragged `delta` points inside `frame`: the photo follows the
/// pointer, so the crop's centre moves the other way, by the part of the photo the frame shows.
pub(super) fn pan(current: Option<Crop>, frame: Rect, delta: egui::Vec2, aspect: f64) -> Crop {
    let c = current.unwrap_or(Crop::FILL);
    let (x, y, w, h) = crop_window(aspect, f64::from(frame.width() / frame.height().max(1.0)), &c);
    let dx = -f64::from(delta.x) / f64::from(frame.width().max(1.0)) * w;
    let dy = -f64::from(delta.y) / f64::from(frame.height().max(1.0)) * h;
    Crop { zoom: c.zoom, x: (x + w / 2.0 + dx).clamp(0.0, 1.0), y: (y + h / 2.0 + dy).clamp(0.0, 1.0) }
}
