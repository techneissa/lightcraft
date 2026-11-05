//! The Info panel's camera card, editable: Camera, Lens, Focal length, Shutter speed, Aperture and ISO of one
//! photo or the selected photos. At rest each row is plain text, a dim label and a value, as in the official card;
//! a click on a value turns it into a text field (Return or a click away saves, Esc cancels). The file's values are
//! never changed; an edit is stored beside them (`photo.setMeta`) and shown instead, marked, with a small reset button
//! (↺, icon only, the same size and place on every row) that gives the file's value back (`photo.resetExif`). Camera
//! and Lens are searchable pickers over the known names (`lightcraft_lensdb::names`) that also take any text; the
//! numbers are typed (`35 mm`, `f/2.8`, `1/250`, `2.5"`, `400`) and checked by the command. A focal length or an
//! aperture the photo's lens cannot give is refused: the value shakes red, keeps its old text, and a small red "!" at
//! the row's end says why when clicked; the "!" also marks an edit a lens change left out of range. With several
//! photos selected a field where they differ says Mixed.
//!
//! The six rows are one height with a gap between them. A value too long for its column wraps onto more lines (the
//! rows below move down for that only); clicking it opens a field in its place that keeps the row's height, and the
//! picker's list is an overlay, so nothing else moves while a value is edited. The card's right edge holds the file
//! format badge (level with Camera) and the picture's size (dimensions level with Aperture, file size with ISO); the
//! rows leave room for them.

use egui::{Rect, Sense, pos2, vec2};
use lightcraft_catalog::exif_override::{format_aperture, format_focal};
use lightcraft_catalog::lens_limits::{LensRefusal, refusal};
use lightcraft_catalog::{ExifField, Meta, Photo, PhotoId};
use serde_json::{Value, json};

use super::{Icon, LightcraftApp, Tokens, register};
use crate::text_field::{Ending, TextField};

/// The reset button, the marker beside it and the alert are small, and the same on every row.
const RESET_SIZE: f32 = 16.0;
const MARKER_W: f32 = 8.0;
const ALERT_SIZE: f32 = 14.0;
/// The gap between the labels and the values, as the official card's grid has.
const LABEL_GAP: f32 = 24.0;
/// A one-line row's height (the edit field fits in it) and the gap between rows.
const ROW_H: f32 = 24.0;
const ROW_GAP: f32 = 6.0;
/// The rows from top to bottom, as the card lists them.
const ROWS: [ExifField; 6] = [ExifField::Camera, ExifField::Lens, ExifField::Focal, ExifField::Shutter, ExifField::Aperture, ExifField::Iso];
/// The most lines a value wraps to; more is cut with an ellipsis.
const MAX_LINES: usize = 4;
/// How long a refused value shakes, in seconds.
const SHAKE_SECS: f64 = 0.3;
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
    /// Per [`ExifField::ALL`]: why an edited value does not fit its photo's lens (a lens change can leave one so).
    alert: [Option<LensRefusal>; 6],
    /// The lens the card shows, which a refusal is remembered against.
    lens: String,
}

/// A refused edit, kept until the field is edited or reset, or the photos or their lens change.
#[derive(Clone)]
struct Refused {
    ids: Vec<u64>,
    lens: String,
    reason: LensRefusal,
    at: f64,
}

fn refused_id(field: ExifField) -> egui::Id {
    egui::Id::unique(("exif-refused", field.key()))
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
        let mut alert: [Option<LensRefusal>; 6] = Default::default();
        for m in targets.iter().filter_map(|&id| app.session.catalog.photo(id)).map(|q| &q.meta) {
            for (i, f) in ExifField::ALL.into_iter().enumerate() {
                edited[i] |= m.exif_overridden(f);
                mixed[i] |= for_selected && m.exif_text(f, false) != shown.exif_text(f, false);
                if alert[i].is_none() {
                    alert[i] = refusal(m, f);
                }
            }
        }
        let lens = shown.effective_lens().to_string();
        Scope { targets, shown, mixed, edited, alert, lens }
    }

    /// The refusal remembered for `field`, while it still applies (same photos, same lens).
    fn refused(&self, ctx: &egui::Context, field: ExifField) -> Option<Refused> {
        let r: Refused = ctx.data(|d| d.get_temp(refused_id(field)))?;
        (r.ids == self.ids() && r.lens == self.lens).then_some(r)
    }

    /// Why `field`'s row carries a "!": an edit out of its lens's range, or a refused edit.
    fn flag(&self, ctx: &egui::Context, field: ExifField) -> Option<LensRefusal> {
        self.alert[index(field)].or_else(|| self.refused(ctx, field).map(|r| r.reason))
    }

    /// Whether any row carries a "!" (every row then keeps room for it, so the rows line up).
    fn any_flag(&self, ctx: &egui::Context) -> bool {
        ExifField::ALL.into_iter().any(|f| self.flag(ctx, f).is_some())
    }

    /// Why `text` as `field`'s value does not fit the lens of some target photo, if it does not.
    fn precheck(&self, app: &LightcraftApp, field: ExifField, text: &str) -> Option<LensRefusal> {
        self.targets.iter().filter_map(|&id| app.session.catalog.photo(id)).find_map(|q| {
            let mut m = q.meta.clone();
            m.set_exif(field, Some(text)).ok()?;
            refusal(&m, field)
        })
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
    /// Frames since the field opened: it takes the focus over the first two.
    frames: u8,
}

