//! The Info panel's camera card, editable: Camera, Lens, Focal length, Shutter speed, Aperture and ISO of one
//! photo or the selected photos. The file's values are never changed; an edit is stored beside them
//! (`photo.setMeta`) and shown instead, marked, with a reset button (↺, icon only, the same size and place on every
//! row) that gives the file's value back (`photo.resetExif`). Camera and Lens are searchable pickers over the known
//! names (`lightcraft_lensdb::names`) that also take any text; the numbers are typed (`35 mm`, `f/2.8`, `1/250`,
//! `2.5"`, `400`) and checked by the command. With several photos selected a field where they differ says Mixed.

use egui::{Rect, Sense, pos2, vec2};
use lightcraft_catalog::{ExifField, Meta, Photo, PhotoId};
use serde_json::{Value, json};

use super::{Icon, LightcraftApp, Tokens, register, text_button};
use crate::text_field::{Ending, TextField};

/// The reset button (and the marker beside it) are this size on every row, edited or not.
const RESET_SIZE: f32 = 18.0;
const MARKER_W: f32 = 10.0;
const LABEL_W: f32 = 92.0;
const ROW_H: f32 = 22.0;
/// Suggestions listed under a picker.
const SUGGESTIONS: usize = 8;

/// What the card edits and shows: the photos an edit goes to, and the values the rows show.
pub(super) struct Scope {
    /// The photos an edit goes to (the selection, or Target Photo's photo alone).
    targets: Vec<PhotoId>,
    /// The metadata the rows show (the active photo's, or the first selected photo's).
    shown: Meta,
    /// Per [`ExifField::ALL`]: the targets' values differ.
    mixed: [bool; 6],
    /// Per [`ExifField::ALL`]: some target has an edited value.
    edited: [bool; 6],
}

fn index(field: ExifField) -> usize {
    ExifField::ALL.iter().position(|f| *f == field).unwrap_or(0)
}

impl Scope {
    pub(super) fn of(app: &LightcraftApp, p: &Photo) -> Scope {
        let selected = app.session.selected();
        let several = selected.len() > 1;
        let for_selected = several && app.ui.settings.info_selected_photos;
        let targets = if (several && !for_selected) || selected.is_empty() { vec![p.id] } else { selected.clone() };
        let shown = super::info::shown_meta(&app.session.catalog, p, &selected, for_selected);
        let (mut mixed, mut edited) = ([false; 6], [false; 6]);
        for m in targets.iter().filter_map(|&id| app.session.catalog.photo(id)).map(|q| &q.meta) {
            for (i, f) in ExifField::ALL.into_iter().enumerate() {
                edited[i] |= m.exif_overridden(f);
                mixed[i] |= for_selected && m.exif_text(f, false) != shown.exif_text(f, false);
            }
        }
        Scope { targets, shown, mixed, edited }
    }

    fn any_edited(&self) -> bool {
        self.edited.iter().any(|e| *e)
    }

    fn ids(&self) -> Vec<u64> {
        self.targets.iter().map(|id| id.0).collect()
    }
}

/// `field`'s value as the rows show and take it: `35 mm`, `f/2.8`, `1/250 s`, `400`, or a name.
pub(super) fn display(field: ExifField, m: &Meta, file: bool) -> String {
    let t = m.exif_text(field, file);
    if t.is_empty() {
        return t;
    }
    match field {
        ExifField::Focal => format!("{t} mm"),
        ExifField::Aperture => format!("f/{t}"),
        ExifField::Shutter => format!("{t} s"),
        _ => t,
    }
}

/// An edit under way, kept in egui memory between frames; it saves to the photos it started on.
#[derive(Clone)]
struct Edit {
    text: String,
    start: String,
    ids: Vec<u64>,
    mixed: bool,
    typed: bool,
    /// The suggestion ↑/↓ moved to.
    sel: Option<usize>,
    /// The list was opened with ↓ before any typing.
    open: bool,
    /// Last frame's suggestion rows (where they were drawn, and what they pick), so a press on one is read before
    /// the field loses the focus to it.
    rows: Vec<(Rect, String)>,
}

impl Edit {
    fn new(value: &str, mixed: bool, ids: Vec<u64>) -> Self {
        Edit { text: value.into(), start: value.into(), ids, mixed, typed: false, sel: None, open: false, rows: Vec::new() }
    }
}

/// Camera and lens: a picker row each.
pub(super) fn camera_lens(app: &mut LightcraftApp, ui: &mut egui::Ui, scope: &Scope) {
    row(app, ui, scope, ExifField::Camera, None);
    ui.add_space(2.0);
    row(app, ui, scope, ExifField::Lens, None);
}

