//! The Portrait tab's section of the background feature (`lightcraft_retouch::background`, `fork/retouch-background.md`):
//! Blur, Swap and what it swaps in (a colour, a picture file or a photo of the view), and Clean Backdrop. One setting
//! for the photo: the sliders write the values for every face whichever face is picked.
//!
//! UI commands: `portrait.backgroundPicture {path?}` asks for a picture file (or takes `path`) and makes it what Swap
//! puts behind the person (the engine copies it into the library and keeps its hash).

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::background::Swap;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::i18n::tr;
use crate::pick::{PickRequest, Picked};
use crate::theme::Tokens;

pub(crate) const CHOOSE_PICTURE: &str = "portrait.backgroundPicture";
/// The picture files Swap takes.
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "bmp", "jxl"];
/// The colours offered in one click (sRGB): white, a light and a mid studio grey, black.
const SWATCHES: [(&str, &str); 4] = [("#FFFFFF", "White"), ("#D9D9D9", "Light Grey"), ("#808080", "Grey"), ("#000000", "Black")];
const SWATCH: f32 = 22.0;
const THUMB: f32 = 48.0;
/// Photos of the view offered.
const STRIP_PHOTOS: usize = 60;

pub(super) fn body(cx: &mut super::Ctx<'_>) {
    // (a setting for the photo: never one face's own)
    let face = cx.face.take();
    cx.slider("retouch.background.blur");
    cx.slider("retouch.background.swap");
    swap_with(cx);
    cx.slider("retouch.background.clean");
    cx.face = face;
}

fn run(cx: &mut super::Ctx<'_>, with: Value) {
    if let Err(e) = cx.app.run("retouch.background", json!({"id": cx.photo.0, "with": with})) {
        cx.app.toast(cx.ui.ctx(), e);
    }
}

/// Swap With: the colour swatches, a colour of one's own, a picture file, a photo of the view.
fn swap_with(cx: &mut super::Ctx<'_>) {
    let t = Tokens::get(cx.ui.ctx());
    let with = cx.d.retouch.global.background.with.clone();
    let status = cx.app.session.backdrop_status(cx.d);
    let strip_id = egui::Id::unique(("portraitBackgroundStrip", cx.photo.0));
    let mut strip = cx.ui.ctx().data(|d| d.get_temp::<bool>(strip_id)).unwrap_or(false);
    let own_id = egui::Id::unique(("portraitBackgroundColour", cx.photo.0));
    let chosen = egui::Frame::NONE
        .inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 6 })
        .show(cx.ui, |ui| {
            ui.label(egui::RichText::new(tr("Swap With")).size(12.0).color(t.text_label));
            let mut chosen: Option<Value> = None;
            ui.horizontal(|ui| {
                for (hex, name) in SWATCHES {
                    let (rect, resp) = ui.allocate_exact_size(vec2(SWATCH, SWATCH), Sense::click());
                    let rgb = match Swap::from_hex(hex) {
                        Some(Swap::Colour { rgb }) => rgb.map(|v| (v * 255.0).round() as u8),
                        _ => [128; 3],
                    };
                    ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                    let on = with.hex().as_deref() == Some(hex);
                    let stroke = if on { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.text_dim) };
                    ui.painter().rect_stroke(rect, 3.0, stroke, StrokeKind::Inside);
                    crate::widgets::register_response(format!("portrait:background:colour:{}", hex.trim_start_matches('#')), rect, &resp);
                    if resp.on_hover_text(tr(name)).clicked() {
                        chosen = Some(json!({"colour": hex}));
                    }
                }
                // a colour of one's own, written once the picker closes (a drag in it is one undo step)
                let current = match &with {
                    Swap::Colour { rgb } => rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8),
                    _ => [217, 217, 217],
                };
                let mut rgb = ui.data(|d| d.get_temp::<[u8; 3]>(own_id)).unwrap_or(current);
                let resp = egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).on_hover_text(tr("Choose Colour"));
                crate::widgets::register_response("portrait:background:colour", resp.rect, &resp);
                if resp.changed() {
                    ui.data_mut(|d| d.insert_temp(own_id, rgb));
                }
                if !egui::Popup::is_any_open(ui.ctx())
                    && let Some(own) = ui.data_mut(|d| d.remove_temp::<[u8; 3]>(own_id))
                    && own != current
                {
                    chosen = Some(json!({"colour": format!("#{:02X}{:02X}{:02X}", own[0], own[1], own[2])}));
                }
            });
            ui.horizontal(|ui| {
                let r = ui.button(tr("Choose Picture…"));
                crate::widgets::register_response("portrait:background:picture", r.rect, &r);
                if r.clicked() {
                    ui.data_mut(|d| d.insert_temp(egui::Id::unique("portraitBackgroundAsk"), true));
                }
                let r = ui.selectable_label(strip, tr("Use a Photo…"));
                crate::widgets::register_response("portrait:background:photos", r.rect, &r);
                if r.clicked() {
                    strip = !strip;
                }
            });
            if let Some(line) = picture_line(&with, &status) {
                ui.add(egui::Label::new(egui::RichText::new(line).size(11.5).color(t.text_dim)).truncate());
            }
            chosen
        })
        .inner;
    cx.ui.ctx().data_mut(|d| d.insert_temp(strip_id, strip));
    if status["loading"] == true {
        cx.ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }
    if strip {
        let picked = photo_strip(cx, &with, &t);
        if let Some(id) = picked {
            run(cx, json!({"photo": id.0}));
        }
    }
    if let Some(w) = chosen {
        run(cx, w);
    }
    if cx.ui.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique("portraitBackgroundAsk"))).unwrap_or(false)
        && let Err(e) = cx.app.run(CHOOSE_PICTURE, json!({"id": cx.photo.0}))
    {
        cx.app.toast(cx.ui.ctx(), e);
    }
}

