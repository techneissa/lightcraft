//! What follows the pointer while a section or a tool strip icon is dragged: a small icon, just
//! right of the pointer and centred on it, drawn faint so the drop line stays the place marker.
//! The tool strip shows the tool's own icon; the Edit panel and the left side bar show a grip (≡).

use egui::{Align2, Pos2, Rect, Stroke, vec2};

use crate::icons::{Icon, paint};
use crate::theme::Tokens;

/// How see-through the icon is.
const OPACITY: f32 = 0.7;
/// Side of the icon's box.
const SIZE: f32 = 20.0;
/// Space between the pointer and the icon.
const OFFSET: f32 = 14.0;

/// Draw the ghost for a drag at `pointer`: `icon` is the tool's own, `None` the grip.
pub fn show(ctx: &egui::Context, pointer: Pos2, icon: Option<Icon>) {
    let t = Tokens::get(ctx);
    egui::Area::new(egui::Id::unique("drag-ghost"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .pivot(Align2::LEFT_CENTER)
        .fixed_pos(pointer + vec2(OFFSET, 0.0))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(SIZE, SIZE), egui::Sense::hover());
            let color = t.text.gamma_multiply(OPACITY);
            match icon {
                Some(icon) => paint(ui.painter(), rect, icon, color),
                None => grip(ui.painter(), rect, color),
            }
        });
}

/// Three short bars, one above the other.
fn grip(p: &egui::Painter, rect: Rect, color: egui::Color32) {
    let c = rect.center();
    for dy in [-5.0, 0.0, 5.0] {
        p.line_segment([egui::pos2(c.x - 7.0, c.y + dy), egui::pos2(c.x + 7.0, c.y + dy)], Stroke::new(2.0, color));
    }
}
