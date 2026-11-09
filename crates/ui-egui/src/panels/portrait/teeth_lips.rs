//! The Portrait tab's Teeth and Lips section (`lightcraft_retouch::teeth_lips`): Teeth Whitening, then Lip Color
//! (a colour swatch with egui's picker and a row of lipstick shades, then its amount), Lip Hue and Lip Saturation.
//! Like the sliders, the colour goes to every face or to the picked one.

use egui::{Color32, Sense, vec2};
use serde_json::json;

use crate::i18n::tr;
use crate::widgets::register;

/// The colour's three controls (sRGB 0…255).
const CHANNELS: [&str; 3] = ["retouch.teethLips.lipColorR", "retouch.teethLips.lipColorG", "retouch.teethLips.lipColorB"];

/// Lipstick shades offered next to the swatch: nude, rose, classic red, coral, berry, plum.
const SHADES: [[u8; 3]; 6] = [[176, 112, 100], [178, 52, 74], [170, 22, 36], [222, 92, 80], [140, 30, 70], [96, 34, 80]];

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    cx.slider("retouch.teethLips.teethWhitening");
    colour_row(cx);
    cx.slider("retouch.teethLips.lipColorAmount");
    cx.slider("retouch.teethLips.lipHue");
    cx.slider("retouch.teethLips.lipSaturation");
}

/// The value of control `id` for the faces the sliders change.
fn value(cx: &super::Ctx<'_>, id: &str) -> Option<f64> {
    match cx.face {
        Some(i) => cx.d.retouch.settings_for(&cx.faces, i).get(id),
        None => cx.d.retouch.global.get(id),
    }
}

fn colour_row(cx: &mut super::Ctx<'_>) {
    let defaults = lightcraft_develop::retouch::teeth_lips::DEFAULT_LIP_COLOR;
    let before: [u8; 3] = std::array::from_fn(|i| {
        let v = CHANNELS.get(i).and_then(|id| value(cx, id)).or(defaults.get(i).copied()).unwrap_or(0.0);
        v.clamp(0.0, 255.0).round() as u8
    });
    let mut picked = None;
    let mut c = before;
    let held = egui::Id::unique(("lc-lip-color-picking", cx.photo.0));
    let picking = cx.ui.data(|d| d.get_temp::<bool>(held)).unwrap_or(false);
    cx.ui.horizontal(|ui| {
        // (the sliders' rows keep 10 pt from the panel's edge)
        ui.add_space(10.0);
        let r = ui.color_edit_button_srgb(&mut c).on_hover_text(tr("Lip Color"));
        register(ui, "lipColorSwatch", r.rect);
        ui.add_space(4.0);
        for (i, shade) in SHADES.iter().enumerate() {
            let (rect, r) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            let fill = Color32::from_rgb(shade[0], shade[1], shade[2]);
            ui.painter().rect_filled(rect, 3.0, fill);
            if *shade == before {
                ui.painter().rect_stroke(rect.expand(1.5), 3.0, (1.5, ui.visuals().strong_text_color()), egui::StrokeKind::Outside);
            }
            register(ui, format!("lipShade.{i}"), rect);
            if r.clicked() {
                picked = Some(*shade);
            }
        }
    });
    if let Some(shade) = picked {
        // a shade is one undo step, and turns Lip Color on when it was off
        let _ = cx.app.run("develop.beginInteraction", json!({"label": "Lip Color"}));
        set_colour(cx, shade);
        if value(cx, "retouch.teethLips.lipColorAmount").unwrap_or(0.0) <= 0.0 {
            let face = cx.face;
            let _ = cx.app.run("retouch.set", json!({"control": "retouch.teethLips.lipColorAmount", "value": 60, "face": face}));
        }
        let _ = cx.app.run("develop.endInteraction", json!({}));
    } else if c != before {
        // a drag in the picker is one undo step
        if !picking {
            let _ = cx.app.run("develop.beginInteraction", json!({"label": "Lip Color"}));
            cx.ui.data_mut(|d| d.insert_temp(held, true));
        }
        set_colour(cx, c);
    } else if picking && !cx.ui.input(|i| i.pointer.any_down()) {
        let _ = cx.app.run("develop.endInteraction", json!({}));
        cx.ui.data_mut(|d| d.remove::<bool>(held));
    }
}

fn set_colour(cx: &mut super::Ctx<'_>, c: [u8; 3]) {
    let face = cx.face;
    for (id, v) in CHANNELS.iter().zip(c) {
        let _ = cx.app.run("retouch.set", json!({"control": id, "value": v, "face": face}));
    }
}