impl Edit {
    fn new(value: &str, mixed: bool, ids: Vec<u64>) -> Self {
        Edit { text: value.into(), start: value.into(), ids, mixed, typed: false, sel: None, open: false, rows: Vec::new(), frames: 0 }
    }
}

/// The value as the card shows it at rest, as the official card words it: `35 mm`, `1/250 sec`, `f / 2.8`, `400`.
fn rest_text(field: ExifField, m: &Meta) -> String {
    let m = m.shown();
    let text = match field {
        ExifField::Camera => m.camera.clone(),
        ExifField::Lens => m.lens.clone(),
        ExifField::Focal => m.focal_mm.map(|f| format!("{f:.1} mm").replace(".0 mm", " mm")).unwrap_or_default(),
        ExifField::Shutter if m.shutter.is_empty() => String::new(),
        ExifField::Shutter => crate::i18n::tr_format!("{} sec", m.shutter),
        ExifField::Aperture => m.aperture.map(|f| format!("f / {f:.1}").replace(".0", "")).unwrap_or_default(),
        ExifField::Iso => m.iso.map(|i| i.to_string()).unwrap_or_default(),
    };
    if text.is_empty() { "—".to_string() } else { text }
}

/// The width of the label column: the widest of the six labels, and the gap the official card's grid leaves.
fn label_col(ui: &egui::Ui) -> f32 {
    let font = egui::FontId::proportional(12.0);
    let widest = ExifField::ALL
        .into_iter()
        .map(|f| ui.painter().layout_no_wrap(crate::i18n::tr(f.label()).to_string(), font.clone(), egui::Color32::WHITE).size().x)
        .fold(0.0_f32, f32::max);
    widest + LABEL_GAP
}

/// What the card shows at its right edge, level with some rows.
pub(super) struct Side {
    /// The file format badge ("ARW"), level with Camera.
    pub format: String,
    /// The picture's size in pixels, level with Aperture.
    pub dims: String,
    /// The file's size, level with ISO.
    pub size: String,
}

/// What stands at a row's right edge.
#[derive(Clone, Copy)]
enum Trailing<'a> {
    None,
    Badge(&'a str),
    Text(&'a str),
}

/// The six rows, one height with a gap between them.
pub(super) fn card(app: &mut LightcraftApp, ui: &mut egui::Ui, scope: &Scope, side: &Side) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        for f in ROWS {
            let trailing = match f {
                ExifField::Camera => Trailing::Badge(&side.format),
                ExifField::Aperture => Trailing::Text(&side.dims),
                ExifField::Iso => Trailing::Text(&side.size),
                _ => Trailing::None,
            };
            row(app, ui, scope, f, trailing);
        }
    });
}

/// `text` laid out in the card's value style, wrapped at `width` onto at most [`MAX_LINES`] lines (a word longer than
/// the width is broken where it must).
fn wrapped(painter: &egui::Painter, text: &str, color: egui::Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let layout = |break_anywhere: bool| {
        let mut job = egui::text::LayoutJob::simple(text.to_string(), egui::FontId::proportional(12.0), color, width.max(1.0));
        job.wrap.max_rows = MAX_LINES;
        job.wrap.break_anywhere = break_anywhere;
        painter.layout_job(job)
    };
    let galley = layout(false);
    if galley.size().x > width + 0.5 { layout(true) } else { galley }
}

/// The height of a line of the card's text.
fn line_height(ui: &egui::Ui) -> f32 {
    ui.painter().layout_no_wrap("Ag".to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE).size().y
}

