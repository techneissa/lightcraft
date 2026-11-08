//! The face picker on the photo (fork, `fork/retouch-foundation.md`): a dark card at the top-right corner of the
//! photo in the Edit view, shown while the Portrait section is open. "All", then each face as a round thumbnail,
//! the picked one ringed; it folds into a small faces button, and stays as it was left (`openFlyouts`, [`FOLDED`]).
//! It sits on the view, not on the photo: zooming and panning move the photo under it. Hidden while the tool
//! overlay is off (View ▸ Tool Overlay ▸ Never, Shift+H) and in the full-screen preview.

use std::sync::Arc;

use egui::epaint::{Mesh, Vertex};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::retouch::Face;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::state::RightPanel;
use crate::state::ToolOverlay;
use crate::theme::Tokens;
use crate::widgets::register;

/// The id in `openFlyouts` that keeps the picker folded (absent: open, the default).
pub(crate) const FOLDED: &str = "portrait.facesFolded";

/// Side of a face in the picker.
const FACE: f32 = 30.0;
/// Space between faces, and around them in the card.
const GAP: f32 = 6.0;
const PAD: f32 = 7.0;
/// The fold button.
const BUTTON: f32 = 26.0;
/// From the photo's corner.
const INSET: f32 = 10.0;

/// "Face 2", or the person's name.
pub(super) fn face_name(f: &Face, i: usize) -> String {
    f.person.as_deref().map(title_case).unwrap_or_else(|| crate::i18n::tr_format!("Face {n}", n = i + 1))
}

/// Draw the picker over `over` (the part of the photo on screen) for `photo`.
pub(crate) fn overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, over: Rect, photo: PhotoId) {
    let hidden = app.ui.fullscreen || app.ui.tool_overlay == ToolOverlay::Never;
    if hidden || app.ui.right != RightPanel::Edit || !app.ui.section_open("portrait") || !over.is_positive() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let st = app.session.retouch_faces_of(photo);
    let faces: Vec<Face> = st.found.as_ref().map(|f| f.input.faces.clone()).unwrap_or_default();
    let face = super::picked(ui.ctx(), photo).filter(|i| *i < faces.len());
    let folded = app.ui.flyout_open(FOLDED);
    // the card: faces in rows as wide as fits in most of the photo's width, the fold button at the end of the first
    let items = faces.len() + 1;
    let room = ((over.width() * 0.6 - 2.0 * PAD - BUTTON - GAP) / (FACE + GAP)).floor().max(1.0) as usize;
    let cols = if folded { 0 } else { items.min(room) };
    let rows = if folded { 0 } else { items.div_ceil(cols.max(1)) };
    let width = 2.0 * PAD + BUTTON + cols as f32 * (FACE + GAP);
    let height = 2.0 * PAD + (rows as f32 * (FACE + GAP) - GAP).max(BUTTON);
    let card = Rect::from_min_size(pos2(over.right() - INSET - width, over.top() + INSET), vec2(width, height));
    let p = ui.painter().with_clip_rect(over);
    // the card takes the clicks between its faces (they never reach the photo)
    let _ = ui.interact(card, egui::Id::unique("portrait-faces-card"), Sense::click());
    p.rect_filled(card, 6.0, Color32::from_black_alpha(170));
    register(ui, "portrait:facesCard", card);

    let button = Rect::from_min_size(pos2(card.right() - PAD - BUTTON, card.top() + PAD), vec2(BUTTON, BUTTON));
    let resp = ui.interact(button, egui::Id::unique("portrait-faces-fold"), Sense::click());
    register(ui, "portrait:faces", button);
    if resp.hovered() {
        p.rect_filled(button, 4.0, Color32::from_white_alpha(28));
    }
    paint(&p, button.shrink(4.0), if folded { Icon::FaceBox } else { Icon::ChevronRight }, Color32::from_gray(225));
    let tip = if folded { crate::i18n::tr("Show Faces") } else { crate::i18n::tr("Hide Faces") };
    if resp.on_hover_text(tip).clicked() {
        app.ui.toggle_flyout(FOLDED);
    }
    if folded {
        return;
    }
    let textures = st.found.as_ref().map(|f| thumbs(ui.ctx(), photo, f)).unwrap_or_default();
    let mut chosen = None;
    for i in 0..items {
        let (row, col) = (i / cols.max(1), i % cols.max(1));
        let min = pos2(card.left() + PAD + col as f32 * (FACE + GAP), card.top() + PAD + row as f32 * (FACE + GAP));
        let rect = Rect::from_min_size(min, vec2(FACE, FACE));
        let (id, picked) = match i {
            0 => ("portrait:face:all".to_string(), face.is_none()),
            n => (format!("portrait:face:{}", n - 1), face == Some(n - 1)),
        };
        let resp = ui.interact(rect, egui::Id::unique(("portrait-face", i)), Sense::click());
        register(ui, id, rect);
        match (i, i.checked_sub(1).and_then(|n| textures.get(n))) {
            (0, _) => {
                p.circle_filled(rect.center(), FACE / 2.0, Color32::from_gray(if resp.hovered() { 85 } else { 60 }));
                p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("All"), t.semibold(11.0), Color32::from_gray(235));
            }
            (_, Some(tex)) => round_image(&p, tex.id(), rect.center(), FACE / 2.0),
            (_, None) => {
                p.circle_filled(rect.center(), FACE / 2.0, Color32::from_gray(60));
            }
        }
        if picked {
            p.circle_stroke(rect.center(), FACE / 2.0 + 2.0, Stroke::new(2.0, t.accent));
        } else if resp.hovered() {
            p.circle_stroke(rect.center(), FACE / 2.0 + 1.0, Stroke::new(1.0, Color32::from_gray(200)));
        }
        let name = match i.checked_sub(1).and_then(|n| faces.get(n).map(|f| (n, f))) {
            Some((n, f)) => face_name(f, n),
            None => crate::i18n::tr("All Faces").to_string(),
        };
        if resp.on_hover_text(name).clicked() {
            chosen = Some(i.checked_sub(1));
        }
    }
    if let Some(choice) = chosen {
        super::pick(ui.ctx(), photo, choice);
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
    #[test]
    fn names_read_in_title_case() {
        assert_eq!(super::title_case("ann marie"), "Ann Marie");
        assert_eq!(super::title_case(""), "");
    }
}
