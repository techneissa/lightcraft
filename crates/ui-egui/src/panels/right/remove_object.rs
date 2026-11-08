//! Fork feature (`fork/object-remove.md`): the Remove panel's Brush / Object choice for Remove and Content-Aware, the
//! Expand slider of new object spots, the download offer without SAM 3, and a mask's "Remove This Area".
//!
//! In Object mode a click on the photo selects the object under it (SAM 3) and one spot in its shape takes it out
//! (`spot.object`; ⇧ adds to the selected object spot, ⌥ leaves a part out); a loose stroke over an object snaps to
//! it (`spot.objectStroke`). The photo side is `panels/detail/object_remove.rs`.

use lightcraft_develop::{ControlSpec, SpotFill};
use serde_json::json;

use super::{LightcraftApp, Tokens, padded, register, slider, text_button};

/// Whether new spots are made by selecting objects: Object mode with Remove or Content-Aware.
pub(crate) fn object_mode(app: &LightcraftApp) -> bool {
    app.ui.remove_object && matches!(super::remove_compact::mode(app), "remove" | "fill")
}

/// The fill the Fill picker shows and sets: Object mode's own (LaMa unless Classic is picked), else the brush's.
pub(super) fn chosen_fill(app: &LightcraftApp) -> SpotFill {
    if object_mode(app) { app.ui.remove_object_fill } else { app.ui.remove_fill }
}

pub(super) fn set_fill(app: &mut LightcraftApp, fill: SpotFill) {
    if object_mode(app) {
        app.ui.remove_object_fill = fill;
    } else {
        app.ui.remove_fill = fill;
    }
}

/// The Expand slider: how far past the object a spot reaches, in thousandths of the long edge.
fn expand_spec(label: &'static str) -> ControlSpec {
    ControlSpec {
        id: "ui.removeExpand",
        label,
        section: lightcraft_develop::Section::Detail,
        min: 1.0,
        max: 50.0,
        default: 5.0,
        step: 1.0,
        decimals: 0,
        track: lightcraft_develop::Track::Plain,
    }
}

/// The Size slider's label and range for the selected spot: "Expand" (1..50) for a spot in an object's shape.
pub(super) fn size_label(shaped: bool) -> (&'static str, f64) {
    if shaped { ("Expand", 50.0) } else { ("Size", 250.0) }
}

#[cfg(test)]
pub(crate) fn size_label_for_tests(shaped: bool) -> (&'static str, f64) {
    size_label(shaped)
}

/// Brush | Object under the tools, while Remove or Content-Aware is chosen; in Object mode the Expand slider of new
/// spots, and without SAM 3 the offer to download it.
pub(super) fn section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !matches!(super::remove_compact::mode(app), "remove" | "fill") {
        return;
    }
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Select")).size(11.5).color(t.text_dim));
            for (object, id, label, hover) in [
                (false, "removeSelect-brush", "Brush", "Paint over what to remove"),
                (true, "removeSelect-object", "Object", "Click an object to remove all of it; ⇧-click adds to it, ⌥-click leaves a part out"),
            ] {
                let r = ui.selectable_label(app.ui.remove_object == object, crate::i18n::tr(label)).on_hover_text(crate::i18n::tr(hover));
                register(ui, id, r.rect);
                if r.clicked() {
                    app.ui.remove_object = object;
                }
            }
        });
    });
    if !app.ui.remove_object {
        return;
    }
    if !app.session.segmenter.has_model() {
        padded(ui, |ui| {
            if text_button(ui, "removeObjectDownload", crate::i18n::tr("Download SAM 3 to Select Objects…"), false).clicked() {
                app.offer_sam_download(None);
            }
        });
    }
    // with an object spot selected, the Size slider below is its Expand
    let selected_shaped = app
        .session
        .active()
        .and_then(|id| app.session.develop_of(id))
        .is_some_and(|d| app.session.active_spot.and_then(|i| d.spots.get(i)).is_some_and(|sp| sp.area.is_some()));
    if !selected_shaped {
        let out = slider(ui, &expand_spec("Expand"), f64::from(app.ui.remove_expand) * 1000.0, true, None);
        if let Some(v) = out.value {
            app.ui.remove_expand = (v / 1000.0) as f32;
        }
    }
}

/// The params of an Object command: the fill the panel's picker gives (Remove and Content-Aware only), the Expand of
/// new spots and the brush.
pub(crate) fn object_params(app: &LightcraftApp, mut params: serde_json::Value) -> serde_json::Value {
    params["expand"] = json!(app.ui.remove_expand);
    params["fill"] = json!(if super::with_fill(app, "remove", json!({}))["fill"] == "lama" { "lama" } else { "classic" });
    params
}

/// "Remove This Area" in a mask's menu: the mask becomes one Remove spot in its shape, and the Remove panel shows it.
pub(crate) fn remove_this_area(app: &mut LightcraftApp, ui: &mut egui::Ui, mask: u32) {
    let b = ui.button(crate::i18n::tr("Remove This Area"));
    register(ui, "maskMenu:removeArea", b.rect);
    if !b.clicked() {
        return;
    }
    ui.close();
    match app.run_typed("spot.fromMask", json!({"id": mask})) {
        Ok(_) => {
            let _ = app.run("panel.remove", json!({}));
        }
        Err(e) => app.toast_error(ui.ctx(), e.message),
    }
}
