//! Drawing the preview: the spread (or two, while one slides over the other) on a dark ground with
//! a soft shadow and a gutter at the spine, and the controls that show while the pointer moves: a
//! close button, the page counter and a strip of spread thumbnails.
use egui::{Align2, Color32, Painter, Pos2, Rect, Sense, Ui, pos2, vec2};
use lightcraft_engine::book::model::Book;

use super::{Preview, cache::Cache};
use crate::LightcraftApp;
use crate::panels::book::thumbs;
use crate::widgets::register;

/// The ground behind the pages.
pub(super) const GROUND: Color32 = Color32::from_rgb(18, 18, 20);
/// Height of a thumbnail in the strip, points.
const THUMB_H: f32 = 58.0;
/// Space between spreads in the strip.
const THUMB_GAP: f32 = 14.0;
/// Space between the two pages of a thumbnail spread.
const THUMB_SPINE: f32 = 1.0;

/// What the controls were used for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Close,
    /// Go to this page's spread.
    Page(usize),
    /// A click on the left (`-1`) or right (`1`) half of the screen.
    Turn(i32),
}

/// The size of one page on `area` (points): two pages side by side fit with room around them.
pub(super) fn page_size(area: Rect, page: (f64, f64)) -> (f32, f32) {
    let (pw, ph) = (page.0.max(0.01), page.1.max(0.01));
    let scale = ((f64::from(area.width()) * 0.92) / (2.0 * pw)).min(f64::from(area.height()) * 0.86 / ph);
    ((pw * scale) as f32, (ph * scale) as f32)
}

/// The rect of each page of `spread` on `area`, the spread centred and moved `dx` to the right.
pub(super) fn spread_rects(area: Rect, size: (f32, f32), count: usize, dx: f32) -> Vec<Rect> {
    let total = size.0 * count as f32;
    let left = area.center().x - total / 2.0 + dx;
    let top = area.center().y - size.1 / 2.0;
    (0..count).map(|k| Rect::from_min_size(pos2(left + size.0 * k as f32, top), vec2(size.0, size.1))).collect()
}

/// Pixels across the longer side of a page drawn `size` points wide and high, rounded up to a
/// step so a window that grows a little doesn't render every page again.
pub(super) fn edge_of(ctx: &egui::Context, size: (f32, f32)) -> usize {
    let px = (size.0.max(size.1) * ctx.pixels_per_point()).ceil().max(64.0) as usize;
    let side = ctx.input(|i| i.max_texture_side).max(256);
    (px.div_ceil(64) * 64).min(lightcraft_engine::book::preview::MAX_EDGE).min(side)
}

