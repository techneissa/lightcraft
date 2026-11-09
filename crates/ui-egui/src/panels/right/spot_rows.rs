//! Fork feature (`fork/spot-rows.md`): one row per Remove, Content-Aware, Heal or Clone spot in the Remove panel, laid
//! out like the submask rows of the Masks panel. A click selects the spot; at the right, a menu for when its circle
//! shows on the photo (Use Global, Auto, Always, Selected, Never), an eye that hides or shows its effect, and a trash
//! can that deletes it. Every row has its own id scope (#68).

use egui::{Rect, Sense, Ui, pos2, vec2};
use lightcraft_develop::{Spot, SpotCircle, SpotMode};
use serde_json::json;

use super::{LightcraftApp, Tokens, divider, register};
use crate::icons::{Icon, paint};
use crate::state::ToolOverlay;

const ROW_H: f32 = 28.0;
const BUTTON_W: f32 = 22.0;
const MENU_W: f32 = 36.0;
/// The list scrolls inside this height, so Find Dust or Find People adding dozens of spots keeps the panel usable.
const LIST_MAX_H: f32 = ROW_H * 7.0;

/// The tool's name as the row shows it.
fn tool_name(mode: SpotMode) -> &'static str {
    match mode {
        SpotMode::Remove => "Remove",
        SpotMode::Fill => "Content-Aware",
        SpotMode::Heal => "Heal",
        SpotMode::Clone => "Clone",
    }
}

/// The name of spot `i`: its tool and its number among the spots of that tool ("Heal 2"). `None` past the end.
pub(crate) fn spot_name(spots: &[Spot], i: usize) -> Option<String> {
    let spot = spots.get(i)?;
    let mode = spot.mode;
    let n = spots.iter().take(i + 1).filter(|s| s.mode == mode).count();
    // a spot in an object's or a mask's shape says what it is ("Remove 3 · Object", `fork/object-remove.md`)
    match &spot.area {
        Some(a) => Some(format!("{} {n} · {}", crate::i18n::tr(tool_name(mode)), crate::i18n::tr(a.label()))),
        None => Some(format!("{} {n}", crate::i18n::tr(tool_name(mode)))),
    }
}

/// The Tool Overlay setting one spot's circle follows: its own, else the global one.
pub(crate) fn overlay_of(spot: &Spot, global: ToolOverlay) -> ToolOverlay {
    match spot.circle {
        Some(SpotCircle::Auto) => ToolOverlay::Auto,
        Some(SpotCircle::Always) => ToolOverlay::Always,
        Some(SpotCircle::Selected) => ToolOverlay::Selected,
        Some(SpotCircle::Never) => ToolOverlay::Never,
        None => global,
    }
}

fn circle_label(c: Option<SpotCircle>) -> &'static str {
    match c {
        None => "Use Global",
        Some(SpotCircle::Auto) => "Auto",
        Some(SpotCircle::Always) => "Always",
        Some(SpotCircle::Selected) => "Selected",
        Some(SpotCircle::Never) => "Never",
    }
}

/// A lasso made from a stroke, which Make Stroke can undo.
fn is_lasso(spot: &Spot) -> bool {
    spot.area.as_ref().is_some_and(|a| a.kind == "lasso" && !a.stroke.is_empty())
}

/// The rows, under a divider; nothing when the photo has no spot.
pub(super) fn section(app: &mut LightcraftApp, ui: &mut Ui, spots: &[Spot]) {
    if spots.is_empty() {
        return;
    }
    divider(ui);
    let active = app.session.active_spot.filter(|i| *i < spots.len());
    // a spot picked on the photo brings its row into view (once per change, so scrolling by hand stays put)
    let seen = egui::Id::unique("spotRowsSeen");
    let changed = ui.ctx().data_mut(|d| {
        let last = d.get_temp::<Option<usize>>(seen);
        d.insert_temp(seen, active);
        last != Some(active)
    });
    egui::Frame::NONE.inner_margin(egui::Margin { left: 14, right: 12, top: 6, bottom: 6 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        egui::ScrollArea::vertical().id_salt("spotRows").max_height(LIST_MAX_H).auto_shrink([false, true]).show(ui, |ui| {
            for i in 0..spots.len() {
                ui.push_id(("spotRow", i), |ui| row(app, ui, spots, i, active == Some(i), changed));
            }
        });
    });
}

fn row(app: &mut LightcraftApp, ui: &mut Ui, spots: &[Spot], i: usize, sel: bool, scroll_to: bool) {
    let Some(spot) = spots.get(i) else { return };
    let t = Tokens::get(ui.ctx());
    let name = spot_name(spots, i).unwrap_or_default();
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    crate::widgets::announce_selected(&resp, name.clone(), sel);
    register(ui, format!("spotRow:{i}"), r);
    if sel && scroll_to {
        ui.scroll_to_rect(r, None);
    }
    let hot = sel || ui.rect_contains_pointer(r);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(
        r,
        4.0,
        if sel {
            t.tool_active
        } else if hot {
            t.hover.gamma_multiply(0.7)
        } else {
            t.inset
        },
    );
    let icon = match spot.mode {
        SpotMode::Remove => Icon::Eraser,
        SpotMode::Fill => Icon::SparkleFill,
        SpotMode::Heal => Icon::HealCross,
        SpotMode::Clone => Icon::CloneStamp,
    };
    let color = if spot.hidden { t.text_disabled } else { t.text };
    paint(
        ui.painter(),
        Rect::from_min_size(r.min + vec2(6.0, (ROW_H - 16.0) / 2.0), vec2(16.0, 16.0)),
        icon,
        if spot.hidden { color } else { t.text_label },
    );
    let extra = if is_lasso(spot) { BUTTON_W + 2.0 } else { 0.0 };
    let room = (r.width() - 28.0 - (BUTTON_W * 2.0 + MENU_W + 8.0 + extra)).max(0.0);
    let g = crate::widgets::one_line(ui.painter(), &name, t.font(13.0), color, room);
    let elided = g.elided;
    ui.painter().galley(pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g, color);
    let resp = if elided { resp.on_hover_text(&name) } else { resp };

    let used = buttons(app, ui, spot, i, r, hot);
    if resp.clicked() && !used {
        let _ = app.run("spot.select", json!({"index": i}));
    }
}