/// Focal length, shutter speed, aperture and ISO, and Reset All under them while anything is edited.
pub(super) fn capture(app: &mut LightcraftApp, ui: &mut egui::Ui, scope: &Scope) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(6.0);
    for f in [ExifField::Focal, ExifField::Shutter, ExifField::Aperture, ExifField::Iso] {
        row(app, ui, scope, f, Some(f.label()));
        ui.add_space(2.0);
    }
    if scope.any_edited() {
        ui.add_space(2.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = text_button(ui, "exifResetAll", crate::i18n::tr("Reset All"), false)
                .on_hover_text(crate::i18n::tr("Give every edited camera and exposure value of the selected photos the file's value back"));
            if r.clicked() {
                run_reset(app, ui.ctx(), scope, None);
            }
            ui.label(egui::RichText::new(crate::i18n::tr("Edited")).size(11.0).color(t.accent));
        });
    }
}

fn run_reset(app: &mut LightcraftApp, ctx: &egui::Context, scope: &Scope, field: Option<ExifField>) {
    let mut params = json!({"ids": scope.ids()});
    if let Some(f) = field {
        params["fields"] = json!([f.key()]);
    }
    if let Err(e) = app.run("photo.resetExif", params) {
        app.toast_error(ctx, e);
    }
}

fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, scope: &Scope, field: ExifField, label: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let i = index(field);
    let (mixed, edited) = (scope.mixed[i], scope.edited[i]);
    let picker = matches!(field, ExifField::Camera | ExifField::Lens);
    let name = crate::i18n::tr(field.label());
    let value = if mixed { String::new() } else { display(field, &scope.shown, false) };
    let id = egui::Id::unique(("exif-field", field.key()));
    let mut edit: Edit = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| Edit::new(&value, mixed, scope.ids()));
    let mut field_rect = Rect::NOTHING;
    let mut commit = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if let Some(l) = label {
            let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, ROW_H), Sense::hover());
            ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, crate::i18n::tr(l), egui::FontId::proportional(12.0), t.text_label);
        }
        let slot = MARKER_W + RESET_SIZE + 2.0 * ui.spacing().item_spacing.x;
        let width = (ui.available_width() - slot).max(40.0);

        // a press on a suggestion is read before the field can lose the focus to it
        let pressed_at = ui.input(|i| i.pointer.primary_pressed().then(|| i.pointer.interact_pos()).flatten());
        let picked = pressed_at.and_then(|pos| edit.rows.iter().find(|(r, _)| r.contains(pos)).map(|(_, n)| n.clone()));
        if let Some(n) = &picked {
            edit.text = n.clone();
            edit.typed = true;
        }

        let hint = if mixed {
            crate::i18n::tr("Mixed").to_string()
        } else if label.is_some() {
            "—".to_string()
        } else {
            name.to_string()
        };
        let widget = format!("field:exif:{}", field.key());
        let r = TextField::singleline(&widget, &mut edit.text)
            .hint(hint)
            .label(name)
            .width(width)
            .font(egui::FontId::proportional(12.0))
            .select_on_focus(true)
            .show(ui);
        field_rect = r.response.rect;
        edit.typed |= edit.text != edit.start;
        // ↑ / ↓ walk the suggestions, ↓ opens the list before anything is typed; Return takes the one walked to
        let mut chosen = picked;
        if picker && r.editing {
            let (down, up) = ui.input_mut(|i| {
                (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp))
            });
            let n = edit.rows.len();
            if down && n > 0 {
                edit.open = true;
                edit.sel = Some(edit.sel.map_or(0, |s| (s + 1).min(n - 1)));
            } else if down {
                edit.open = true;
            }
            if up && n > 0 {
                edit.sel = Some(edit.sel.map_or(0, |s| s.saturating_sub(1)));
            }
            if r.ending == Some(Ending::Return)
                && let Some((_, n)) = edit.sel.and_then(|s| edit.rows.get(s))
            {
                chosen = Some(n.clone());
            }
        }
        if let Some(n) = &chosen {
            edit.text = n.clone();
        }
        if chosen.is_some() || (r.committed() && (edit.text != edit.start || (edit.mixed && edit.typed))) {
            commit = Some(edit.text.trim().to_string());
        }
        if chosen.is_none() && r.cancelled() {
            edit.typed = false;
        }
        if chosen.is_some() {
            ui.memory_mut(|m| m.surrender_focus(crate::text_field::id(&widget)));
        }
        let live = r.editing && chosen.is_none();
        if live {
            ui.data_mut(|d| d.insert_temp(id, edit.clone()));
        } else {
            ui.data_mut(|d| d.remove::<Edit>(id));
        }

        // the marker and the reset button: the same slot on every row, drawn only while something is edited
        let (m, mresp) = ui.allocate_exact_size(vec2(MARKER_W, ROW_H), Sense::hover());
        if edited {
            ui.painter().circle_filled(m.center(), 3.0, t.accent);
            let file = if mixed { String::new() } else { display(field, &scope.shown, true) };
            let tip = if file.is_empty() {
                crate::i18n::tr("Edited; the file has no value").to_string()
            } else {
                crate::i18n::tr_format!("Edited; the file says {}", file)
            };
            mresp.on_hover_text(tip);
        }
        let size = vec2(RESET_SIZE, RESET_SIZE);
        if edited {
            let file = display(field, &scope.shown, true);
            let tip = if mixed || file.is_empty() {
                crate::i18n::tr("Reset to file value").to_string()
            } else {
                crate::i18n::tr_format!("Reset to file value ({})", file)
            };
            if crate::widgets::icon_button(ui, &format!("exifReset:{}", field.key()), Icon::Reset, size, false, true, &tip).clicked() {
                run_reset(app, ui.ctx(), scope, Some(field));
            }
        } else {
            let (r, _) = ui.allocate_exact_size(size, Sense::hover());
            register(ui, format!("exifResetSlot:{}", field.key()), r);
        }
    });
    if let Some(text) = commit {
        let v = if text.is_empty() { Value::Null } else { Value::String(text) };
        if let Err(e) = app.run("photo.setMeta", json!({"ids": edit.ids, field.key(): v})) {
            app.toast_error(ui.ctx(), e);
        }
    }
    if picker {
        suggestions(ui, id, field, field_rect);
    }
}

