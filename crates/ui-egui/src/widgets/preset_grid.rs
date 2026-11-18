//! Fork (issue #344): the grid of cards every preset list with a preview picture shows (Light Presets, the Presets
//! panel, the Profiles browser, Camera Matching). A card is the preview picture on top and the name and its label
//! under it, wrapping, never cut short. How many cards share a row follows the width the grid has: one when it is
//! narrow, two when it is a bit wider, three when it is wide ([`columns`]); the cards share the width evenly and the
//! pictures keep the photo's shape. A card can have no picture: then it is the name and label only.
//!
//! The grid lays the cards out, paints their background, applied mark, name and label and makes each one a click
//! target; the list's own behaviour (the picture, hover preview, click, menu, overlays such as a star) goes in the
//! closure given to [`Grid::show`], which gets the card's rectangles and its response. Only the rows in view are
//! built, so a list of thousands of presets stays cheap: the rows' heights (their wrapped text) are worked out once for
//! a width and a list, not each frame.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

use crate::theme::Tokens;

/// The least width of a card (points): a row holds as many cards as keep to it.
pub const MIN_CARD_W: f32 = 108.0;
/// Most cards in a row, however wide the grid.
pub const MAX_COLUMNS: usize = 3;
/// Space between cards, across and down.
pub const GAP: f32 = 8.0;
/// Space around the text under the picture, and between the picture and the card's edge.
const PAD: f32 = 6.0;
/// The least height of the text area: room for a name being typed in its place.
const MIN_TEXT_H: f32 = 26.0;
/// The photo's shape (width over height) until a picture shows it, and the limits of a shape taken from a picture.
const DEFAULT_ASPECT: f32 = 1.5;
const ASPECT_RANGE: (f32, f32) = (0.6, 2.2);
const NAME_SIZE: f32 = 12.5;
const LABEL_SIZE: f32 = 11.0;

/// How many cards a row of `width` points holds: as many as keep to [`MIN_CARD_W`] with [`GAP`] between, from one to
/// [`MAX_COLUMNS`].
pub fn columns(width: f32) -> usize {
    if !width.is_finite() {
        return 1;
    }
    // (`as usize` saturates: a negative or NaN width is 0)
    let fit = ((width + GAP) / (MIN_CARD_W + GAP)).floor() as usize;
    fit.clamp(1, MAX_COLUMNS)
}

/// The width of one card of a row of `columns` cards across `width` points.
pub fn card_width(width: f32, columns: usize) -> f32 {
    let n = columns.max(1) as f32;
    ((width - GAP * (n - 1.0)) / n).max(1.0)
}

/// What one card says: its automation id, the name and label under the picture and its look.
#[derive(Clone, Debug, Default)]
pub struct Item {
    /// The id the control channel names the card by (`register`).
    pub key: String,
    pub name: String,
    /// Under the name, smaller: what the item is ("Single Light", "2 lights · Underwater"). Empty for none.
    pub label: String,
    /// The one in use: its card is marked.
    pub applied: bool,
    /// Not on offer now: its name is dimmed.
    pub dim: bool,
    /// The name is being edited in place: the grid leaves the text to the closure, which draws its field in
    /// [`Card::text`].
    pub editing: bool,
}

/// Where a card is and what the closure may draw in it.
pub struct Card {
    pub rect: Rect,
    /// The picture's rectangle (`None` for a card without one).
    pub picture: Option<Rect>,
    /// Where the name and label are (a name being typed goes here).
    pub text: Rect,
    pub resp: Response,
}

impl Card {
    /// Paint a texture as the card's picture (centre-cropped to its rectangle) and remember its shape for the next
    /// frame's layout: the cards then keep the photo's shape.
    pub fn paint_picture(&self, ui: &Ui, texture: egui::TextureId, size: [usize; 2]) {
        let Some(pic) = self.picture else { return };
        ui.painter().image(texture, pic, crate::panels::presets::cover_uv(pic, size), Color32::WHITE);
        remember_aspect(ui.ctx(), size);
    }

