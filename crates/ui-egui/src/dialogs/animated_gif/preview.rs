//! Export Animated GIF's preview: the photos playing at the chosen speed on a canvas shaped like
//! the GIF (photos of another shape on black), and below it every photo in frame order, numbered,
//! in a strip that scrolls sideways. Library thumbnails stand in for the frames.

use std::time::Duration;

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;

use crate::LightcraftApp;
use crate::theme::Tokens;

/// The largest the playing canvas gets, in points.
const PLAY_W: f32 = 560.0;
const PLAY_H: f32 = 260.0;
/// The strip's cells, in points.
const CELL_W: f32 = 72.0;
const CELL_H: f32 = 54.0;
const GAP: f32 = 4.0;
/// The thumbnail asked for when a photo has none yet (the grid's own is used when it has one).
const THUMB_PX: usize = 256;

/// The preview for `ids` (frame order) at `w × h`, each photo up for `seconds`.
pub(super) fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[PhotoId], (w, h): (usize, usize), seconds: f64) {
    if ids.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let current = frame_at(ui.input(|i| i.time), seconds, ids.len());
    let step = seconds.max(0.02);
    let next = step - ui.input(|i| i.time).rem_euclid(step);
    ui.ctx().request_repaint_after(Duration::from_secs_f64(next.clamp(0.01, step)));

    let ratio = w as f32 / h.max(1) as f32;
    let size = if ratio >= PLAY_W / PLAY_H { vec2(PLAY_W, PLAY_W / ratio) } else { vec2(PLAY_H * ratio, PLAY_H) };
    let play = ui.vertical_centered(|ui| ui.allocate_exact_size(size, Sense::hover()).0).inner;
    crate::widgets::register(ui, "preview:animatedGif", play);
    let p = ui.painter();
    p.rect_filled(play, 0.0, Color32::BLACK);
    if let Some(id) = ids.get(current) {
        paint_photo(app, p, play, *id, &t);
    }
    let label = format!("{} / {}", current + 1, ids.len());
    let g = p.layout_no_wrap(label, t.font(11.0), Color32::WHITE);
    let at = play.right_bottom() - g.size() - vec2(6.0, 4.0);
    p.rect_filled(Rect::from_min_size(at - vec2(4.0, 1.0), g.size() + vec2(8.0, 2.0)), 3.0, Color32::from_black_alpha(160));
    p.galley(at, g, Color32::WHITE);

    egui::ScrollArea::horizontal().id_salt("animatedGifStrip").max_width(PLAY_W).show_viewport(ui, |ui, view| {
        let total = ids.len() as f32 * (CELL_W + GAP) - GAP;
        let (strip, _) = ui.allocate_exact_size(vec2(total, CELL_H), Sense::hover());
        // only the cells in view are drawn (and ask for thumbnails)
        let first = (view.min.x / (CELL_W + GAP)).floor().max(0.0) as usize;
        let last = ((view.max.x / (CELL_W + GAP)).ceil().max(0.0) as usize).min(ids.len());
        let p = ui.painter();
        for (i, id) in ids.iter().enumerate().take(last).skip(first) {
            let cell = Rect::from_min_size(strip.min + vec2(i as f32 * (CELL_W + GAP), 0.0), vec2(CELL_W, CELL_H));
            p.rect_filled(cell, 0.0, t.canvas);
            paint_photo(app, p, cell, *id, &t);
            crate::panels::selection_order::paint(p, cell.right() - 4.0, cell.top() + 9.0, i + 1, &t);
            if i == current {
                p.rect_stroke(cell, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            }
        }
    });
}

/// The frame showing `time` seconds in, at `seconds` a photo, looping over `n`.
fn frame_at(time: f64, seconds: f64, n: usize) -> usize {
    let step = if seconds.is_finite() { seconds.max(0.02) } else { 1.0 };
    let i = (time.max(0.0) / step).floor();
    if n == 0 || !i.is_finite() { 0 } else { (i % n as f64) as usize }
}

/// `id`'s thumbnail fitted in `area` (a placeholder while it renders).
fn paint_photo(app: &mut LightcraftApp, p: &egui::Painter, area: Rect, id: PhotoId, t: &Tokens) {
    if app.renderer.thumb(id).is_none() {
        crate::panels::grid::request_thumb(app, id, THUMB_PX, 10);
    }
    let Some(tex) = app.renderer.thumb(id) else {
        p.rect_filled(area.shrink(2.0), 0.0, t.skeleton);
        return;
    };
    let [tw, th] = tex.size;
    let s = (area.width() / tw.max(1) as f32).min(area.height() / th.max(1) as f32);
    let shown = Rect::from_center_size(area.center(), vec2(tw as f32 * s, th as f32 * s));
    p.image(tex.tex.id(), shown, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::frame_at;

    #[test]
    fn frames_advance_with_time_and_loop() {
        assert_eq!(frame_at(0.0, 1.0, 3), 0);
        assert_eq!(frame_at(1.5, 1.0, 3), 1);
        assert_eq!(frame_at(3.2, 1.0, 3), 0);
        assert_eq!(frame_at(0.25, 0.1, 3), 2);
        assert_eq!(frame_at(f64::INFINITY, 1.0, 3), 0);
        assert_eq!(frame_at(5.0, f64::NAN, 3), 2);
        assert_eq!(frame_at(5.0, 1.0, 0), 0);
    }
}