fn run_reset(app: &mut LightcraftApp, ctx: &egui::Context, scope: &Scope, field: Option<ExifField>) {
    let mut params = json!({"ids": scope.ids()});
    if let Some(f) = field {
        params["fields"] = json!([f.key()]);
    }
    for f in ExifField::ALL.into_iter().filter(|f| field.is_none_or(|only| only == *f)) {
        ctx.data_mut(|d| d.remove::<Refused>(refused_id(f)));
    }
    if let Err(e) = app.run("photo.resetExif", params) {
        app.toast_error(ctx, e);
    }
}

/// A refused value: remembered (for the "!" and the shake), and nothing is saved.
fn refuse(ctx: &egui::Context, scope: &Scope, field: ExifField, reason: LensRefusal) {
    let at = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(refused_id(field), Refused { ids: scope.ids(), lens: scope.lens.clone(), reason, at }));
    ctx.request_repaint();
}

/// A refusal's sentence in the interface language.
fn reason_text(r: &LensRefusal) -> String {
    match *r {
        LensRefusal::Aperture(limit) => crate::i18n::tr_format!("This lens opens to f/{} at most", format_aperture(limit)),
        LensRefusal::Focal(lo, hi) if lo == hi => crate::i18n::tr_format!("This lens is {} mm", format_focal(lo)),
        LensRefusal::Focal(lo, hi) => crate::i18n::tr_format!("This lens covers {}–{} mm", format_focal(lo), format_focal(hi)),
    }
}

/// Forget every refused edit: a new edit begins, so the old refusal is over.
fn forget_refusals(ctx: &egui::Context) {
    for f in ExifField::ALL {
        ctx.data_mut(|d| d.remove::<Refused>(refused_id(f)));
    }
}

/// How far a refused value is pushed aside now (a quick shake that settles), and whether it is still moving.
fn shake(ctx: &egui::Context, r: &Refused) -> (f32, bool) {
    let age = ctx.input(|i| i.time) - r.at;
    if !(0.0..SHAKE_SECS).contains(&age) {
        return (0.0, false);
    }
    let fade = 1.0 - (age / SHAKE_SECS) as f32;
    ctx.request_repaint();
    ((age as f32 * 70.0).sin() * 3.0 * fade, true)
}