    /// The top-right corner of the picture (of the card without one), `size` big and `inset` in: where an overlay
    /// button or star goes.
    pub fn corner(&self, size: egui::Vec2, inset: f32) -> Rect {
        let at = self.picture.unwrap_or(self.rect);
        Rect::from_min_size(pos2(at.right() - inset - size.x, at.top() + inset), size)
    }
}

fn aspect_id() -> egui::Id {
    egui::Id::unique("preset-grid-aspect")
}

/// The shape of the last picture painted (all the lists show the active photo, so they share it).
fn remember_aspect(ctx: &egui::Context, size: [usize; 2]) {
    if size[0] == 0 || size[1] == 0 {
        return;
    }
    let aspect = (size[0] as f32 / size[1] as f32).clamp(ASPECT_RANGE.0, ASPECT_RANGE.1);
    let before = ctx.data(|d| d.get_temp::<f32>(aspect_id()));
    if before != Some(aspect) {
        ctx.data_mut(|d| d.insert_temp(aspect_id(), aspect));
        // (the layout of the frame already drawn used the old shape)
        if before.is_some() {
            ctx.request_repaint();
        }
    }
}

/// The heights of a grid's rows for one width and list: `tops[r]` is where row `r` starts, `tops[rows]` the whole height.
struct Rows {
    key: u64,
    tops: Vec<f32>,
}

/// A grid of cards: set it up, then [`Grid::show`] it.
pub struct Grid {
    salt: egui::Id,
    /// Space left and right of the grid, inside the width it is given.
    inset: (f32, f32),
    pictures: bool,
    /// Width kept clear at the right of a card's text when the card has no picture (an overlay button sits there).
    corner: f32,
}

impl Grid {
    /// A grid whose cards have a picture. `salt` is unique to the list on screen (two lists listing the same items
    /// differ).
    pub fn new(salt: impl Hash + std::fmt::Debug) -> Self {
        Self { salt: egui::Id::unique(("preset-grid", salt)), inset: (0.0, 0.0), pictures: true, corner: 0.0 }
    }

    /// Space left and right of the grid.
    pub fn inset(mut self, left: f32, right: f32) -> Self {
        self.inset = (left, right);
        self
    }

    /// The cards have no picture: name and label only. `corner` is the width an overlay at their top-right takes.
    pub fn without_pictures(mut self, corner: f32) -> Self {
        (self.pictures, self.corner) = (false, corner);
        self
    }

    /// Lay out and draw `items`; `card` runs for each card in view with its index.
    pub fn show(self, ui: &mut Ui, items: &[Item], mut card: impl FnMut(&mut Ui, usize, &Card)) {
        let t = Tokens::get(ui.ctx());
        let full = ui.available_width();
        let width = (full - self.inset.0 - self.inset.1).max(1.0);
        let cols = columns(width);
        let cw = card_width(width, cols);
        let aspect = ui.ctx().data(|d| d.get_temp::<f32>(aspect_id())).unwrap_or(DEFAULT_ASPECT);
        let pic_h = if self.pictures { (cw / aspect).round() } else { 0.0 };
        let text_w = (cw - 2.0 * PAD - if self.pictures { 0.0 } else { self.corner }).max(8.0);
        let rows = self.rows(ui, items, cols, text_w, pic_h);
        let n_rows = rows.tops.len().saturating_sub(1);
        let total = rows.tops.last().copied().unwrap_or(0.0);
        let (outer, _) = ui.allocate_exact_size(vec2(full, total), Sense::hover());
        let (left, top) = (outer.left() + self.inset.0, outer.top());
        // only the rows in view (a little beyond) are built
        let view = ui.clip_rect().expand(40.0);
        let first = rows.tops.partition_point(|&r| top + r < view.top()).saturating_sub(1).min(n_rows);
        let end = rows.tops.partition_point(|&r| top + r <= view.bottom()).min(n_rows);
        for row in first..end {
            let (y0, y1) = (rows.tops.get(row).copied().unwrap_or(0.0), rows.tops.get(row + 1).copied().unwrap_or(total));
            let h = (y1 - y0 - GAP).max(1.0);
            for col in 0..cols {
                let i = row * cols + col;
                let Some(item) = items.get(i) else { break };
                let rect = Rect::from_min_size(pos2(left + col as f32 * (cw + GAP), top + y0), vec2(cw, h));
                self.draw(ui, &t, i, item, rect, (cw, pic_h, text_w), &mut card);
            }
        }
    }

