//! Fork feature (`fork/retouch-foundation.md`): the Edit panel's Portrait section.
//!
//! A face picker (All, or one face as a round thumbnail) and a collapsible section per retouching feature, in the
//! order of `lightcraft_retouch::FEATURES`. Each feature's section is its own file in `portrait/`; with a face picked,
//! its sliders change that face alone ([`Ctx::slider`] runs `retouch.set` with the face).

use std::sync::Arc;

use egui::epaint::{Mesh, Vertex};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
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
mod clothes_colour;
mod eyes;
mod glare;
mod hair;
mod reshape;
mod skin;
mod skin_tone;
mod teeth_lips;
mod wrinkles;

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
    (<r::clothes_colour::ClothesColour as Feature>::KEY, clothes_colour::ICON, clothes_colour::body),
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
        let st = app.session.retouch_faces_of(photo);
        // the faces are found once per photo, when the section is first open on it
        if st.found.is_none() && !st.busy && st.error.is_none() {
            app.session.retouch_search(photo, false);
        }
        if st.busy {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        let faces: Arc<Vec<Face>> = Arc::new(st.found.as_ref().map(|f| f.input.faces.clone()).unwrap_or_default());
        let face = picked(ui.ctx(), photo).filter(|i| *i < faces.len());
        if let Some(chosen) = picker(ui, photo, &st, face) {
            pick(ui.ctx(), photo, chosen);
        }
        let face = picked(ui.ctx(), photo).filter(|i| *i < faces.len());
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

/// Side of a face in the picker.
const FACE: f32 = 34.0;

/// The face picker: All, then each face found. Returns the choice clicked (`Some(None)`: All).
fn picker(ui: &mut egui::Ui, photo: PhotoId, st: &lightcraft_engine::retouch_input::FacesState, face: Option<usize>) -> Option<Option<usize>> {
    let t = Tokens::get(ui.ctx());
    let mut chosen = None;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 4 }).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(FACE, FACE), Sense::click());
            register(ui, "portrait:face:all", rect);
            let p = ui.painter();
            p.circle_filled(rect.center(), FACE / 2.0, if resp.hovered() { t.hover } else { t.inset });
            p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("All"), t.semibold(11.5), t.text_label);
            ring(p, rect, face.is_none(), &t);
            if resp.on_hover_text(crate::i18n::tr("All Faces")).clicked() {
                chosen = Some(None);
            }
            let Some(found) = &st.found else { return };
            let textures = thumbs(ui.ctx(), photo, found);
            for (i, f) in found.input.faces.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(FACE, FACE), Sense::click());
                register(ui, format!("portrait:face:{i}"), rect);
                let p = ui.painter();
                match textures.get(i) {
                    Some(tex) => round_image(p, tex.id(), rect.center(), FACE / 2.0),
                    None => {
                        p.circle_filled(rect.center(), FACE / 2.0, t.inset);
                    }
                }
                ring(p, rect, face == Some(i), &t);
                if resp.hovered() && face != Some(i) {
                    p.circle_stroke(rect.center(), FACE / 2.0, Stroke::new(1.0, t.text_dim));
                }
                let name = f.person.as_deref().map(title_case).unwrap_or_else(|| crate::i18n::tr_format!("Face {n}", n = i + 1));
                if resp.on_hover_text(name).clicked() {
                    chosen = Some(Some(i));
                }
            }
        });
        let note = if st.busy {
            Some(crate::i18n::tr("Finding Faces…").to_string())
        } else if let Some(found) = &st.found {
            found.input.faces.is_empty().then(|| crate::i18n::tr("No Faces Found").to_string())
        } else {
            st.error.clone()
        };
        if let Some(note) = note {
            ui.add(egui::Label::new(egui::RichText::new(note).size(11.5).color(t.text_dim)).wrap());
        }
    });
    chosen
}

fn ring(p: &egui::Painter, rect: Rect, on: bool, t: &Tokens) {
    if on {
        p.circle_stroke(rect.center(), FACE / 2.0 + 2.0, Stroke::new(2.0, t.accent));
    }
}

/// `name` with each word's first letter in capitals (the stored person is lower case).
fn title_case(name: &str) -> String {
    name.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The textures of the photo's face thumbnails, made once per detection.
fn thumbs(ctx: &egui::Context, photo: PhotoId, found: &lightcraft_engine::retouch_input::Found) -> Arc<Vec<egui::TextureHandle>> {
    let id = egui::Id::unique(("portraitThumbs", photo.0, found.input.fingerprint()));
    if let Some(t) = ctx.data(|d| d.get_temp::<Arc<Vec<egui::TextureHandle>>>(id)) {
        return t;
    }
    let textures: Vec<egui::TextureHandle> = found
        .thumbs
        .iter()
        .enumerate()
        .map(|(i, img)| {
            let bytes: Vec<u8> = img.data.iter().flatten().copied().collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &bytes);
            ctx.load_texture(format!("portrait-face-{}-{i}", photo.0), image, egui::TextureOptions::LINEAR)
        })
        .collect();
    let textures = Arc::new(textures);
    ctx.data_mut(|d| d.insert_temp(id, textures.clone()));
    textures
}

/// Draw texture `tex` as a disc of radius `r` around `c`.
fn round_image(p: &egui::Painter, tex: egui::TextureId, c: Pos2, r: f32) {
    const STEPS: u32 = 40;
    let mut m = Mesh::with_texture(tex);
    m.vertices.push(Vertex { pos: c, uv: pos2(0.5, 0.5), color: Color32::WHITE });
    for i in 0..=STEPS {
        let (s, co) = (i as f32 / STEPS as f32 * std::f32::consts::TAU).sin_cos();
        m.vertices.push(Vertex { pos: c + vec2(co, s) * r, uv: pos2(0.5 + 0.5 * co, 0.5 + 0.5 * s), color: Color32::WHITE });
    }
    for i in 1..=STEPS {
        m.add_triangle(0, i, i + 1);
    }
    p.add(m);
    p.circle_stroke(c, r, Stroke::new(1.0, Color32::from_black_alpha(90)));
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

    #[test]
    fn names_read_in_title_case() {
        assert_eq!(title_case("ann marie"), "Ann Marie");
        assert_eq!(title_case(""), "");
    }
}