/// The picker's list under its field, while text is typed in it (or ↓ opened it): the known names matching, best
/// first. Free text is still taken as typed.
fn suggestions(ui: &mut egui::Ui, id: egui::Id, field: ExifField, under: Rect) {
    let Some(mut edit) = ui.data(|d| d.get_temp::<Edit>(id)) else { return };
    let query = if edit.typed { edit.text.clone() } else { String::new() };
    let list = if field == ExifField::Camera { lightcraft_engine::camera_names::cameras() } else { lightcraft_engine::camera_names::lenses() };
    let hits = if edit.typed || edit.open { list.search(&query, SUGGESTIONS) } else { Vec::new() };
    edit.rows.clear();
    if hits.is_empty() {
        edit.sel = None;
        ui.data_mut(|d| d.insert_temp(id, edit));
        return;
    }
    edit.sel = edit.sel.map(|s| s.min(hits.len() - 1));
    let t = Tokens::get(ui.ctx());
    let width = under.width().max(220.0);
    let hover = ui.input(|i| i.pointer.hover_pos());
    let layer = egui::Area::new(id.with("suggestions")).order(egui::Order::Foreground).fixed_pos(pos2(under.left(), under.bottom() + 2.0)).show(
        ui.ctx(),
        |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(3)).show(ui, |ui| {
                ui.set_width(width - 8.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for (n, h) in hits.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
                    crate::widgets::announce(&resp, h.name.clone());
                    register(ui, format!("exifSuggestion:{}:{n}", field.key()), r);
                    if edit.sel == Some(n) || hover.is_some_and(|p| r.contains(p)) {
                        ui.painter().rect_filled(r, 3.0, t.hover);
                    }
                    let extra = h.extra.first().map(String::as_str).unwrap_or("");
                    let font = egui::FontId::proportional(12.0);
                    let extra_g = ui.painter().layout_no_wrap(extra.to_string(), egui::FontId::proportional(10.5), t.text_dim);
                    let room = (r.width() - extra_g.size().x - 16.0).max(40.0);
                    let name_g = crate::widgets::one_line(ui.painter(), &h.name, font, t.text, room);
                    ui.painter().galley(pos2(r.left() + 6.0, r.center().y - name_g.size().y / 2.0), name_g, t.text);
                    ui.painter().galley(pos2(r.right() - 6.0 - extra_g.size().x, r.center().y - extra_g.size().y / 2.0), extra_g, t.text_dim);
                    edit.rows.push((r, h.name.clone()));
                }
            });
        },
    );
    let _ = layer;
    ui.data_mut(|d| d.insert_temp(id, edit));
}

#[cfg(test)]
mod tests;
