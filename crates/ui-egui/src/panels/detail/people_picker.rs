//! Fork (`fork/retouch-foundation.md`): the people picker, a card over the loupe at the top-right corner of the
//! image view (a fixed margin from the view's edges, the same place for every photo, zoom and aspect ratio).
//!
//! One row: the faces ("All", then one round chip per face; one at a time), then a dropdown of parts a caller may
//! fill (Entire Person, Facial Skin, Hair…; several with ⌘-click; `parts_menu.rs`), absent while it has no items. The card folds into an
//! overflow button ("⋯") at the same corner, which opens it again. It sits on the view: zooming and panning move
//! the photo under it, and clicks on it never reach the photo. The caller decides when it shows and keeps the
//! folded state and the selections ([`Picker`] in, [`Picked`] out).
//!
//! Faces stay single-select: a per-face slider writes one face's own settings (`retouch.set {face}`); writing
//! several faces at once would need a multi-face write and undo step of its own.

use egui::epaint::{Mesh, Vertex};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, pos2, vec2};

use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

/// A face chip: its round thumbnail and its name (on hover).
pub(crate) struct FaceChip {
    pub texture: Option<egui::TextureId>,
    pub name: String,
}

/// An item of the parts row.
pub(crate) struct PartItem {
    /// Stable key, part of the widget id (`<id>:part:<key>`).
    pub key: String,
    pub label: String,
    /// The item starts a group of the menu under this header (`""`: a divider only).
    pub group: Option<&'static str>,
}

/// What the picker shows.
pub(crate) struct Picker<'a> {
    /// The widget-id prefix (`portrait` → `portrait:face:all`, `portrait:face:0`, `portrait:part:hair`,
    /// `portrait:more`, `portrait:card`).
    pub id: &'a str,
    pub faces: &'a [FaceChip],
    /// The face picked; `None`: All; an index past `faces`: none (a saved face not in this photo).
    pub face: Option<usize>,
    /// The parts row; empty: no row.
    pub parts: &'a [PartItem],
    /// Indexes of `parts` selected now.
    pub selected_parts: &'a [usize],
    pub folded: bool,
    /// A line under the faces ("Finding people…", why there are none); `None`: no line (Select People,
    /// `fork/select-people.md`).
    pub note: Option<&'a str>,
}

/// What was clicked this frame.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Picked {
    /// A face chip: `Some(None)` = All, `Some(Some(i))` = face `i`.
    pub face: Option<Option<usize>>,
    /// The new parts selection, when it changed.
    pub parts: Option<Vec<usize>>,
    /// The overflow button: fold or open the card.
    pub toggle_fold: bool,
}

/// The selection after clicking item `clicked` of `current`: a plain click selects that item alone; with `add`
/// (⌘-click) it joins the selection, or leaves it when it was in. Sorted, without repeats.
pub(crate) fn select(current: &[usize], clicked: usize, add: bool) -> Vec<usize> {
    let mut out: Vec<usize> = if add { current.to_vec() } else { Vec::new() };
    out.sort_unstable();
    out.dedup();
    match out.binary_search(&clicked) {
        Ok(i) if add => {
            out.remove(i);
        }
        Ok(_) => {}
        Err(i) => out.insert(i, clicked),
    }
    out
}

/// From the view's edges.
pub(crate) const MARGIN: f32 = 12.0;
const FACE: f32 = 30.0;
const GAP: f32 = 6.0;
const PAD: f32 = 7.0;
const BUTTON: f32 = 26.0;
const PART_H: f32 = 24.0;
/// The parts dropdown is at most this wide.
const DROP_MAX: f32 = 180.0;

