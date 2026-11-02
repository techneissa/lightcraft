//! Dragging an Edit panel section by its heading, as the left sidebar's sections move: a line
//! shows where it would go, releasing over the panel moves it there (`view.editSectionMove`),
//! releasing elsewhere or Esc leaves it. A click on a heading still opens or folds its section.

use egui::{Rect, pos2};
use serde_json::{Value, json};

use super::EditSection;
use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// The headings drawn this frame, kept in the context's temporary data (keyed by this type).
#[derive(Clone, Default)]
struct Headers(Vec<(EditSection, Rect)>);

fn headers_id() -> egui::Id {
    egui::Id::NULL
}

/// Forget last frame's headings; the panel calls it before it draws its sections.
pub fn begin_frame(ui: &egui::Ui) {
    ui.data_mut(|d| d.insert_temp(headers_id(), Headers::default()));
}

/// A section's heading was drawn at `rect` (its place for the drop line).
pub fn note_header(ui: &egui::Ui, id: &str, rect: Rect) {
    let Some(section) = EditSection::from_id(id) else { return };
    ui.data_mut(|d| d.get_temp_mut_or_default::<Headers>(headers_id()).0.push((section, rect)));
}

/// A press at the right end of a heading belongs to its buttons (Solo, on / off), not to a drag.
pub fn on_header_buttons(ui: &egui::Ui, header: Rect) -> bool {
    ui.input(|i| i.pointer.press_origin()).is_some_and(|p| p.x > header.right() - 70.0)
}

/// Above the first heading whose middle is below `y`, or last (`None`).
pub fn drop_before(placed: &[(EditSection, Rect)], y: f32) -> Option<EditSection> {
    placed.iter().find(|(_, r)| y < r.center().y).map(|(s, _)| *s)
}

/// Run by the panel after it drew its sections.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let Some(dragged) = app.ui.dragging_edit_section else { return };
    let placed: Vec<(EditSection, Rect)> = ui.data(|d| d.get_temp::<Headers>(headers_id())).unwrap_or_default().0;
    let (down, released, pos, esc) =
        ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_released(), i.pointer.latest_pos(), i.key_pressed(egui::Key::Escape)));
    if esc {
        crate::widgets::take_escape(ui.ctx());
        app.ui.dragging_edit_section = None;
        return;
    }
    if (!down && !released) || placed.is_empty() {
        app.ui.dragging_edit_section = None;
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let view = ui.clip_rect();
    let Some(pos) = pos.filter(|p| view.contains(*p)) else {
        if released {
            app.ui.dragging_edit_section = None;
        }
        return;
    };
    let before = drop_before(&placed, pos.y);
    if released {
        app.ui.dragging_edit_section = None;
        let before = before.map_or(Value::Null, |b| json!(b.id()));
        if let Err(e) = app.run("view.editSectionMove", json!({"section": dragged.id(), "before": before})) {
            app.toast(ui.ctx(), e);
        }
        return;
    }
    let t = Tokens::get(ui.ctx());
    let y = match before {
        Some(b) => placed.iter().find(|(s, _)| *s == b).map(|(_, r)| r.top()),
        None => Some(ui.min_rect().bottom()),
    };
    if let Some(y) = y {
        let line = Rect::from_min_max(pos2(view.left() + 8.0, y - 1.0), pos2(view.right() - 8.0, y + 1.0));
        ui.painter().rect_filled(line, 1.0, t.accent);
        register(ui, "editSectionDrop", line);
    }
    if let Some(y) = crate::drag_ghost::row_centre(placed.iter().map(|(_, r)| *r), pos.y) {
        crate::drag_ghost::show(ui.ctx(), pos, y, None);
    }
}
