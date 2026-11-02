//! Fork feature (`fork/remove-panel-layout.md`): the Remove panel's compact controls. The four tools and the three
//! Find buttons are rows of equal tiles (an icon above the label) that fill the side bar's width. Labels keep 10.5 pt
//! (never under 9 pt); "Content-Aware" wraps to two lines in its tile. When four tool tiles no longer fit a row at
//! 9 pt (a long translation), the tools go to two rows of two.

use egui::{Align2, CornerRadius, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::icons::Icon;

use super::{LightcraftApp, Tokens, register};

const GAP: f32 = 4.0;
const PAD: f32 = 6.0;
const TILE_HEIGHT: f32 = 62.0;
/// Tile label size: 10.5 pt, and never under 9 pt.
const TILE_FONT: f32 = 10.5;
const TILE_MIN_FONT: f32 = 9.0;

/// The tool in use: Content-Aware, Heal or Clone when chosen, else Remove (what the photo does for any other tool).
pub(super) fn mode(app: &LightcraftApp) -> &'static str {
    match app.ui.tool.as_str() {
        "fill" => "fill",
        "heal" => "heal",
        "clone" => "clone",
        _ => "remove",
    }
}

/// Whether the tool in use takes a fill (Remove and Content-Aware; Heal and Clone copy a source).
pub(super) fn takes_fill(app: &LightcraftApp) -> bool {
    matches!(mode(app), "remove" | "fill")
}

/// The margins of the tile rows: nearly edge to edge.
pub(super) fn margin() -> egui::Margin {
    egui::Margin { left: 12, right: 12, top: 6, bottom: 6 }
}

/// A label on the lines it wraps to inside a tile: "Content-Aware" breaks at its hyphen, the rest stay on one line.
fn tile_lines(label: &str) -> Vec<&str> {
    label.split('-').collect()
}

/// The widest line among `labels` at `size`.
fn widest_line(ui: &egui::Ui, labels: &[&str], size: f32) -> f32 {
    let t = Tokens::get(ui.ctx());
    labels
        .iter()
        .flat_map(|l| tile_lines(crate::i18n::tr(l)))
        .map(|line| ui.painter().layout_no_wrap(line.to_string(), t.semibold(size), t.text).size().x)
        .fold(1.0, f32::max)
}

/// One row of equal tiles filling the width: an icon centred above the label, the same label size on every tile.
/// `items`: (id, label, hover text, active, enabled). Returns the clicked index.
fn tile_row(ui: &mut egui::Ui, items: &[(&str, &str, &str, bool, bool)], icons: &[Icon]) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let n = items.len().max(1) as f32;
    let each = ((ui.available_width() - GAP * (n - 1.0)) / n).floor().max(1.0);
    let labels: Vec<&str> = items.iter().map(|(_, l, ..)| *l).collect();
    let widest = widest_line(ui, &labels, TILE_FONT);
    let size = if widest <= each - 2.0 * PAD { TILE_FONT } else { (TILE_FONT * (each - 2.0 * PAD) / widest).clamp(TILE_MIN_FONT, TILE_FONT) };
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (i, (id, label, hover, active, enabled)) in items.iter().enumerate() {
            let sense = if *enabled { Sense::click() } else { Sense::hover() };
            let (r, resp) = ui.allocate_exact_size(vec2(each, TILE_HEIGHT), sense);
            let resp = resp.on_hover_text(crate::i18n::tr(hover));
            let name = crate::i18n::tr(label);
            crate::widgets::announce_selected(&resp, name.to_string(), *active);
            register(ui, format!("button:{id}"), r);
            let fill = if *active {
                t.pressed
            } else if *enabled && resp.hovered() {
                t.hover
            } else {
                t.button
            };
            let ink = if *enabled { t.text } else { t.text_dim };
            let border = if *active { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.button_border) };
            let p = ui.painter();
            p.rect(r, CornerRadius::same(4), fill, border, StrokeKind::Inside);
            if let Some(icon) = icons.get(i) {
                let icon_rect = egui::Rect::from_center_size(pos2(r.center().x, r.top() + 19.0), vec2(20.0, 20.0));
                crate::icons::paint(p, icon_rect, *icon, ink);
            }
            // the label area under the icon: one line centred, two lines stacked, room above and below
            let lines = tile_lines(name);
            let line_h = size + 2.0;
            let (area_top, area_bottom) = (r.top() + 34.0, r.bottom() - 5.0);
            let first = (area_top + area_bottom) / 2.0 - line_h * (lines.len() as f32 - 1.0) / 2.0;
            for (k, line) in lines.iter().enumerate() {
                p.text(pos2(r.center().x, first + line_h * k as f32), Align2::CENTER_CENTER, *line, t.semibold(size), ink);
            }
            if *enabled && resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Remove · Content-Aware · Heal · Clone as tiles; a click chooses the tool.
pub(super) fn tools(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let current = mode(app);
    let tools = [
        ("remove", "Remove", "Paint over a distraction to remove it"),
        ("fill", "Content-Aware", "Paint over an area to fill it from its surroundings"),
        ("heal", "Heal", "Paint a spot and blend in texture from a source"),
        ("clone", "Clone", "Paint a spot and copy pixels from a source"),
    ];
    let ids: Vec<String> = tools.iter().map(|(tool, ..)| format!("removeMode-{tool}")).collect();
    let items: Vec<_> = tools.iter().zip(&ids).map(|((tool, label, hover), id)| (id.as_str(), *label, *hover, *tool == current, true)).collect();
    let icons = [Icon::Eraser, Icon::SparkleFill, Icon::HealCross, Icon::CloneStamp];
    let labels = tools.map(|(_, l, _)| l);
    // four tiles in a row while the labels keep 9 pt or more, else two rows of two
    let clicked = if (ui.available_width() - 3.0 * GAP) / 4.0 - 2.0 * PAD >= widest_line(ui, &labels, TILE_MIN_FONT) {
        tile_row(ui, &items, &icons)
    } else {
        let (top, bottom) = items.split_at(2);
        tile_row(ui, top, &icons[..2]).or_else(|| tile_row(ui, bottom, &icons[2..]).map(|i| i + 2))
    };
    if let Some(i) = clicked
        && let Some((tool, ..)) = tools.get(i)
    {
        app.ui.tool = (*tool).into();
    }
}

/// The Find tiles (Dust, People, Wires) under a "Find" caption with the spot count at its right. `busy`: a search
/// still running. Returns the clicked search (0 Dust, 1 People, 2 Wires).
pub(super) fn find(ui: &mut egui::Ui, busy: [bool; 3], spots: usize) -> Option<usize> {
    let t = Tokens::get(ui.ctx());
    let items = [
        ("findDust", "Dust", "Heal the small, soft dark spots sensor dust leaves on smooth areas", false, !busy[0]),
        ("findPeople", "People", "Outline the people in the background; the main subject is left out", false, !busy[1]),
        ("findWires", "Wires", "Outline wires and cables across the photo", false, !busy[2]),
    ];
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Find")).size(11.5).color(t.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(spot_count(spots)).size(11.0).color(t.text_dim));
        });
    });
    tile_row(ui, &items, &[Icon::Search, Icon::Search, Icon::Search])
}

/// "6 Spots" / "1 Spot".
pub(super) fn spot_count(n: usize) -> String {
    if n == 1 { crate::i18n::tr_format!("{} Spot", n) } else { crate::i18n::tr_format!("{} Spots", n) }
}
