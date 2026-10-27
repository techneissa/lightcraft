//! Export Animated GIF's preview: the photos playing at the chosen speed on a canvas shaped like
//! the GIF (photos of another shape on black), with ‹ Play/Pause › below it, and every photo in
//! frame order, numbered, in a strip that scrolls sideways (the mouse wheel scrolls it too).
//! Library thumbnails stand in for the frames.
//!
//! The strip edits the GIF: a click picks a photo (and pauses on it), a drag moves it, ✕ or
//! Delete takes it out. The frames are the selection in the order it was made, so these change
//! the selection (`library.select`), and the grid's numbers follow.

use std::time::Duration;

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::json;

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
/// Points a frame the strip scrolls while a photo is dragged near its ends.
const EDGE_SCROLL: f32 = 8.0;

/// Playback and strip state, kept in egui's memory while the dialog is open.
#[derive(Clone, Copy, Debug, Default)]
struct Play {
    paused: bool,
    /// Playback counts from this frame at this time (re-anchored on pause, a click or a new speed).
    from_frame: usize,
    from_time: f64,
    step: f64,
    /// The photo clicked in the strip (Delete takes it out).
    picked: Option<PhotoId>,
    /// The strip index being dragged.
    dragging: Option<usize>,
    /// Where the strip should scroll to on the next frame.
    scroll_to: Option<f32>,
}

/// What the strip asked for this frame.
enum Edit {
    Remove(usize),
    Move { from: usize, to: usize },
}

