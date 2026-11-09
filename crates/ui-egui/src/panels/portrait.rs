//! Fork feature (`fork/retouch-foundation.md`): the Portrait tab of the right-hand tool strip (`RightPanel::Portrait`,
//! next to Edit, Crop, Remove and Masking; shortcut T).
//!
//! A title row with the switch for all retouching, a line saying which faces the sliders change, and a collapsible
//! section per portrait feature, then Portrait Presets ([`SECTIONS`]). Each section is its own file in `portrait/`;
//! with a face picked on the photo (the people picker, `portrait/overlay.rs`, shown while this tab is open), its
//! sliders change that face alone ([`Ctx::slider`] runs `retouch.set` with the face). Open sections are kept in the
//! saved `openSections` as `portrait.<key>` (closed by default).

use std::sync::Arc;

use egui::{Align2, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::{Face, Feature, Input};
use lightcraft_develop::{DevelopSettings, controls};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::{divider, register};

mod background;
mod blemish;
mod eyes;
mod glare;
mod hair;
mod presets;
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

/// The id `develop.sectionEnabled` takes for all of portrait retouching (the title row's eye).
const SWITCH: &str = "portrait";

/// One section of the tab: its key (`portrait.<key>` in `openSections`, widget `section:portrait.<key>`), its
/// title (a feature's label, translated where it is drawn) and its body.
pub(crate) struct Section {
    pub key: &'static str,
    pub title: &'static str,
    body: fn(&mut Ctx<'_>),
}

const fn feature<F: Feature>(body: fn(&mut Ctx<'_>)) -> Section {
    Section { key: F::KEY, title: F::LABEL, body }
}

/// The tab's sections, top to bottom: every retouching feature, then Portrait Presets. (The render order is
/// `lightcraft_retouch::FEATURES`'; this is the order to work in.)
pub(crate) const SECTIONS: &[Section] = &[
    feature::<r::skin::Skin>(skin::body),
    feature::<r::blemish::Blemish>(blemish::body),
    feature::<r::wrinkles::Wrinkles>(wrinkles::body),
    feature::<r::eyes::Eyes>(eyes::body),
    feature::<r::teeth_lips::TeethLips>(teeth_lips::body),
    feature::<r::skin_tone::SkinTone>(skin_tone::body),
    feature::<r::reshape::Reshape>(reshape::body),
    feature::<r::glare::Glare>(glare::body),
    feature::<r::background::Background>(background::body),
    feature::<r::hair::Hair>(hair::body),
    Section { key: presets::KEY, title: presets::TITLE, body: presets::body },
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
    /// The photo's faces as found (shared with the store: never copied per frame).
    pub input: Arc<Input>,
    /// The photo's lens corrections and transform, to match a face's own settings kept in the other frame.
    pub keys: Option<lightcraft_pipeline::retouch_step::FrameKeyMap>,
}

impl Ctx<'_> {
    /// The slider of control `id` (`retouch.<feature>.<field>`): the value for every face, or the picked face's.
    #[allow(dead_code)] // the feature sections use it as they land
    pub fn slider(&mut self, id: &str) {
        let Some(spec) = controls::find(id) else { return };
        let v = match self.face {
            Some(i) => {
                let keys = self.keys.as_ref().map(|k| k as &dyn lightcraft_develop::retouch::KeyMap);
                self.d.retouch.settings_for_in(&self.input.faces, i, keys).get(id)
            }
            None => self.d.retouch.global.get(id),
        };
        let out = crate::widgets::slider(self.ui, spec, v.unwrap_or(spec.default), true, None);
        let face = self.face;
        crate::panels::edit::apply_slider_out(self.app, spec, out, |app, v| app.run("retouch.set", json!({"control": id, "value": v, "face": face})));
    }
}

/// The People view's "Apply Portrait Preset" menu for `person` (`portrait/presets.rs`).
pub(crate) fn apply_preset_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, person: &str) {
    presets::person_menu(app, ui, person);
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

/// The Portrait tab for `photo`.
pub(crate) fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, photo: PhotoId) {
    let d = app.session.develop_of(photo).unwrap_or_default();
    title_row(app, ui, &d);
    let mut st = app.session.retouch_faces_of(photo);
    // the faces are found once per photo, when the tab is first open on it (without a face model: why not)
    if st.found.is_none() && !st.busy && st.error.is_none() {
        st.error = app.session.retouch_search(photo, false);
    }
    if st.busy {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
    let input: Arc<Input> = st.found.as_ref().map(|f| f.input.clone()).unwrap_or_default();
    let face = picked(ui.ctx(), photo).filter(|i| *i < input.faces.len());
    // the face picker is on the photo (`overlay`); here, which face the sliders change
    editing_line(ui, &st, face.and_then(|i| input.faces.get(i).map(|f| (i, f))));
    ui.add_space(6.0);
    divider(ui);
    let keys = app.session.retouch_key_map(photo);
    let mut cx = Ctx { app, ui, d: &d, photo, face, input, keys };
    for s in SECTIONS {
        let id = format!("portrait.{}", s.key);
        let open = cx.app.ui.section_open(&id);
        let (resp, _) = crate::widgets::section_header(cx.ui, &id, s.title, open, None);
        if resp.clicked()
            && let Err(e) = cx.app.run("portrait.section", json!({"section": s.key}))
        {
            cx.app.toast(cx.ui.ctx(), e);
        }
        if open {
            (s.body)(&mut cx);
        }
        divider(cx.ui);
    }
    cx.ui.add_space(40.0);
}

/// "Portrait", and the eye that turns all retouching off and on for the photo (`develop.sectionEnabled`).
fn title_row(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr("Portrait"), t.semibold(15.0), t.text);
    let on = d.section_enabled(SWITCH);
    let eye = egui::Rect::from_center_size(pos2(r.right() - 34.0, r.center().y + 2.0), vec2(26.0, 26.0));
    let resp = ui.interact(eye, egui::Id::unique("portrait-switch"), Sense::click());
    register(ui, "portrait:switch", eye);
    if resp.hovered() {
        ui.painter().rect_filled(eye, 4.0, t.chrome.gamma_multiply(1.3));
    }
    paint(ui.painter(), eye.shrink(4.0), if on { Icon::Eye } else { Icon::EyeOff }, if on { t.text_label } else { t.text_dim });
    let tip = if on { crate::i18n::tr("Turn Portrait Off") } else { crate::i18n::tr("Turn Portrait On") };
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run("develop.sectionEnabled", json!({"section": SWITCH, "enabled": !on}));
    }
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
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 2 }).show(ui, |ui| {
        let r = ui.label(egui::RichText::new(crate::i18n::tr_format!("Editing: {who}", who = who)).size(12.5).color(t.text_label));
        register(ui, "portrait:editing", r.rect);
        if let Some(note) = note {
            ui.add(egui::Label::new(egui::RichText::new(note).size(11.5).color(t.text_dim)).wrap());
        }
    });
}

