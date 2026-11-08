//! What follows the pointer while a tool strip icon is dragged: the tool's icon drawn like the
//! eye and Solo icons on a heading (an 18-point box, their stroke and colour), just right of the
//! pointer, its middle snapped to the middle of the row under the pointer, so it lines up with
//! that row's title and icons. Section drags (Edit panel, left side bar) show nothing beside the
//! pointer: their blue drop line is the only marker.

use egui::{Align2, Pos2, Rect, pos2, vec2};

use crate::icons::{Icon, paint};
use crate::theme::Tokens;

/// Side of the icon's box: the eye and Solo icons on an Edit heading.
const SIZE: f32 = 18.0;
/// Space between the pointer and the icon.
const OFFSET: f32 = 14.0;

/// The middle height of the row under height `y`: the row holding it, else the nearest one.
pub fn row_centre(rows: impl IntoIterator<Item = Rect>, y: f32) -> Option<f32> {
    let gap = |r: &Rect| {
        if y < r.top() {
            r.top() - y
        } else if y > r.bottom() {
            y - r.bottom()
        } else {
            0.0
        }
    };
    rows.into_iter().min_by(|a, b| gap(a).total_cmp(&gap(b))).map(|r| r.center().y)
}

/// Draw `icon` beside the pointer at `pointer`, centred on height `centre_y`.
pub fn show(ctx: &egui::Context, pointer: Pos2, centre_y: f32, icon: Icon) {
    let t = Tokens::get(ctx);
    egui::Area::new(egui::Id::unique("drag-ghost"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .pivot(Align2::LEFT_CENTER)
        .fixed_pos(pos2(pointer.x + OFFSET, centre_y))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(SIZE, SIZE), egui::Sense::hover());
            paint(ui.painter(), rect, icon, t.icon);
        });
}