/// The preview for `ids` (frame order) at `w × h`, each photo up for `seconds`.
pub(super) fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, ids: &[PhotoId], (w, h): (usize, usize), seconds: f64) {
    let n = ids.len();
    if n == 0 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let key = egui::Id::unique("animatedGifPreview");
    let now = ui.input(|i| i.time);
    let step = if seconds.is_finite() { seconds.max(lightcraft_engine::animated_gif::SECONDS_MIN) } else { 1.0 };
    let mut st: Play = ui.data(|d| d.get_temp(key)).unwrap_or(Play { from_time: now, step, ..Default::default() });
    if st.step != step {
        st.from_frame = st.current(now, n);
        st.from_time = now;
        st.step = step;
    }
    let current = st.current(now, n);
    if !st.paused {
        let next = step - (now - st.from_time).max(0.0).rem_euclid(step);
        ui.ctx().request_repaint_after(Duration::from_secs_f64(next.clamp(0.01, step)));
    }

    let ratio = w as f32 / h.max(1) as f32;
    let size = if ratio >= PLAY_W / PLAY_H { vec2(PLAY_W, PLAY_W / ratio) } else { vec2(PLAY_H * ratio, PLAY_H) };
    let play = ui.vertical_centered(|ui| ui.allocate_exact_size(size, Sense::hover()).0).inner;
    crate::widgets::register(ui, "preview:animatedGif", play);
    let p = ui.painter();
    p.rect_filled(play, 0.0, Color32::BLACK);
    if let Some(id) = ids.get(current) {
        paint_photo(app, p, play, *id, &t);
    }
    let g = p.layout_no_wrap(format!("{} / {n}", current + 1), t.font(11.0), Color32::WHITE);
    let at = play.right_bottom() - g.size() - vec2(6.0, 4.0);
    p.rect_filled(Rect::from_min_size(at - vec2(4.0, 1.0), g.size() + vec2(8.0, 2.0)), 3.0, Color32::from_black_alpha(160));
    p.galley(at, g, Color32::WHITE);

    // ‹ Play/Pause ›; Space plays and pauses when nothing has the keyboard
    let free_keys = ui.memory(|m| m.focused().is_none());
    ui.vertical_centered(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = 3.0 * 4.0 + 2.0 * 28.0 + 64.0;
            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
            if crate::widgets::text_button(ui, "animatedGifPrev", "‹", false).on_hover_text(crate::i18n::tr("Previous photo")).clicked() {
                st.pause_on((current + n - 1) % n, now);
            }
            let label = if st.paused { "Play" } else { "Pause" };
            let space = free_keys && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
            if crate::widgets::text_button(ui, "animatedGifPlay", label, false).clicked() || space {
                if st.paused {
                    st.paused = false;
                    st.from_frame = current;
                    st.from_time = now;
                } else {
                    st.pause_on(current, now);
                }
            }
            if crate::widgets::text_button(ui, "animatedGifNext", "›", false).on_hover_text(crate::i18n::tr("Next photo")).clicked() {
                st.pause_on((current + 1) % n, now);
            }
        });
    });

    let mut edit = None;
    let mut area = egui::ScrollArea::horizontal().id_salt("animatedGifStrip").max_width(PLAY_W);
    if let Some(x) = st.scroll_to.take() {
        area = area.horizontal_scroll_offset(x);
    }
    let out = area.show_viewport(ui, |ui, view| {
        let total = n as f32 * (CELL_W + GAP) - GAP;
        let (strip, _) = ui.allocate_exact_size(vec2(total, CELL_H), Sense::hover());
        // only the cells in view are drawn (and ask for thumbnails)
        let first = (view.min.x / (CELL_W + GAP)).floor().max(0.0) as usize;
        let last = ((view.max.x / (CELL_W + GAP)).ceil().max(0.0) as usize).min(n);
        for (i, id) in ids.iter().enumerate().take(last).skip(first) {
            let cell = Rect::from_min_size(strip.min + vec2(i as f32 * (CELL_W + GAP), 0.0), vec2(CELL_W, CELL_H));
            let resp = ui.interact(cell, egui::Id::unique(("animatedGifFrame", id.0)), Sense::click_and_drag());
            crate::widgets::register(ui, format!("animatedGifFrame:{i}"), cell);
            if resp.clicked() {
                st.picked = Some(*id);
                st.pause_on(i, now);
            }
            if resp.drag_started() && n > 1 {
                st.dragging = Some(i);
            }
            let p = ui.painter();
            p.rect_filled(cell, 0.0, t.canvas);
            paint_photo(app, p, cell, *id, &t);
            if st.dragging == Some(i) {
                p.rect_filled(cell, 0.0, Color32::from_black_alpha(150));
            }
            crate::panels::selection_order::paint(p, cell.right() - 4.0, cell.top() + 9.0, i + 1, &t);
            if st.picked == Some(*id) {
                p.rect_stroke(cell, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
            } else if i == current {
                p.rect_stroke(cell, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
            }
            // ✕ on the hovered or picked photo takes it out of the GIF (one photo always stays)
            if n > 1 && st.dragging.is_none() && (resp.hovered() || st.picked == Some(*id) || ui.rect_contains_pointer(cell)) {
                let x = Rect::from_center_size(cell.left_top() + vec2(10.0, 10.0), vec2(16.0, 16.0));
                let xr = ui
                    .interact(x, egui::Id::unique(("animatedGifRemove", id.0)), Sense::click())
                    .on_hover_text(crate::i18n::tr("Remove from the GIF"));
                crate::widgets::register(ui, format!("animatedGifRemove:{i}"), x);
                let p = ui.painter();
                p.circle_filled(x.center(), 8.0, if xr.hovered() { t.accent } else { Color32::from_black_alpha(200) });
                let (a, b) = (3.5, Stroke::new(1.5, Color32::WHITE));
                p.line_segment([x.center() + vec2(-a, -a), x.center() + vec2(a, a)], b);
                p.line_segment([x.center() + vec2(-a, a), x.center() + vec2(a, -a)], b);
                if xr.clicked() {
                    edit = Some(Edit::Remove(i));
                }
            }
        }
        // a dragged photo goes where the pointer is: a line marks the gap it will land in
        if let (Some(from), Some(pos)) = (st.dragging, ui.input(|i| i.pointer.interact_pos())) {
            let slot = (((pos.x - strip.min.x) / (CELL_W + GAP)).round().max(0.0) as usize).min(n);
            let x = strip.min.x + slot as f32 * (CELL_W + GAP) - GAP / 2.0;
            ui.painter().line_segment([pos2(x, strip.top()), pos2(x, strip.bottom())], Stroke::new(3.0, t.accent));
            if ui.input(|i| i.pointer.any_released()) {
                st.dragging = None;
                edit = Some(Edit::Move { from, to: slot });
            }
        }
    });

    // the mouse wheel's up/down scrolls the strip sideways; a drag near either end scrolls it too
    let mut dx = 0.0;
    if ui.rect_contains_pointer(out.inner_rect) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            dx -= dy;
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    if st.dragging.is_some()
        && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
    {
        if pos.x < out.inner_rect.left() + CELL_W / 2.0 {
            dx -= EDGE_SCROLL;
        } else if pos.x > out.inner_rect.right() - CELL_W / 2.0 {
            dx += EDGE_SCROLL;
        }
        ui.ctx().request_repaint();
    }
    if dx != 0.0 {
        let max = (out.content_size.x - out.inner_rect.width()).max(0.0);
        st.scroll_to = Some((out.state.clamped_offset().x + dx).clamp(0.0, max));
        ui.ctx().request_repaint();
    }
    if st.dragging.is_some() && !ui.input(|i| i.pointer.any_down()) {
        st.dragging = None;
    }

    // Delete or Backspace takes the picked photo out, when nothing else has the keyboard
    if edit.is_none()
        && free_keys
        && n > 1
        && let Some(i) = st.picked.and_then(|id| ids.iter().position(|x| *x == id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
    {
        edit = Some(Edit::Remove(i));
    }
    if let Some(e) = edit
        && let Some(order) = edited(ids, &e)
    {
        let last = order.len().saturating_sub(1);
        match e {
            Edit::Remove(i) => {
                // the next photo is picked, so Delete can go on taking photos out
                st.picked = st.picked.and_then(|_| order.get(i.min(last)).copied());
                st.from_frame = st.from_frame.min(last);
            }
            Edit::Move { from, .. } => {
                // the moved photo is picked and shown where it landed
                let moved = ids.get(from).copied();
                st.picked = moved;
                st.pause_on(order.iter().position(|id| Some(*id) == moved).unwrap_or(0), now);
            }
        }
        let active = app.session.selection.active.filter(|a| order.contains(a)).or(order.first().copied());
        let ids: Vec<u64> = order.iter().map(|id| id.0).collect();
        if let Err(e) = app.run("library.select", json!({"ids": ids, "active": active.map(|a| a.0)})) {
            app.toast(ui.ctx(), e);
        }
    }
    ui.data_mut(|d| d.insert_temp(key, st));
}

impl Play {
    /// The frame showing at `now`, of `n`.
    fn current(&self, now: f64, n: usize) -> usize {
        if self.paused { if n == 0 { 0 } else { self.from_frame % n } } else { frame_at(now - self.from_time, self.step, self.from_frame, n) }
    }

    fn pause_on(&mut self, frame: usize, now: f64) {
        self.paused = true;
        self.from_frame = frame;
        self.from_time = now;
    }
}

/// `ids` after `edit`, or None when it changes nothing (or would leave no photo).
fn edited(ids: &[PhotoId], edit: &Edit) -> Option<Vec<PhotoId>> {
    let mut order = ids.to_vec();
    match *edit {
        Edit::Remove(i) => {
            if order.len() < 2 || i >= order.len() {
                return None;
            }
            order.remove(i);
        }
        Edit::Move { from, to } => {
            // `to` is a gap: 0 before the first photo, len after the last
            if from >= order.len() || to > order.len() || to == from || to == from + 1 {
                return None;
            }
            let id = order.remove(from);
            order.insert(if to > from { to - 1 } else { to }, id);
        }
    }
    Some(order)
}

/// The frame showing `elapsed` seconds after `start` began, at `seconds` a photo, looping over `n`.
fn frame_at(elapsed: f64, seconds: f64, start: usize, n: usize) -> usize {
    let step = if seconds.is_finite() { seconds.max(0.02) } else { 1.0 };
    let i = (elapsed.max(0.0) / step).floor();
    if n == 0 || !i.is_finite() { 0 } else { ((i % n as f64) as usize + start % n) % n }
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
    use super::{Edit, edited, frame_at};
    use lightcraft_catalog::PhotoId;

    #[test]
    fn frames_advance_with_time_and_loop() {
        assert_eq!(frame_at(0.0, 1.0, 0, 3), 0);
        assert_eq!(frame_at(1.5, 1.0, 0, 3), 1);
        assert_eq!(frame_at(3.2, 1.0, 0, 3), 0);
        assert_eq!(frame_at(0.25, 0.1, 0, 3), 2);
        assert_eq!(frame_at(1.0, 1.0, 2, 3), 0, "from the frame playback restarted on");
        assert_eq!(frame_at(f64::INFINITY, 1.0, 0, 3), 0);
        assert_eq!(frame_at(5.0, f64::NAN, 0, 3), 2);
        assert_eq!(frame_at(5.0, 1.0, 0, 0), 0);
    }

    #[test]
    fn removing_and_moving_photos() {
        let ids: Vec<PhotoId> = [1, 2, 3, 4].map(PhotoId).to_vec();
        let order = |e| edited(&ids, &e).map(|v| v.iter().map(|id| id.0).collect::<Vec<_>>());
        assert_eq!(order(Edit::Remove(1)), Some(vec![1, 3, 4]));
        assert_eq!(order(Edit::Remove(9)), None);
        assert_eq!(edited(&[PhotoId(1)], &Edit::Remove(0)), None, "the last photo stays");
        assert_eq!(order(Edit::Move { from: 0, to: 4 }), Some(vec![2, 3, 4, 1]));
        assert_eq!(order(Edit::Move { from: 3, to: 0 }), Some(vec![4, 1, 2, 3]));
        assert_eq!(order(Edit::Move { from: 1, to: 3 }), Some(vec![1, 3, 2, 4]));
        assert_eq!(order(Edit::Move { from: 1, to: 1 }), None, "dropped where it was");
        assert_eq!(order(Edit::Move { from: 1, to: 2 }), None, "dropped where it was");
        assert_eq!(order(Edit::Move { from: 1, to: 9 }), None);
    }
}