fn gradient_x(painter: &Painter, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = egui::Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    for (pos, color) in [(rect.left_top(), left), (rect.right_top(), right), (rect.right_bottom(), right), (rect.left_bottom(), left)] {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

/// A spread: the shadow under its pages, each page (from the render when it is ready, else from
/// the grid's thumbnails), and the gutter where two pages meet. `true`: a page is drawn from
/// thumbnails and its render has not ended.
pub(super) fn spread(app: &mut LightcraftApp, painter: &Painter, cache: &Cache, book: &Book, pages: &[usize], rects: &[Rect], edge: usize) -> bool {
    let (Some(first), Some(last)) = (rects.first(), rects.last()) else { return false };
    let mut standin = false;
    let all = first.union(*last);
    painter.add(egui::Shadow { offset: [0, 10], blur: 44, spread: 0, color: Color32::from_black_alpha(190) }.as_shape(all, 0.0));
    painter.add(egui::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(110) }.as_shape(all, 0.0));
    for (&index, &rect) in pages.iter().zip(rects) {
        match cache.texture(book, index, edge) {
            Some(tex) => {
                painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                standin |= !cache.settled(book, index, edge);
                thumbs::paint_page(app, painter, book, index, rect, None, false)
            }
        }
    }
    if let [left, right] = rects {
        let g = (left.width() * 0.045).clamp(8.0, 40.0);
        let dark = Color32::from_black_alpha(70);
        gradient_x(painter, Rect::from_min_max(pos2(left.right() - g, left.top()), left.right_bottom()), Color32::TRANSPARENT, dark);
        gradient_x(painter, Rect::from_min_max(right.left_top(), pos2(right.left() + g, right.bottom())), dark, Color32::TRANSPARENT);
    }
    standin
}

/// The text of the page counter for the spread of `pages`.
pub(super) fn counter_text(book: &Book, pages: &[usize]) -> String {
    let n = book.pages.len();
    match pages {
        [] => crate::i18n::tr("No pages").to_string(),
        [0] if book.cover && n == 1 => crate::i18n::tr("Cover · 1 page").to_string(),
        [0] if book.cover => crate::i18n::tr_format!("Cover · {n} pages", n = n),
        [p] => crate::i18n::tr_format!("Page {i} of {n}", i = p + 1, n = n),
        [a, .., b] => crate::i18n::tr_format!("Pages {a}–{b} of {n}", a = a + 1, b = b + 1, n = n),
    }
}

/// Which spreads fit in the strip around `current`, `(first, last)`, given `room` points of width.
pub(super) fn strip_window(spreads: &[Vec<usize>], current: usize, room: f32, page_w: f32) -> (usize, usize) {
    let width = |s: usize| spreads.get(s).map_or(0.0, |p| p.len() as f32 * page_w + THUMB_SPINE * p.len().saturating_sub(1) as f32);
    let (mut lo, mut hi) = (current, current);
    let mut used = width(current);
    loop {
        let mut grew = false;
        if hi + 1 < spreads.len() && used + THUMB_GAP + width(hi + 1) <= room {
            used += THUMB_GAP + width(hi + 1);
            hi += 1;
            grew = true;
        }
        if lo > 0 && used + THUMB_GAP + width(lo - 1) <= room {
            used += THUMB_GAP + width(lo - 1);
            lo -= 1;
            grew = true;
        }
        if !grew {
            return (lo, hi);
        }
    }
}

/// The controls over the spread: close, counter and thumbnail strip. `alpha` fades them (0 to 1).
pub(super) fn controls(
    app: &mut LightcraftApp,
    ui: &mut Ui,
    pv: &Preview,
    book: &Book,
    spreads: &[Vec<usize>],
    area: Rect,
    alpha: f32,
) -> Option<Action> {
    let painter = ui.painter_at(area);
    let fade = |c: Color32| c.gamma_multiply(alpha);
    let mut action = None;
    // close
    let close = Rect::from_center_size(pos2(area.right() - 34.0, area.top() + 34.0), vec2(34.0, 34.0));
    let resp = ui.interact(close, egui::Id::unique("book-preview-close"), Sense::click());
    painter.circle_filled(close.center(), 17.0, fade(Color32::from_black_alpha(if resp.hovered() { 200 } else { 140 })));
    crate::icons::paint(&painter, Rect::from_center_size(close.center(), vec2(14.0, 14.0)), crate::icons::Icon::Close, fade(Color32::WHITE));
    register(ui, "button:bookPreviewClose", close);
    crate::widgets::announce(&resp, crate::i18n::tr("Close Preview"));
    if resp.clicked() {
        action = Some(Action::Close);
    }
    // the strip's backdrop, counter and thumbnails
    let Ok((pw, ph)) = book.settings.page_size() else { return action };
    let thumb_w = (f64::from(THUMB_H) * pw / ph) as f32;
    let current = spreads.iter().position(|s| s.contains(&pv.page)).unwrap_or(0);
    let bar = Rect::from_min_max(pos2(area.left(), area.bottom() - THUMB_H - 62.0), area.max);
    gradient_y(&painter, bar, Color32::TRANSPARENT, fade(Color32::from_black_alpha(215)));
    let shown = spreads.get(current).map(Vec::as_slice).unwrap_or_default();
    let text = counter_text(book, shown);
    let label = painter.text(
        pos2(area.center().x, area.bottom() - THUMB_H - 34.0),
        Align2::CENTER_CENTER,
        text,
        crate::theme::Tokens::get(ui.ctx()).font(12.5),
        fade(Color32::WHITE),
    );
    register(ui, "book:previewCounter", label.expand(4.0));
    let (lo, hi) = strip_window(spreads, current, (area.width() - 120.0).min(1100.0), thumb_w);
    let widths: Vec<f32> =
        (lo..=hi).map(|s| spreads.get(s).map_or(0.0, |p| p.len() as f32 * thumb_w + THUMB_SPINE * p.len().saturating_sub(1) as f32)).collect();
    let total: f32 = widths.iter().sum::<f32>() + THUMB_GAP * widths.len().saturating_sub(1) as f32;
    let mut x = area.center().x - total / 2.0;
    let top = area.bottom() - THUMB_H - 18.0;
    let t = crate::theme::Tokens::get(ui.ctx());
    for (s, w) in (lo..=hi).zip(widths) {
        let Some(pages) = spreads.get(s) else { continue };
        let frame = Rect::from_min_size(pos2(x, top), vec2(w, THUMB_H));
        for (k, &index) in pages.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x + k as f32 * (thumb_w + THUMB_SPINE), top), vec2(thumb_w, THUMB_H));
            thumbs::paint_page(app, &painter, book, index, r, None, false);
        }
        if s == current {
            painter.rect_stroke(frame.expand(2.0), 1.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
        } else if alpha < 1.0 {
            painter.rect_filled(frame, 0.0, Color32::from_black_alpha(((1.0 - alpha) * 120.0) as u8));
        }
        let hit = ui.interact(frame.expand(2.0), egui::Id::unique(("book-preview-thumb", s)), Sense::click());
        register(ui, format!("book:previewThumb:{s}"), frame);
        if hit.clicked() {
            action = pages.first().map(|p| Action::Page(*p));
        }
        x += w + THUMB_GAP;
    }
    action
}

fn gradient_y(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    for (pos, color) in [(rect.left_top(), top), (rect.right_top(), top), (rect.right_bottom(), bottom), (rect.left_bottom(), bottom)] {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    painter.add(egui::Shape::mesh(mesh));
}

/// Which half of `area` a point is in: `-1` left, `1` right.
pub(super) fn half(area: Rect, at: Pos2) -> i32 {
    if at.x < area.center().x { -1 } else { 1 }
}