/// Draw the picker over `canvas` (the image view). The card's top-right corner is `MARGIN` from the view's, below
/// the camera-preview badge when that is in the way.
pub(crate) fn show(ui: &mut egui::Ui, canvas: Rect, pk: &Picker<'_>) -> Picked {
    let mut out = Picked::default();
    if !canvas.is_positive() {
        return out;
    }
    let t = Tokens::get(ui.ctx());
    let p = ui.painter().with_clip_rect(canvas);
    let max_w = (canvas.width() * 0.6).max(BUTTON + 2.0 * PAD);
    let font = t.font(11.5);
    // one row: All + faces, the parts dropdown, then the button (the faces wrap when there are many)
    let drop_text = parts_menu::summary(pk.parts, pk.selected_parts);
    let drop_w = if pk.parts.is_empty() { 0.0 } else { parts_menu::button_width(&p, &drop_text, font.clone(), DROP_MAX) + GAP };
    let items = pk.faces.len() + 1;
    let room = ((max_w - 2.0 * PAD - BUTTON - GAP - drop_w) / (FACE + GAP)).floor().max(1.0) as usize;
    let cols = items.min(room);
    let rows = items.div_ceil(cols.max(1));
    let row1_w = cols as f32 * (FACE + GAP) + drop_w + BUTTON;
    let row1_h = (rows as f32 * (FACE + GAP) - GAP).max(BUTTON);
    let note = pk.note.map(|n| p.layout(crate::i18n::tr(n).to_string(), font.clone(), Color32::from_gray(205), max_w - 2.0 * PAD));
    let note_h = note.as_ref().map_or(0.0, |g| GAP + g.size().y);
    let note_w = note.as_ref().map_or(0.0, |g| g.size().x);
    let size =
        if pk.folded { vec2(BUTTON + 2.0 * PAD, BUTTON + 2.0 * PAD) } else { vec2(2.0 * PAD + row1_w.max(note_w), 2.0 * PAD + row1_h + note_h) };
    let mut top = canvas.top() + MARGIN;
    let mut card = Rect::from_min_size(pos2(canvas.right() - MARGIN - size.x, top), size);
    if let Some(badge) = registered(ui.ctx(), "loupe:cameraPreview").filter(|b| b.intersects(card.expand(4.0))) {
        top = badge.bottom() + 6.0;
        card = Rect::from_min_size(pos2(card.left(), top), size);
    }
    // the card takes the clicks between its items (they never reach the photo)
    let _ = ui.interact(card, egui::Id::unique((pk.id, "people-picker-card")), Sense::click());
    p.rect_filled(card, 6.0, Color32::from_black_alpha(170));
    register(ui, format!("{}:card", pk.id), card);

    // the overflow button, at the card's top-right corner folded or not
    let button = Rect::from_min_size(pos2(card.right() - PAD - BUTTON, card.top() + PAD), vec2(BUTTON, BUTTON));
    let resp = ui.interact(button, egui::Id::unique((pk.id, "people-picker-more")), Sense::click());
    register(ui, format!("{}:more", pk.id), button);
    if resp.hovered() {
        p.rect_filled(button, 4.0, Color32::from_white_alpha(28));
    }
    paint(&p, button.shrink(4.0), if pk.folded { Icon::More } else { Icon::ChevronRight }, Color32::from_gray(225));
    let tip = if pk.folded { crate::i18n::tr("Show People") } else { crate::i18n::tr("Hide People") };
    out.toggle_fold = resp.on_hover_text(tip).clicked();
    if pk.folded {
        parts_menu::set_open(ui.ctx(), pk, false);
        return out;
    }
    // row 1
    for i in 0..items {
        let (row, col) = (i / cols.max(1), i % cols.max(1));
        let rect =
            Rect::from_min_size(pos2(card.left() + PAD + col as f32 * (FACE + GAP), card.top() + PAD + row as f32 * (FACE + GAP)), vec2(FACE, FACE));
        let face = i.checked_sub(1);
        let resp = ui.interact(rect, egui::Id::unique((pk.id, "people-picker-face", i)), Sense::click());
        register(ui, face.map_or(format!("{}:face:all", pk.id), |n| format!("{}:face:{n}", pk.id)), rect);
        match face.and_then(|n| pk.faces.get(n)) {
            None => {
                p.circle_filled(rect.center(), FACE / 2.0, Color32::from_gray(if resp.hovered() { 85 } else { 60 }));
                p.text(rect.center(), Align2::CENTER_CENTER, crate::i18n::tr("All"), t.semibold(11.0), Color32::from_gray(235));
            }
            Some(FaceChip { texture: Some(tex), .. }) => round_image(&p, *tex, rect.center(), FACE / 2.0),
            Some(_) => {
                p.circle_filled(rect.center(), FACE / 2.0, Color32::from_gray(60));
            }
        }
        if pk.face == face {
            p.circle_stroke(rect.center(), FACE / 2.0 + 2.0, Stroke::new(2.0, t.accent));
        } else if resp.hovered() {
            p.circle_stroke(rect.center(), FACE / 2.0 + 1.0, Stroke::new(1.0, Color32::from_gray(200)));
        }
        let name = face.and_then(|n| pk.faces.get(n)).map_or_else(|| crate::i18n::tr("All Faces").to_string(), |f| f.name.clone());
        if resp.on_hover_text(name).clicked() {
            out.face = Some(face);
        }
    }
    // the note
    if let Some(g) = note {
        let y = card.top() + PAD + row1_h + GAP;
        register(ui, format!("{}:note", pk.id), Rect::from_min_size(pos2(card.left() + PAD, y), g.size()));
        p.galley(pos2(card.left() + PAD, y), g, Color32::from_gray(205));
    }
    // the parts dropdown, left of the button
    if !pk.parts.is_empty() {
        let w = drop_w - GAP;
        let rect = Rect::from_min_size(pos2(button.left() - GAP - w, card.top() + PAD + (FACE - PART_H) / 2.0), vec2(w, PART_H));
        out.parts = parts_menu::show(ui, pk, rect, card, &drop_text);
    } else {
        parts_menu::set_open(ui.ctx(), pk, false);
    }
    out
}

/// The rect registered for automation id `id` so far in this pass.
fn registered(ctx: &egui::Context, id: &str) -> Option<Rect> {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<crate::widgets::Registry>(egui::Id::unique("lc-registry"))
            .rects
            .iter()
            .rev()
            .find(|(k, _)| k == id)
            .map(|(_, r)| *r)
    })
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

mod parts_menu;

#[cfg(test)]
mod tests;
