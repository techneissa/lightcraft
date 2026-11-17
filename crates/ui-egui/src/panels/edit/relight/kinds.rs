//! Fork (issue #300, `fork/relight.md`): a light's type in the Relight section: the menu on its row, and which
//! controls the selected light shows for its type (a spot's cone, a sun's direction and height, a softbox's size and
//! turn, a rim's width and side, a fill's bias towards near things).

use egui::{Align2, Rect, Sense, pos2};
use lightcraft_develop::relight::LightSettings;
use serde_json::json;

use super::{Icon, LightcraftApp, Tokens, register};

/// The types, in the order `relight.<n>.kind` counts them.
pub(super) const NAMES: [&str; 6] = ["Point", "Spot", "Sun / Window", "Softbox", "Rim", "Fill"];

/// The controls the selected light shows for its type `kind` (an index of [`NAMES`]), in order. (A spot's aim and a
/// sun's, a softbox's and a point's places are dragged on the photo; they are controls too, for agents and presets.)
pub(super) fn fields(kind: usize) -> &'static [&'static str] {
    match kind {
        1 => &["amount", "softness", "warmth", "spotAngle", "spotEdge"],
        2 => &["amount", "softness", "warmth", "sunAzimuth", "sunElevation"],
        3 => &["amount", "warmth", "boxWidth", "boxHeight", "boxRotation", "boxAimH", "boxAimV"],
        4 => &["amount", "warmth", "rimWidth", "rimAngle", "rimSide"],
        5 => &["amount", "softness", "warmth", "fillNear"],
        _ => &["amount", "softness", "warmth"],
    }
}

/// Whether the type has a depth (the Near–Far track): a sun, a rim and a fill have none.
pub(super) fn has_depth(kind: usize) -> bool {
    matches!(kind, 0 | 1 | 3)
}

/// Whether the type has Wrap (how far it wraps round shapes): a rim has none.
pub(super) fn has_wrap(kind: usize) -> bool {
    kind != 4
}

/// The type's menu on light `i`'s row `row`: its name and a chevron; a pick selects the light and sets its type.
pub(super) fn type_chip(app: &mut LightcraftApp, ui: &mut egui::Ui, row: Rect, i: usize, l: &LightSettings, sel: usize) {
    let t = Tokens::get(ui.ctx());
    let cur = l.kind.index();
    let chip = Rect::from_min_max(pos2(row.right() - 136.0, row.top() + 3.0), pos2(row.right() - 40.0, row.bottom() - 3.0));
    let resp = ui.interact(chip, ui.scope_id().with(("relightType", i)), Sense::click());
    register(ui, format!("relight:type:{i}"), chip);
    let name = crate::i18n::tr(NAMES.get(cur).copied().unwrap_or("Point"));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, format!("Light {} Type: {name}", i + 1)));
    let colour = if resp.hovered() { t.text } else { t.text_dim };
    let p = ui.painter();
    p.text(pos2(chip.right() - 16.0, chip.center().y), Align2::RIGHT_CENTER, name, t.font(11.0), colour);
    crate::icons::paint(p, Rect::from_center_size(pos2(chip.right() - 8.0, chip.center().y), egui::vec2(14.0, 14.0)), Icon::ChevronDown, colour);
    let mut picked = None;
    egui::Popup::menu(&resp).show(|ui| {
        for (k, n) in NAMES.iter().enumerate() {
            let r = ui.selectable_label(k == cur, crate::i18n::tr(n));
            register(ui, format!("relight:typeItem:{k}"), r.rect);
            if r.clicked() {
                picked = Some(k);
            }
        }
    });
    if let Some(k) = picked {
        if i != sel
            && let Err(e) = app.run("relight.selectLight", json!({"index": i}))
        {
            app.toast(ui.ctx(), e);
            return;
        }
        if k != cur
            && let Err(e) = app.run("develop.set", json!({"control": format!("relight.{i}.kind"), "value": k}))
        {
            app.toast(ui.ctx(), e);
        }
    }
}
