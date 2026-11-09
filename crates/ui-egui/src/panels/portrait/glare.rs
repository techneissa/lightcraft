//! The Portrait tab's section of the glare feature (`lightcraft_retouch::glare`, `fork/glare-repair.md`): how to
//! repair (FLUX.1 Fill repaints lenses, LaMa fills glints), Repair Glare for the picked face or every face, the
//! repair's state, and Amount, which blends the repair in.

/// The section's choices, kept for the app's run (not per photo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(super) struct Choice {
    pub flux: bool,
    /// `auto`, `whole` or `glints` (FLUX only).
    pub lenses: &'static str,
}

impl Default for Choice {
    fn default() -> Self {
        Choice { flux: true, lenses: "auto" }
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn choice_id() -> egui::Id {
    egui::Id::unique("portrait-glare-choice")
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const LENSES: [(&str, &str); 3] = [("auto", "Auto"), ("whole", "Whole Lenses"), ("glints", "Glints Only")];

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    #[cfg(not(target_arch = "wasm32"))]
    repair(cx);
    cx.slider("retouch.glare.amount");
}

#[cfg(not(target_arch = "wasm32"))]
fn repair(cx: &mut super::Ctx<'_>) {
    use serde_json::json;

    use crate::i18n::tr;
    use crate::widgets::register;

    let ui = &mut *cx.ui;
    let mut choice: Choice = ui.ctx().data(|d| d.get_temp(choice_id())).unwrap_or_default();
    let glare = cx.app.session.glare.clone();
    let running = glare.running();
    let busy = running.is_some();
    let here = running.as_ref().is_some_and(|r| r.photo == cx.photo);
    let flux_ready = glare.weights().is_ok();
    let lama_ready = cx.app.session.lama.installed();
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 2 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_sized([58.0, 22.0], egui::Label::new(tr("Method")));
            let label = |flux: bool| if flux { tr("Repaint (FLUX)") } else { tr("Fill Glints (LaMa)") };
            let r = crate::widgets::combo_box("portrait-glare-method").selected_text(label(choice.flux)).show_ui(ui, |ui| {
                for flux in [true, false] {
                    let r = ui.add_enabled(!busy, egui::Button::selectable(choice.flux == flux, label(flux)));
                    if r.clicked() {
                        choice.flux = flux;
                    }
                    register(ui, format!("portrait:glare:method:{}", if flux { "flux" } else { "lama" }), r.rect);
                }
            });
            crate::widgets::name_combo(&r.response, tr("Method"));
            register(ui, "portrait:glare:method", r.response.rect);
        });
        if choice.flux {
            ui.horizontal(|ui| {
                ui.add_sized([58.0, 22.0], egui::Label::new(tr("Lenses")));
                let label = |key: &str| tr(LENSES.iter().find(|(k, _)| *k == key).map_or("Auto", |(_, l)| l));
                let r = crate::widgets::combo_box("portrait-glare-lenses").selected_text(label(choice.lenses)).show_ui(ui, |ui| {
                    for (key, _) in LENSES {
                        let r = ui.add_enabled(!busy, egui::Button::selectable(choice.lenses == key, label(key)));
                        if r.clicked() {
                            choice.lenses = key;
                        }
                        register(ui, format!("portrait:glare:lenses:{key}"), r.rect);
                    }
                });
                crate::widgets::name_combo(&r.response, tr("Lenses"));
                register(ui, "portrait:glare:lenses", r.response.rect);
            });
        }
        ui.ctx().data_mut(|d| d.insert_temp(choice_id(), choice));
        let ready = if choice.flux { flux_ready } else { lama_ready };
        let faces = &cx.input.faces;
        let repaired = match cx.face {
            Some(i) => faces.get(i).is_some_and(|f| f.patch.is_some()),
            None => faces.iter().any(|f| f.patch.is_some()),
        };
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let r = ui.add_enabled(!busy && ready && !faces.is_empty(), egui::Button::new(tr("Repair Glare")));
            register(ui, "portrait:glare:repair", r.rect);
            if r.clicked() {
                let mut p = json!({"face": cx.face, "method": if choice.flux { "flux" } else { "lama" }});
                if choice.flux {
                    p["lenses"] = json!(choice.lenses);
                }
                if let Err(e) = cx.app.run("retouch.glareRepair", p) {
                    cx.app.toast(ui.ctx(), e);
                }
            }
            if repaired {
                let r = ui.add_enabled(!busy, egui::Button::new(tr("Remove Repair")));
                register(ui, "portrait:glare:remove", r.rect);
                if r.clicked()
                    && let Err(e) = cx.app.run("retouch.glareClear", json!({"face": cx.face}))
                {
                    cx.app.toast(ui.ctx(), e);
                }
            }
        });
        let t = crate::theme::Tokens::get(ui.ctx());
        let note = |ui: &mut egui::Ui, text: &str| {
            ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(t.text_dim)).wrap());
        };
        if here {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Repairing Glare…"));
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        } else if busy {
            note(ui, tr("Another photo's glare is being repaired."));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        } else if !ready {
            note(ui, if choice.flux { tr("FLUX.1 Fill isn't installed.") } else { tr("LaMa isn't installed.") });
        } else if let Some(m) = glare.message() {
            note(ui, &m);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use crate::headless::Headless;
    use crate::{LightcraftApp, Services};

    /// Opening the Glasses Glare section shows the method, Repair Glare and Amount; a bare session has no FLUX, so
    /// Repair Glare says FLUX is not installed and the command refuses with why.
    #[test]
    fn the_glare_section_has_its_choices_button_and_amount() {
        let t = Duration::from_secs(20);
        let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services { png: None, ..Default::default() });
        let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
        assert_eq!(h.request("ui.set", json!({"view": "detail"}), t)["ok"], true);
        assert_eq!(h.request("ui.clickWidget", json!({"id": "icon:portrait"}), t)["ok"], true);
        h.step();
        let r = h.request("engine.execute", json!({"command": "portrait.section", "params": {"section": "glare", "open": true}}), t);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        h.step();
        let shown = h.request("ui.widgets", json!({"filter": "portrait:glare"}), t);
        let ids: Vec<&str> = shown["result"].as_array().into_iter().flatten().filter_map(|w| w["id"].as_str()).collect();
        for id in ["portrait:glare:method", "portrait:glare:lenses", "portrait:glare:repair"] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        let slider = h.request("ui.widgets", json!({"filter": "slider:retouch.glare.amount"}), t);
        assert_eq!(slider["result"].as_array().map(Vec::len), Some(1), "{slider}");
        let r = h.request("engine.execute", json!({"command": "retouch.glareRepair", "params": {"method": "flux"}}), t);
        assert_eq!(r["ok"], false, "{r}");
        assert!(r.to_string().contains("not installed"), "{r}");
    }
}
