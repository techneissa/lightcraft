//! The divider of the Before/After split views (View ▸ Before/After Split, Shift+Y, and Split
//! Top/Bottom): the line between Before and After that can be dragged across the photo, with a
//! round grip on it and the two side labels.
//!
//! The position is a fraction of the part of the photo on screen (`UiState::split_at`, shared by
//! both split views, the middle until it is moved), so it follows zoom, pan and resizing. Double-click
//! on the line puts it back in the middle.

use egui::{Align2, Color32, CursorIcon, Id, Painter, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::LightcraftApp;
use crate::state::BeforeAfter;
use crate::theme::Tokens;
use crate::widgets::register;

/// Where the line is until it is moved: the middle of the photo on screen.
pub const MIDDLE: f32 = 0.5;
/// How far from the line (points) it can be grabbed.
const GRAB: f32 = 6.0;
/// Radius of the round grip on the line.
const GRIP_RADIUS: f32 = 13.0;

/// The fraction a stored position stands for: the middle when unset or not a number, else
/// clamped to the photo.
pub fn position(stored: Option<f32>) -> f32 {
    stored.filter(|v| v.is_finite()).map_or(MIDDLE, |v| v.clamp(0.0, 1.0))
}

/// The fraction of `lo..hi` that `pointer` is at, clamped to 0..=1; `None` for an empty or
/// non-finite span or pointer.
pub fn fraction_at(pointer: f32, lo: f32, hi: f32) -> Option<f32> {
    let span = hi - lo;
    (span.is_finite() && span > 0.0 && pointer.is_finite()).then(|| ((pointer - lo) / span).clamp(0.0, 1.0))
}

/// The divider for this frame: where the line is, and (in a split view) how to draw its grip.
pub(super) struct Divider {
    /// The line's position as a fraction of the photo on screen.
    pub at: f32,
    /// Where the line crosses (`x` for Before/After Split, `y` for Split Top/Bottom).
    pub line: Pos2,
    split: Option<Split>,
}

/// What [`Divider::paint`] needs of a split view.
struct Split {
    /// The line is vertical (Before on its left), else horizontal (Before above).
    vertical: bool,
    /// The part of the photo on screen.
    shown: Rect,
    /// The pointer is on the line, or dragging it.
    hot: bool,
}

/// Lay out the divider over `img` (the photo, `canvas` what is on screen) and take its input:
/// a drag moves the line, a double-click puts it back in the middle. Registered where the caller
/// is in the frame (the drag wins over the loupe's own pan, zoom and tools there), and before the
/// line is used, so the frame already shows the new position.
pub(super) fn interact(app: &mut LightcraftApp, ui: &mut egui::Ui, canvas: Rect, img: Rect) -> Divider {
    let mode = app.ui.before_after;
    let shown = img.intersect(canvas);
    let vertical = mode == BeforeAfter::Split;
    if !(vertical || mode == BeforeAfter::SplitTopBottom) || !shown.is_positive() {
        let line = crate::region::wipe_line_at(img, canvas, MIDDLE);
        return Divider { at: MIDDLE, line, split: None };
    }
    let mut at = position(app.ui.split_at);
    let line = crate::region::wipe_line_at(img, canvas, at);
    let (strip, grip_centre) = if vertical {
        (Rect::from_min_max(pos2(line.x - GRAB, shown.top()), pos2(line.x + GRAB, shown.bottom())), pos2(line.x, shown.center().y))
    } else {
        (Rect::from_min_max(pos2(shown.left(), line.y - GRAB), pos2(shown.right(), line.y + GRAB)), pos2(shown.center().x, line.y))
    };
    let grip = Rect::from_center_size(grip_centre, vec2(2.0 * GRIP_RADIUS, 2.0 * GRIP_RADIUS));
    register(ui, "canvas:splitDivider", grip);
    let strip_resp = ui.interact(strip, Id::unique("split-divider-line"), Sense::click_and_drag());
    let grip_resp = ui.interact(grip, Id::unique("split-divider-grip"), Sense::click_and_drag());
    crate::widgets::announce(&grip_resp, crate::i18n::tr("Before/After divider"));
    let dragging = strip_resp.dragged() || grip_resp.dragged();
    let hot = dragging || strip_resp.hovered() || grip_resp.hovered();
    if hot {
        ui.ctx().set_cursor_icon(if vertical { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
    }
    if strip_resp.double_clicked() || grip_resp.double_clicked() {
        app.ui.split_at = None;
        at = MIDDLE;
    } else if dragging && let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
        let (v, lo, hi) = if vertical { (p.x, shown.left(), shown.right()) } else { (p.y, shown.top(), shown.bottom()) };
        if let Some(f) = fraction_at(v, lo, hi) {
            app.ui.split_at = Some(f);
            at = f;
        }
    }
    let line = crate::region::wipe_line_at(img, canvas, at);
    Divider { at, line, split: Some(Split { vertical, shown, hot }) }
}

impl Divider {
    /// The grip on the line and the Before / After labels, over everything the loupe draws.
    pub(super) fn paint(&self, p: &Painter, t: &Tokens) {
        let Some(Split { vertical, shown, hot }) = &self.split else { return };
        let (vertical, shown, hot) = (*vertical, *shown, *hot);
        let p = p.with_clip_rect(p.clip_rect().intersect(shown.expand(GRIP_RADIUS + 2.0)));
        // the grip: a round disc on the line with an arrow to each side the line can move
        let centre = if vertical { pos2(self.line.x, shown.center().y) } else { pos2(shown.center().x, self.line.y) };
        let fill = if hot { Color32::WHITE } else { Color32::from_white_alpha(235) };
        p.circle_filled(centre + vec2(0.0, 1.0), GRIP_RADIUS, Color32::from_black_alpha(70));
        p.circle(centre, GRIP_RADIUS, fill, Stroke::new(1.0, Color32::from_black_alpha(150)));
        let (a, b) = if vertical { (vec2(1.0, 0.0), vec2(0.0, 1.0)) } else { (vec2(0.0, 1.0), vec2(1.0, 0.0)) };
        let ink = Stroke::new(1.8, Color32::from_gray(40));
        for dir in [-1.0f32, 1.0] {
            let tip = centre + a * (dir * 7.0);
            p.line_segment([tip, tip - a * (dir * 3.5) + b * 4.0], ink);
            p.line_segment([tip, tip - a * (dir * 3.5) - b * 4.0], ink);
        }
        // the labels: Before / After in the corners of the photo on screen, each only while its
        // side of the line has room for it
        let font = t.font(12.0);
        let label = |text: &str, corner: Pos2, anchor: Align2, side: Rect| {
            let g = p.layout_no_wrap(crate::i18n::tr(text).to_string(), font.clone(), Color32::WHITE);
            let size = g.size() + vec2(16.0, 8.0);
            let min = match anchor {
                Align2::RIGHT_BOTTOM => corner - size,
                Align2::RIGHT_TOP => corner - vec2(size.x, 0.0),
                Align2::LEFT_BOTTOM => corner - vec2(0.0, size.y),
                _ => corner,
            };
            let bg = Rect::from_min_size(min, size);
            if side.contains_rect(bg) {
                p.rect_filled(bg, 4.0, Color32::from_black_alpha(160));
                p.galley(bg.min + vec2(8.0, 4.0), g, Color32::WHITE);
            }
        };
        let inset = vec2(10.0, 10.0);
        if vertical {
            let before = Rect::from_min_max(shown.min, pos2(self.line.x, shown.max.y));
            let after = Rect::from_min_max(pos2(self.line.x, shown.min.y), shown.max);
            label("Before", pos2(shown.left() + inset.x, shown.bottom() - inset.y), Align2::LEFT_BOTTOM, before);
            label("After", pos2(shown.right() - inset.x, shown.bottom() - inset.y), Align2::RIGHT_BOTTOM, after);
        } else {
            let before = Rect::from_min_max(shown.min, pos2(shown.max.x, self.line.y));
            let after = Rect::from_min_max(pos2(shown.min.x, self.line.y), shown.max);
            label("Before", pos2(shown.right() - inset.x, shown.top() + inset.y), Align2::RIGHT_TOP, before);
            label("After", pos2(shown.right() - inset.x, shown.bottom() - inset.y), Align2::RIGHT_BOTTOM, after);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_position_is_clamped_to_the_photo_and_defaults_to_the_middle() {
        assert_eq!(position(None), MIDDLE);
        assert_eq!(position(Some(f32::NAN)), MIDDLE);
        assert_eq!(position(Some(f32::INFINITY)), MIDDLE);
        assert_eq!(position(Some(-0.3)), 0.0);
        assert_eq!(position(Some(1.7)), 1.0);
        assert_eq!(position(Some(0.25)), 0.25);
    }

    #[test]
    fn the_pointer_fraction_is_clamped_and_ignores_an_empty_span() {
        assert_eq!(fraction_at(150.0, 100.0, 300.0), Some(0.25));
        assert_eq!(fraction_at(0.0, 100.0, 300.0), Some(0.0));
        assert_eq!(fraction_at(900.0, 100.0, 300.0), Some(1.0));
        assert_eq!(fraction_at(150.0, 100.0, 100.0), None);
        assert_eq!(fraction_at(f32::NAN, 100.0, 300.0), None);
        assert_eq!(fraction_at(150.0, 100.0, f32::INFINITY), None);
    }
}