/// One row: a dim label, and the value as plain text; a click turns the value into a text field. Right of the value:
/// the edited marker and the reset button (while anything is edited, in every row, so they line up) and the "!".
/// The row is as tall as its value wraps to, whether or not the value is being edited, so an edit moves nothing.
fn row(app: &mut LightcraftApp, ui: &mut egui::Ui, scope: &Scope, field: ExifField, trailing: Trailing) {
    let t = Tokens::get(ui.ctx());
    let i = index(field);
    let mixed = scope.mixed[i];
    let name = crate::i18n::tr(field.label());
    let value = if mixed { String::new() } else { display(field, &scope.shown, false) };
    let id = egui::Id::unique(("exif-field", field.key()));
    let open: Option<Edit> = ui.data(|d| d.get_temp(id));
    let (any_edited, any_flag) = (scope.any_edited(), scope.any_flag(ui.ctx()));
    let mut edit = open.clone().unwrap_or_else(|| Edit::new(&value, mixed, scope.ids()));
    let (field_rect, commit);
    let space_x = 4.0;
    // what stands at the row's right edge, and the room the value leaves for it
    let trail = match trailing {
        Trailing::None => None,
        Trailing::Badge(s) => Some((ui.painter().layout_no_wrap(s.to_uppercase(), egui::FontId::proportional(10.5), t.text_label), 10.0)),
        Trailing::Text(s) => Some((ui.painter().layout_no_wrap(s.to_string(), egui::FontId::proportional(12.0), t.text_label), 0.0)),
    };
    let reserve = trail.as_ref().map_or(0.0, |(g, pad)| g.size().x + pad + 2.0 * space_x);
    let label_w = label_col(ui);
    let mut slots = 0.0;
    if any_edited {
        slots += MARKER_W + RESET_SIZE + 2.0 * space_x;
    }
    if any_flag {
        slots += ALERT_SIZE + space_x;
    }
    let width = (ui.available_width() - label_w - slots - reserve).max(40.0);
    // the value wraps at the column's width; the row is as tall as that
    let shown = if mixed { crate::i18n::tr("Mixed").to_string() } else { rest_text(field, &scope.shown) };
    let color = if mixed { t.text_dim } else { t.text_label };
    let galley = wrapped(ui.painter(), &shown, color, width);
    let pad = ((ROW_H - line_height(ui)) / 2.0).max(0.0);
    let height = ROW_H.max(galley.size().y + 2.0 * pad);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    register(ui, format!("exifRow:{}", field.key()), rect);
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Min)));
    {
        let ui = &mut inner;
        ui.spacing_mut().item_spacing.x = space_x;
        let (r, _) = ui.allocate_exact_size(vec2(label_w - space_x, ROW_H), Sense::hover());
        ui.painter().text(r.left_center(), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(12.0), t.text_dim);
        if open.is_some() {
            (field_rect, commit) = open_field(ui, scope, field, id, &mut edit, width);
        } else {
            (field_rect, commit) = (rest_value(ui, scope, field, id, (width, height), (&value, &shown, &galley, pad)), None);
        }
        // the marks sit halfway down the value block, so a wrapped value has them between its lines
        let rest = Rect::from_min_max(pos2(ui.cursor().left(), rect.top()), rect.max);
        let mut marks = ui.new_child(egui::UiBuilder::new().max_rect(rest).layout(egui::Layout::left_to_right(egui::Align::Center)));
        marks.spacing_mut().item_spacing.x = space_x;
        cluster(app, &mut marks, scope, field, height);
    }
    if let Some((g, pad)) = trail {
        let size = g.size() + vec2(pad, if pad > 0.0 { 2.0 } else { 0.0 });
        let at = egui::Rect::from_min_size(pos2(rect.right() - size.x, rect.top() + (ROW_H - size.y) / 2.0), size);
        register(ui, format!("exifTrailing:{}", field.key()), at);
        if pad > 0.0 {
            ui.painter().rect_filled(at, 3.0, t.chrome);
        }
        ui.painter().galley(at.center() - g.size() / 2.0, g, t.text_label);
    }
    if let Some(text) = commit {
        let v = if text.is_empty() { Value::Null } else { Value::String(text.clone()) };
        if let Some(why) = scope.precheck(app, field, &text).filter(|_| !text.is_empty()) {
            // the old value stays; the row shakes, and its "!" says why
            refuse(ui.ctx(), scope, field, why);
        } else {
            ui.ctx().data_mut(|d| d.remove::<Refused>(refused_id(field)));
            if let Err(e) = app.run("photo.setMeta", json!({"ids": edit.ids, field.key(): v})) {
                app.toast_error(ui.ctx(), e);
            }
        }
    }
    if matches!(field, ExifField::Camera | ExifField::Lens) {
        suggestions(ui, id, field, field_rect);
    }
}