/// How `ui.json` saves the open tab (`UiState::right`): the Portrait tab as Edit. A build without this tab fails to
/// read a `right` it does not know and drops the whole file, so the app reopens on Edit instead.
pub(crate) fn save_right<S: serde::Serializer>(right: &RightPanel, s: S) -> Result<S::Ok, S::Error> {
    let saved = if *right == RightPanel::Portrait { RightPanel::Edit } else { *right };
    serde::Serialize::serialize(&saved, s)
}

/// The tab's UI commands: `portrait.section {section, open?}` opens the Portrait tab and opens or closes one of its
/// sections (no `open`: toggles). Any other id: an error.
pub(crate) fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<Value, String> {
    if id != "portrait.section" {
        return Err(format!("{id}: unknown command"));
    }
    let keys = || SECTIONS.iter().map(|s| s.key).collect::<Vec<_>>().join("|");
    let key = p.get("section").and_then(Value::as_str).ok_or_else(|| format!("{id}: `section` is needed ({})", keys()))?;
    if !SECTIONS.iter().any(|s| s.key == key) {
        return Err(format!("{id}: unknown section `{key}` ({})", keys()));
    }
    let open = match p.get("open") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| format!("{id}: `open` must be true or false, not {v}"))?),
    };
    let sid = format!("portrait.{key}");
    let now = app.ui.section_open(&sid);
    let want = open.unwrap_or(!now);
    // (not `toggle_section`: the Edit panel's Solo Mode would close its own sections)
    if want && !now {
        app.ui.open_sections.push(sid);
    } else if !want {
        app.ui.open_sections.retain(|s| *s != sid);
    }
    app.ui.right = RightPanel::Portrait;
    Ok(json!({"section": key, "open": want}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::retouch::FEATURES;

    /// An older build reads `ui.json`: the Portrait tab is saved as Edit, so the file stays readable there.
    #[test]
    fn the_portrait_tab_is_saved_as_edit() {
        let ui = crate::state::UiState { right: RightPanel::Portrait, ..Default::default() };
        let saved = serde_json::to_value(&ui).unwrap();
        assert_eq!(saved["right"], "edit");
        let ui = crate::state::UiState { right: RightPanel::Masking, ..Default::default() };
        assert_eq!(serde_json::to_value(&ui).unwrap()["right"], "masking");
    }

    #[test]
    fn every_feature_has_one_section_and_presets_come_last() {
        for f in FEATURES {
            assert_eq!(SECTIONS.iter().filter(|s| s.key == f.key).count(), 1, "{}", f.key);
        }
        assert_eq!(SECTIONS.len(), FEATURES.len() + 1);
        assert_eq!(SECTIONS.last().map(|s| s.key), Some(presets::KEY));
        let first: Vec<&str> = SECTIONS.iter().take(3).map(|s| s.key).collect();
        assert_eq!(first, ["skin", "blemish", "wrinkles"]);
    }
}