    /// The rows' tops, from the cache while the list, the width and the picture shape are the ones it was made for.
    fn rows(&self, ui: &Ui, items: &[Item], cols: usize, text_w: f32, pic_h: f32) -> Arc<Rows> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (cols, text_w.to_bits(), pic_h.to_bits(), self.pictures).hash(&mut h);
        for it in items {
            (&it.name, &it.label).hash(&mut h);
        }
        let key = h.finish();
        let id = self.salt.with("rows");
        if let Some(rows) = ui.ctx().data(|d| d.get_temp::<Arc<Rows>>(id))
            && rows.key == key
        {
            return rows;
        }
        let t = Tokens::get(ui.ctx());
        let painter = ui.painter();
        let mut tops = Vec::with_capacity(items.len() / cols.max(1) + 2);
        let mut y = 0.0_f32;
        for row in items.chunks(cols.max(1)) {
            tops.push(y);
            let text_h = row
                .iter()
                .map(|it| {
                    let (name, label) = text_of(painter, &t, it, text_w, false);
                    text_height(&name, label.as_deref())
                })
                .fold(MIN_TEXT_H, f32::max);
            y += card_height(self.pictures, pic_h, text_h) + GAP;
        }
        tops.push(y);
        let rows = Arc::new(Rows { key, tops });
        ui.ctx().data_mut(|d| d.insert_temp(id, rows.clone()));
        rows
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        ui: &mut Ui,
        t: &Tokens,
        i: usize,
        item: &Item,
        rect: Rect,
        (cw, pic_h, text_w): (f32, f32, f32),
        card: &mut impl FnMut(&mut Ui, usize, &Card),
    ) {
        let resp = ui.interact(rect, self.salt.with(i), Sense::click());
        crate::widgets::announce(&resp, item.name.clone());
        crate::widgets::register(ui, item.key.clone(), rect);
        let painter = ui.painter().clone();
        if item.applied {
            painter.rect_filled(rect, 4.0, t.tool_active);
        } else if resp.hovered() {
            painter.rect_filled(rect, 4.0, t.hover.gamma_multiply(0.7));
        }
        let picture = self.pictures.then(|| Rect::from_min_size(rect.min, vec2(cw, pic_h)));
        if let Some(pic) = picture {
            painter.rect_filled(pic, 4.0, t.canvas);
            if item.applied {
                painter.rect_stroke(pic.expand(1.0), 5.0, Stroke::new(2.0, t.pick), egui::StrokeKind::Outside);
            }
        }
        let text_top = picture.map_or(rect.top(), |p| p.bottom()) + PAD;
        let text = Rect::from_min_max(pos2(rect.left() + PAD, text_top), pos2(rect.left() + PAD + text_w, (rect.bottom() - PAD).max(text_top)));
        if !item.editing {
            let (name, label) = text_of(&painter, t, item, text_w, item.applied);
            painter.galley(text.min, name.clone(), t.text_label);
            if let Some(label) = label {
                painter.galley(pos2(text.left(), text.top() + name.size().y + 2.0), label, t.text_dim);
            }
        }
        let c = Card { rect, picture, text, resp };
        card(ui, i, &c);
        if c.resp.has_focus() {
            painter.rect_stroke(rect, 4.0, ui.visuals().selection.stroke, egui::StrokeKind::Inside);
        }
    }
}

/// The name's and the label's wrapped text of `item`, `w` wide.
fn text_of(painter: &egui::Painter, t: &Tokens, item: &Item, w: f32, applied: bool) -> (Arc<egui::Galley>, Option<Arc<egui::Galley>>) {
    let colour = if applied {
        t.text
    } else if item.dim {
        t.text_dim
    } else {
        t.text_label
    };
    let name = painter.layout(item.name.clone(), t.font(NAME_SIZE), colour, w);
    let label = (!item.label.is_empty()).then(|| painter.layout(item.label.clone(), t.font(LABEL_SIZE), t.text_dim, w));
    (name, label)
}