/// The value at rest: plain text, wrapped onto the lines `galley` has (a refused one shakes in a red outline); a
/// click opens its field.
fn rest_value(
    ui: &mut egui::Ui,
    scope: &Scope,
    field: ExifField,
    id: egui::Id,
    (width, height): (f32, f32),
    (value, shown, galley, pad): (&str, &str, &std::sync::Arc<egui::Galley>, f32),
) -> Rect {
    let t = Tokens::get(ui.ctx());
    let mixed = scope.mixed[index(field)];
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    crate::widgets::register(ui, format!("exifValue:{}", field.key()), rect);
    crate::widgets::announce(&resp, format!("{}: {shown}", crate::i18n::tr(field.label())));
    let (dx, shaking) = scope.refused(ui.ctx(), field).map_or((0.0, false), |r| shake(ui.ctx(), &r));
    let at = rect.translate(vec2(dx, 0.0));
    if resp.hovered() && !shaking {
        ui.painter().rect_filled(rect.expand2(vec2(3.0, 0.0)), 3.0, t.hover.gamma_multiply(0.6));
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    if shaking {
        ui.painter().rect_stroke(at.expand2(vec2(3.0, 0.0)), 3.0, egui::Stroke::new(1.0, t.reject), egui::StrokeKind::Inside);
    }
    let color = if mixed { t.text_dim } else { t.text_label };
    ui.painter().with_clip_rect(rect.expand2(vec2(4.0, 0.0))).galley(pos2(at.left(), at.top() + pad), galley.clone(), color);
    if resp.clicked() {
        // a new edit: an earlier refusal is over, and Camera and Lens open their list of names at once
        forget_refusals(ui.ctx());
        let mut edit = Edit::new(value, mixed, scope.ids());
        edit.open = matches!(field, ExifField::Camera | ExifField::Lens);
        ui.data_mut(|d| d.insert_temp(id, edit));
        ui.ctx().request_repaint();
    }
    rect
}

/// The open field: where it is, and the text to save when the edit ended with it kept.
fn open_field(ui: &mut egui::Ui, scope: &Scope, field: ExifField, id: egui::Id, edit: &mut Edit, width: f32) -> (Rect, Option<String>) {
    let (mixed, picker) = (scope.mixed[index(field)], matches!(field, ExifField::Camera | ExifField::Lens));
    // a press on a suggestion is read before the field can lose the focus to it
    let pressed_at = ui.input(|i| i.pointer.primary_pressed().then(|| i.pointer.interact_pos()).flatten());
    let picked = pressed_at.and_then(|pos| edit.rows.iter().find(|(r, _)| r.contains(pos)).map(|(_, n)| n.clone()));
    if let Some(n) = &picked {
        edit.text = n.clone();
        edit.typed = true;
    }
    let hint = if mixed { crate::i18n::tr("Mixed").to_string() } else { "—".to_string() };
    let widget = format!("field:exif:{}", field.key());
    let r = TextField::singleline(&widget, &mut edit.text)
        .hint(hint)
        .label(crate::i18n::tr(field.label()))
        .width(width)
        .font(egui::FontId::proportional(12.0))
        .select_on_focus(true)
        .begin_edit(edit.frames == 0)
        .show(ui);
    edit.typed |= edit.text != edit.start;
    // ↑ / ↓ walk the suggestions, ↓ opens the list before anything is typed; Return takes the one walked to
    let mut chosen = picked;
    if picker && r.editing {
        let (down, up) =
            ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
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
    let commit =
        (chosen.is_some() || (r.committed() && (edit.text != edit.start || (edit.mixed && edit.typed)))).then(|| edit.text.trim().to_string());
    if chosen.is_none() && r.cancelled() {
        edit.typed = false;
    }
    if chosen.is_some() {
        ui.memory_mut(|m| m.surrender_focus(crate::text_field::id(&widget)));
    }
    // the field takes the focus over its first frames; after that, losing it (or an ending) closes it
    let starting = edit.frames < 2 && r.ending.is_none();
    if starting {
        r.response.request_focus();
    }
    edit.frames = edit.frames.saturating_add(1);
    if (r.editing || starting) && chosen.is_none() {
        ui.data_mut(|d| d.insert_temp(id, edit.clone()));
    } else {
        ui.data_mut(|d| d.remove::<Edit>(id));
    }
    (r.response.rect, commit)
}

/// Right of the value: the edited marker and reset button (in every row while anything is edited, so they line up),
/// then the "!" (where the value does not fit the lens; a click says why).
fn cluster(app: &mut LightcraftApp, ui: &mut egui::Ui, scope: &Scope, field: ExifField, row_h: f32) {
    let t = Tokens::get(ui.ctx());
    let (mixed, edited) = (scope.mixed[index(field)], scope.edited[index(field)]);
    if scope.any_edited() {
        let (m, mresp) = ui.allocate_exact_size(vec2(MARKER_W, row_h), Sense::hover());
        let size = vec2(RESET_SIZE, RESET_SIZE);
        if edited {
            ui.painter().circle_filled(m.center(), 2.5, t.accent);
            let file = if mixed { String::new() } else { display(field, &scope.shown, true) };
            mresp.on_hover_text(if file.is_empty() {
                crate::i18n::tr("Edited; the file has no value").to_string()
            } else {
                crate::i18n::tr_format!("Edited; the file says {}", file)
            });
            let tip = if file.is_empty() {
                crate::i18n::tr("Reset to file value").to_string()
            } else {
                crate::i18n::tr_format!("Reset to file value ({})", file)
            };
            if crate::widgets::icon_button(ui, &format!("exifReset:{}", field.key()), Icon::Reset, size, false, true, &tip).clicked() {
                run_reset(app, ui.ctx(), scope, Some(field));
            }
        } else {
            ui.allocate_exact_size(size, Sense::hover());
        }
    }
    if scope.any_flag(ui.ctx()) {
        let flag = scope.flag(ui.ctx(), field);
        let (r, resp) = ui.allocate_exact_size(vec2(ALERT_SIZE, ALERT_SIZE), if flag.is_some() { Sense::click() } else { Sense::hover() });
        if let Some(reason) = flag.as_ref().map(reason_text) {
            register(ui, format!("exifAlert:{}", field.key()), r);
            crate::icons::paint(ui.painter(), r, Icon::Alert, t.reject);
            crate::widgets::announce(&resp, reason.clone());
            egui::Popup::menu(&resp).show(|ui| {
                ui.label(egui::RichText::new(reason).size(12.0).color(t.text));
            });
        }
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
