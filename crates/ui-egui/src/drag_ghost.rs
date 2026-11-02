//! What follows the pointer while a section or a tool strip icon is dragged: an icon drawn like
//! the eye and Solo icons on a heading (an 18-point box, their stroke and colour), just right of
//! the pointer, its middle snapped to the middle of the row under the pointer, so it lines up
//! with that row's title and icons. The tool strip shows the tool's own icon; the Edit panel and
//! the left side bar show a grip (≡). The blue drop line stays the place marker.

use egui::{Align2, Pos2, Rect, Stroke, pos2, vec2};

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

/// Draw the ghost for a drag with the pointer at `pointer`, centred on height `centre_y`: `icon`
/// is the tool's own, `None` the grip.
pub fn show(ctx: &egui::Context, pointer: Pos2, centre_y: f32, icon: Option<Icon>) {
    let t = Tokens::get(ctx);
    egui::Area::new(egui::Id::unique("drag-ghost"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .pivot(Align2::LEFT_CENTER)
        .fixed_pos(pos2(pointer.x + OFFSET, centre_y))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(SIZE, SIZE), egui::Sense::hover());
            match icon {
                Some(icon) => paint(ui.painter(), rect, icon, t.icon),
                None => grip(ui.painter(), rect, t.icon),
            }
        });
}

/// Three bars, one above the other, on the icons' 20-point grid scaled to the box, with their
/// 1.5-point stroke and rounded ends.
fn grip(p: &egui::Painter, rect: Rect, color: egui::Color32) {
    let k = rect.width() / 20.0;
    let stroke = Stroke::new((1.5 * k).max(1.0), color);
    let c = rect.center();
    for dy in [-4.5, 0.0, 4.5] {
        let (a, b) = (pos2(c.x - 6.0 * k, c.y + dy * k), pos2(c.x + 6.0 * k, c.y + dy * k));
        p.line_segment([a, b], stroke);
        p.circle_filled(a, stroke.width / 2.0, color);
        p.circle_filled(b, stroke.width / 2.0, color);
    }
}