fn text_height(name: &egui::Galley, label: Option<&egui::Galley>) -> f32 {
    name.size().y + label.map_or(0.0, |l| l.size().y + 2.0)
}

/// A card's height: the picture, then padding around the text.
fn card_height(pictures: bool, pic_h: f32, text_h: f32) -> f32 {
    (if pictures { pic_h } else { 0.0 }) + PAD + text_h + PAD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_follow_the_width() {
        // narrow: one card; a bit wider: two; wide: three; never more
        assert_eq!(columns(0.0), 1);
        assert_eq!(columns(f32::NAN), 1);
        assert_eq!(columns(MIN_CARD_W), 1);
        assert_eq!(columns(2.0 * MIN_CARD_W + GAP - 1.0), 1);
        assert_eq!(columns(2.0 * MIN_CARD_W + GAP), 2);
        assert_eq!(columns(3.0 * MIN_CARD_W + 2.0 * GAP - 1.0), 2);
        assert_eq!(columns(3.0 * MIN_CARD_W + 2.0 * GAP), 3);
        assert_eq!(columns(5000.0), MAX_COLUMNS);
    }

    #[test]
    fn cards_share_the_width_and_keep_to_the_least() {
        for width in [120.0, 230.0, 240.0, 340.0, 520.0, 900.0] {
            let n = columns(width);
            let w = card_width(width, n);
            assert!(w >= MIN_CARD_W.min(width), "{width}: {w}");
            assert!((n as f32 * w + (n as f32 - 1.0) * GAP - width).abs() < 0.01, "{width}: {n} cards of {w}");
        }
    }

    /// Run the grid for a frame at `width` and return where the cards of the registered ids are.
    fn laid_out(width: f32, items: &[Item], clicks: Option<egui::Pos2>) -> (Vec<(String, Rect)>, Vec<usize>) {
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::apply(&ctx);
        let mut clicked = Vec::new();
        let mut registry = Vec::new();
        for frame in 0..3 {
            let mut raw = crate::headless::HeadlessView::raw_input(vec2(width, 3000.0), 1.0, f64::from(frame) / 60.0, vec![]);
            // (the click comes on the third frame, once the cards have a place)
            if let (Some(at), 2) = (clicks, frame) {
                raw.events.push(egui::Event::PointerMoved(at));
                for pressed in [true, false] {
                    raw.events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            clicked.clear();
            let mut out = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    Grid::new("test").show(ui, items, |_, i, card| {
                        if card.resp.clicked() {
                            clicked.push(i);
                        }
                    });
                });
            });
            out.textures_delta.clear();
            registry = crate::widgets::take_registry(&ctx);
        }
        (registry, clicked)
    }

    fn items(n: usize) -> Vec<Item> {
        (0..n)
            .map(|i| Item {
                key: format!("card:{i}"),
                name: if i == 1 { "A Name Long Enough To Need Several Lines In A Narrow Card".into() } else { format!("Preset {i}") },
                label: "Single Light".into(),
                ..Item::default()
            })
            .collect()
    }

    #[test]
    fn cards_are_placed_by_width_and_a_click_reaches_its_card() {
        let items = items(6);
        for (width, cols) in [(200.0_f32, 1), (250.0, 2), (400.0, 3)] {
            let (reg, _) = laid_out(width, &items, None);
            let rect = |i: usize| reg.iter().find(|(k, _)| k == &format!("card:{i}")).map(|(_, r)| *r).expect("card registered");
            let across = (0..6).filter(|&i| (rect(i).top() - rect(0).top()).abs() < 0.5).count();
            assert_eq!(across, cols.min(6), "{width}: cards on the first row");
            // the rows are as tall as their tallest card, nothing overlaps, everything stays in the width
            for i in 0..6 {
                for j in 0..i {
                    assert!(!rect(i).shrink(0.5).intersects(rect(j).shrink(0.5)), "{width}: card {i} over {j}");
                }
                assert!(rect(i).right() <= width + 0.5, "{width}: card {i} sticks out: {:?}", rect(i));
            }
            // a click on card 4 reaches only card 4
            let (_, clicked) = laid_out(width, &items, Some(rect(4).center()));
            assert_eq!(clicked, vec![4], "{width}");
        }
    }
}
