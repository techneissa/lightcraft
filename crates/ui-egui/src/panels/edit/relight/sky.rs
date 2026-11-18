//! Fork (issue #330, `fork/relight.md`): the Sky Rays group of the Relight section, for a selected Sun / Window light: its
//! rays in the sky (`relight.<n>.skyRays`) and their shape (`.rayCount`, `.rayLength`, `.rayWidth`, `.rayVariation`,
//! `.rayDirection`, `.raySpread`, `.raySoftness`, `.rayBlocking`), its soft rays opposite (`.oppositeRays`) and where they
//! centre (`.raysPlaced`: found from the sky, or placed with `.raysX` and `.raysY`). A photo without a sky says so.

use lightcraft_develop::DevelopSettings;
use serde_json::json;

use super::super::{Icon, LightcraftApp, Tokens, control, register};

/// The group: a flyout row "Sky Rays" under a sun's controls.
pub(super) fn sky(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, sel: usize, made: Option<&lightcraft_engine::relight::Made>) {
    let Some(l) = d.relight.lights.get(sel) else { return };
    if l.kind.index() != 2 {
        return;
    }
    let open = app.ui.flyout_open("relightSky");
    if crate::widgets::flyout_row(ui, "relightSky", "Sky Rays", Icon::Cloud, open).clicked() {
        app.ui.toggle_flyout("relightSky");
    }
    if !open {
        return;
    }
    let e = l.effects;
    control(app, ui, d, &format!("relight.{sel}.skyRays"), true);
    // (the rays' shape, while they show)
    if e.sky_rays > 0.0 {
        for c in ["rayCount", "rayLength", "rayWidth", "rayVariation", "rayDirection", "raySpread", "raySoftness", "rayBlocking"] {
            control(app, ui, d, &format!("relight.{sel}.{c}"), true);
        }
    }
    control(app, ui, d, &format!("relight.{sel}.oppositeRays"), true);
    if e.sky_rays <= 0.0 && e.opposite_rays <= 0.0 {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let margin = egui::Margin { left: 24, right: 22, top: 2, bottom: 4 };
    // (a photo without a sky: the rays change nothing, and the group says why; known once a render has looked)
    if made.and_then(|m| m.input.field.sky_known()) == Some(false) {
        egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
            let r = ui.label(
                egui::RichText::new(crate::i18n::tr("No sky found in this photo, so the rays change nothing.")).size(11.5).color(t.text_label),
            );
            register(ui, "relight:sky:none", r.rect);
        });
        return;
    }
    let items = [("Brightest Sky", "auto"), ("Placed", "placed")];
    let tips = [
        "Centre the rays on the brightest round spot of the sky (Sky Rays) or the middle of the horizon (Opposite Rays)",
        "Centre the rays where the two sliders below, or the sun handle on the photo, put them",
    ];
    let placed = e.rays_placed >= 0.5;
    egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
        if let Some(i) = crate::widgets::segmented_tips(ui, "relightRaysCentre", &items, &tips, Some(usize::from(placed)), 2) {
            let mut values = serde_json::Map::new();
            values.insert(format!("relight.{sel}.raysPlaced"), json!(i));
            // (placing starts where the rays are now: the centre found from the sky, once a render has found it)
            let rays = if e.sky_rays > 0.0 { lightcraft_relight::Rays::Sun } else { lightcraft_relight::Rays::Opposite };
            if i == 1
                && !placed
                && let Some((x, y)) = made.and_then(|m| m.input.field.rays_centre_known(rays))
            {
                values.insert(format!("relight.{sel}.raysX"), json!((f64::from(x) * 1000.0).round() / 1000.0));
                values.insert(format!("relight.{sel}.raysY"), json!((f64::from(y) * 1000.0).round() / 1000.0));
            }
            if let Err(err) = app.run("develop.set", json!({"values": values})) {
                app.toast(ui.ctx(), err);
            }
        }
    });
    if placed {
        control(app, ui, d, &format!("relight.{sel}.raysX"), true);
        control(app, ui, d, &format!("relight.{sel}.raysY"), true);
    } else {
        // (which point the sky gave: the sun handle on the photo shows it too)
        let rays = if e.sky_rays > 0.0 { lightcraft_relight::Rays::Sun } else { lightcraft_relight::Rays::Opposite };
        if let Some((x, y)) = made.and_then(|m| m.input.field.rays_centre_known(rays)) {
            let what =
                if rays == lightcraft_relight::Rays::Sun { "the brightest open spot of the sky" } else { "a little below the middle of the horizon" };
            let text = crate::i18n::tr_format!(
                "Centred on {what}, {x} % across and {y} % down (the sun handle on the photo).",
                what = crate::i18n::tr(what),
                x = (f64::from(x) * 100.0).round() as i64,
                y = (f64::from(y) * 100.0).round() as i64
            );
            // (a pick the sky doesn't make clear, a sunlit cloud as likely as the sun: ask for the sun to be placed)
            let unsure = rays == lightcraft_relight::Rays::Sun && made.and_then(|m| m.input.field.rays_centre_sure()) == Some(false);
            let text = if unsure { format!("{text} {}", crate::i18n::tr("Drag the sun to place it.")) } else { text };
            egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
                let r = ui.label(egui::RichText::new(text).size(11.5).color(t.text_label));
                register(ui, "relight:sky:centre", r.rect);
            });
        }
    }
}