/// The circle menu, the eye and the trash can at the right of row `i` (`r`); whether one was pressed (the row must not
/// also be selected).
fn buttons(app: &mut LightcraftApp, ui: &mut Ui, spot: &Spot, i: usize, r: Rect, hot: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let dim = if hot { t.text } else { t.text_dim };
    let mut used = false;
    let mut x = r.right() - 4.0;
    let mut button = |ui: &mut Ui, key: &str, name: &str, tip: &str, w: f32, active: bool, draw: &dyn Fn(&egui::Painter, Rect, egui::Color32)| {
        let br = Rect::from_min_size(pos2(x - w, r.center().y - 11.0), vec2(w, 22.0));
        x -= w + 2.0;
        register(ui, format!("{key}:{i}"), br);
        let resp = ui.interact(br, ui.scope_id().with(key), Sense::click());
        crate::widgets::announce_selected(&resp, crate::i18n::tr(name), active);
        let resp = resp.on_hover_text(crate::i18n::tr(tip));
        if active {
            ui.painter().rect(br, 3.0, t.pressed, egui::Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(br, 3.0, t.hover);
        }
        draw(ui.painter(), br, if resp.hovered() || active { t.text } else { dim });
        resp
    };
    if button(ui, "spotDelete", "Delete Spot", "Delete Spot", BUTTON_W, false, &|p, br, col| paint(p, br.shrink(3.0), Icon::Trash, col)).clicked() {
        let _ = app.run("spot.delete", json!({"index": i}));
        used = true;
    }
    let (eye_name, eye_tip) = if spot.hidden { ("Show Spot", "Show Spot") } else { ("Hide Spot", "Hide Spot") };
    if button(ui, "spotVisible", eye_name, eye_tip, BUTTON_W, false, &|p, br, col| {
        paint(p, br.shrink(3.0), if spot.hidden { Icon::EyeOff } else { Icon::Eye }, col)
    })
    .clicked()
    {
        let _ = app.run("spot.visible", json!({"index": i, "visible": spot.hidden}));
        used = true;
    }
    let now = spot.circle;
    // a lasso (`fork/brush-area.md`) can go back to the plain stroke it came from
    if is_lasso(spot)
        && button(ui, "spotStroke", "Make Stroke", "Make Stroke: back to the plain stroke, without its inside", BUTTON_W, false, &|p, br, col| {
            paint(p, br.shrink(3.0), Icon::Brush, col)
        })
        .clicked()
    {
        if let Err(e) = app.run("spot.makeStroke", json!({"index": i})) {
            app.toast_error(ui.ctx(), e);
        }
        used = true;
    }
    let menu = button(ui, "spotCircle", "Circle on Photo", "When the circle shows on the photo", MENU_W, now.is_some(), &|p, br, col| {
        paint(p, Rect::from_min_size(br.min + vec2(2.0, 0.0), vec2(22.0, br.height())).shrink(4.0), Icon::Circle, col);
        paint(p, Rect::from_center_size(pos2(br.right() - 8.0, br.center().y), vec2(10.0, 10.0)), Icon::ChevronDown, col);
    });
    used |= menu.clicked();
    egui::Popup::menu(&menu).show(|ui| {
        let choices: Vec<Option<SpotCircle>> = std::iter::once(None).chain(SpotCircle::ALL.into_iter().map(Some)).collect();
        for c in choices {
            let (row, resp) = ui.allocate_exact_size(vec2(130.0, 24.0), Sense::click());
            register(ui, format!("spotCircle:{i}:{}", c.map_or("global", SpotCircle::name)), row);
            if resp.hovered() || c == now {
                ui.painter().rect_filled(row, 3.0, if resp.hovered() { t.hover } else { t.pressed });
            }
            ui.painter().text(
                pos2(row.left() + 8.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                crate::i18n::tr(circle_label(c)),
                t.font(12.5),
                t.text,
            );
            if resp.clicked() {
                if c != now {
                    let _ = app.run("spot.circle", json!({"index": i, "mode": c.map_or("global", SpotCircle::name)}));
                }
                ui.close();
            }
        }
    });
    used
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(mode: SpotMode) -> Spot {
        Spot { mode, ..Default::default() }
    }

    #[test]
    fn a_spot_is_named_by_its_tool_and_its_number_among_that_tool() {
        let spots = [spot(SpotMode::Heal), spot(SpotMode::Clone), spot(SpotMode::Heal), spot(SpotMode::Fill)];
        let names: Vec<_> = (0..5).map(|i| spot_name(&spots, i)).collect();
        assert_eq!(names, vec![Some("Heal 1".into()), Some("Clone 1".into()), Some("Heal 2".into()), Some("Content-Aware 1".into()), None]);
    }

    #[test]
    fn a_spots_own_circle_setting_beats_the_global_one() {
        let own = Spot { circle: Some(SpotCircle::Never), ..Default::default() };
        assert_eq!(overlay_of(&own, ToolOverlay::Always), ToolOverlay::Never);
        assert_eq!(overlay_of(&Spot::default(), ToolOverlay::Always), ToolOverlay::Always, "Use Global follows it");
    }
}