/// What the picture line says: the picture's name and whether it is loading, missing or failed; `None` for a colour.
fn picture_line(with: &Swap, status: &Value) -> Option<String> {
    if matches!(with, Swap::Colour { .. }) {
        return None;
    }
    if status["missing"] == true {
        return Some(tr("Background Picture Missing").to_string());
    }
    let name = match (with, status["name"].as_str()) {
        (_, Some(n)) => n.to_string(),
        (Swap::Photo { id }, None) => format!("{} {id}", tr("Photo")),
        _ => tr("Photo").to_string(),
    };
    Some(match (status["loading"].as_bool(), status["error"].as_str()) {
        (_, Some(e)) => format!("{name}: {e}"),
        (Some(true), _) => format!("{name} ({})", tr("Loading…")),
        _ => name,
    })
}

/// The photos of the view, small; the one clicked becomes Swap's picture.
fn photo_strip(cx: &mut super::Ctx<'_>, with: &Swap, t: &Tokens) -> Option<PhotoId> {
    let ids: Vec<PhotoId> = cx.app.session.visible_cloned().into_iter().filter(|id| *id != cx.photo).take(STRIP_PHOTOS).collect();
    let chosen = match with {
        Swap::Photo { id } => Some(PhotoId(*id)),
        _ => None,
    };
    let mut picked = None;
    let app = &mut *cx.app;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 6 }).show(cx.ui, |ui| {
        egui::ScrollArea::horizontal().id_salt("portrait:background:strip").max_height(THUMB + 8.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                for id in ids {
                    crate::panels::grid::request_thumb(app, id, 128, 4);
                    let (rect, resp) = ui.allocate_exact_size(vec2(THUMB, THUMB), Sense::click());
                    let p = ui.painter_at(rect);
                    match app.renderer.thumb(id) {
                        Some(tex) => {
                            let k = (THUMB / tex.size[0].max(tex.size[1]).max(1) as f32).min(1.0);
                            let size = vec2(tex.size[0] as f32 * k, tex.size[1] as f32 * k);
                            let r = Rect::from_center_size(rect.center(), size);
                            p.image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        None => {
                            p.rect_filled(rect, 2.0, t.skeleton);
                        }
                    }
                    if chosen == Some(id) {
                        p.rect_stroke(rect, 2.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
                    }
                    crate::widgets::register_response(format!("portrait:background:photo:{}", id.0), rect, &resp);
                    if resp.clicked() {
                        picked = Some(id);
                    }
                }
            });
        });
    });
    picked
}

/// `portrait.backgroundPicture {path?, id?}`: a picture file becomes what Swap puts behind the person.
pub(crate) fn choose_picture(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => Some(path.trim().to_string()),
        None => {
            let req = PickRequest::file(tr("Choose Background Picture…"), tr("Images"), EXTENSIONS);
            match crate::pick::ask(app, CHOOSE_PICTURE, p, "path", req, |s| s.pick_files.as_mut().map(|f| f())) {
                Picked::Now(paths) => paths.into_iter().next(),
                Picked::Later => return Ok(Value::Null),
                Picked::Unavailable => return Err("no file dialog on this platform; supply path".into()),
            }
        }
    };
    let Some(path) = path.filter(|p| !p.is_empty()) else { return Ok(Value::Null) };
    let mut params = json!({"with": {"file": path}});
    if let Some(id) = p.get("id").filter(|v| !v.is_null()) {
        params["id"] = id.clone();
    }
    app.run("retouch.background", params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_line_names_the_picture_and_says_when_it_is_missing() {
        let pic = Swap::Picture { hash: "ab".repeat(32) };
        assert_eq!(picture_line(&Swap::default(), &json!({})), None);
        assert_eq!(picture_line(&pic, &json!({"missing": true, "name": "wall.jpg"})).as_deref(), Some("Background Picture Missing"));
        assert_eq!(picture_line(&pic, &json!({"name": "wall.jpg", "loading": true})).as_deref(), Some("wall.jpg (Loading…)"));
        assert_eq!(picture_line(&pic, &json!({"name": "wall.jpg", "error": "broken"})).as_deref(), Some("wall.jpg: broken"));
        assert_eq!(picture_line(&Swap::Photo { id: 4 }, &json!({})).as_deref(), Some("Photo 4"));
    }
}
