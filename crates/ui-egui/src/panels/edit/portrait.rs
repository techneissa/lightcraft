//! Fork feature (`fork/retouch-foundation.md`): the Edit panel's Portrait section.
//!
//! A face picker (All, or one face as a round thumbnail) and a collapsible section per retouching feature, in the
//! order of `lightcraft_retouch::FEATURES`. Each feature's section is its own file in `portrait/`; with a face picked,
//! its sliders change that face alone ([`Ctx::slider`] runs `retouch.set` with the face).

use std::sync::Arc;

use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::{FEATURES, Face, Feature};
use lightcraft_develop::{DevelopSettings, controls};
use serde_json::json;

use super::sliders::{apply_slider_out, section};
use crate::LightcraftApp;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::widgets::{flyout_row, register};

mod background;
mod blemish;
mod eyes;
mod glare;
mod hair;
mod reshape;
mod skin;
mod skin_tone;
mod teeth_lips;
mod wrinkles;

mod overlay;
#[cfg(test)]
pub(crate) use overlay::FOLDED;
pub(crate) use overlay::overlay;

use lightcraft_develop::retouch as r;

/// Each feature's section: (feature key, icon, body), in panel order.
const SECTIONS: &[(&str, Icon, fn(&mut Ctx<'_>))] = &[
    (<r::blemish::Blemish as Feature>::KEY, blemish::ICON, blemish::body),
    (<r::skin::Skin as Feature>::KEY, skin::ICON, skin::body),
    (<r::wrinkles::Wrinkles as Feature>::KEY, wrinkles::ICON, wrinkles::body),
    (<r::skin_tone::SkinTone as Feature>::KEY, skin_tone::ICON, skin_tone::body),
    (<r::eyes::Eyes as Feature>::KEY, eyes::ICON, eyes::body),
    (<r::teeth_lips::TeethLips as Feature>::KEY, teeth_lips::ICON, teeth_lips::body),
    (<r::glare::Glare as Feature>::KEY, glare::ICON, glare::body),
    (<r::hair::Hair as Feature>::KEY, hair::ICON, hair::body),
    (<r::reshape::Reshape as Feature>::KEY, reshape::ICON, reshape::body),
    (<r::background::Background as Feature>::KEY, background::ICON, background::body),
];

/// What a feature's section draws with.
pub(super) struct Ctx<'a> {
    pub app: &'a mut LightcraftApp,
    pub ui: &'a mut egui::Ui,
    pub d: &'a DevelopSettings,
    #[allow(dead_code)] // for the feature sections
    pub photo: PhotoId,
    /// The face picked (an index of `retouch.faces`), `None` for All.
    pub face: Option<usize>,
    /// The photo's faces as found.
    pub faces: Arc<Vec<Face>>,
}

impl Ctx<'_> {
    /// The slider of control `id` (`retouch.<feature>.<field>`): the value for every face, or the picked face's.
    #[allow(dead_code)] // the feature sections use it as they land
    pub fn slider(&mut self, id: &str) {
        let Some(spec) = controls::find(id) else { return };
        let v = match self.face {
            Some(i) => self.d.retouch.settings_for(&self.faces, i).get(id),
            None => self.d.retouch.global.get(id),
        };
        let out = crate::widgets::slider(self.ui, spec, v.unwrap_or(spec.default), true, None);
        let face = self.face;
        apply_slider_out(self.app, spec, out, |app, v| app.run("retouch.set", json!({"control": id, "value": v, "face": face})));
    }
}

/// The face picked for `photo` (`None`: All).
pub(crate) fn picked(ctx: &egui::Context, photo: PhotoId) -> Option<usize> {
    ctx.data(|d| d.get_temp::<usize>(pick_id(photo)))
}

pub(crate) fn pick(ctx: &egui::Context, photo: PhotoId, face: Option<usize>) {
    ctx.data_mut(|d| match face {
        Some(i) => {
            d.insert_temp(pick_id(photo), i);
        }
        None => d.remove::<usize>(pick_id(photo)),
    });
}

fn pick_id(photo: PhotoId) -> egui::Id {
    egui::Id::unique(("portraitFace", photo.0))
}

/// The Portrait section of the Edit panel.
pub(super) fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, photo: PhotoId) {
    section(app, ui, d, "portrait", "Portrait", |app, ui, d| {
        let mut st = app.session.retouch_faces_of(photo);
        // the faces are found once per photo, when the section is first open on it (without a face model: why not)
        if st.found.is_none() && !st.busy && st.error.is_none() {
            st.error = app.session.retouch_search(photo, false);
        }
        if st.busy {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        let faces: Arc<Vec<Face>> = Arc::new(st.found.as_ref().map(|f| f.input.faces.clone()).unwrap_or_default());
        let face = picked(ui.ctx(), photo).filter(|i| *i < faces.len());
        // the face picker is on the photo (`overlay`); here, which face the sliders change
        editing_line(ui, &st, face.and_then(|i| faces.get(i).map(|f| (i, f))));
        ui.add_space(4.0);
        let mut cx = Ctx { app, ui, d, photo, face, faces };
        for (key, icon, body) in SECTIONS {
            let label = FEATURES.iter().find(|f| f.key == *key).map_or(*key, |f| f.label);
            let id = format!("portrait.{key}");
            let open = cx.app.ui.flyout_open(&id);
            if flyout_row(cx.ui, &id, label, *icon, open).clicked() {
                cx.app.ui.toggle_flyout(&id);
            }
            if open {
                body(&mut cx);
            }
        }
        cx.ui.add_space(8.0);
    });
}

/// "Editing: All Faces" or the picked face, and why there are no faces when there are none.
fn editing_line(ui: &mut egui::Ui, st: &lightcraft_engine::retouch_input::FacesState, face: Option<(usize, &Face)>) {
    let t = Tokens::get(ui.ctx());
    let who = match face {
        Some((i, f)) => overlay::face_name(f, i),
        None => crate::i18n::tr("All Faces").to_string(),
    };
    let note = if st.busy {
        Some(crate::i18n::tr("Finding Faces…").to_string())
    } else if let Some(found) = &st.found {
        found.input.faces.is_empty().then(|| crate::i18n::tr("No Faces Found").to_string())
    } else {
        st.error.clone()
    };
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 2 }).show(ui, |ui| {
        let r = ui.label(egui::RichText::new(crate::i18n::tr_format!("Editing: {who}", who = who)).size(12.5).color(t.text_label));
        register(ui, "portrait:editing", r.rect);
        if let Some(note) = note {
            ui.add(egui::Label::new(egui::RichText::new(note).size(11.5).color(t.text_dim)).wrap());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_has_a_section_in_feature_order() {
        let keys: Vec<&str> = SECTIONS.iter().map(|s| s.0).collect();
        let features: Vec<&str> = FEATURES.iter().map(|f| f.key).collect();
        assert_eq!(keys, features);
    }
}
