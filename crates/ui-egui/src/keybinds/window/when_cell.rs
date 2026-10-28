//! The When column of the Keyboard Shortcuts window. At rest a condition is a row of readable
//! chips (`Loupe View`, `Not Typing`); a click turns the cell into a text box with an autocomplete
//! list (keys, their negated forms, operators, values), and `+` opens a small builder (key, then
//! Is / Is Not, then value) for people who would rather not type. Nothing invalid is saved: the
//! reason shows under the box in red.

use egui::{Align2, FontId, Id, Key, Modifiers, Order, PopupCloseBehavior, Rect, RichText, Sense, pos2, vec2};

use crate::LightcraftApp;
use crate::keybinds::model::conditions;
use crate::keybinds::when::keys::{KEYS, Kind};
use crate::keybinds::when::suggest::{self, ItemKind};
use crate::keybinds::when::{Chip, Expr, expr, parse};
use crate::text_field::{self, Ending, TextField};
use crate::theme::Tokens;
use crate::widgets::register;

/// The automation id of the text box.
pub const FIELD: &str = "field:shortcutWhen";
const PLUS_W: f32 = 22.0;
const MAX_ITEMS: usize = 40;
/// Rows of the list that show before it scrolls.
const LIST_ROWS: f32 = 10.0;
const LIST_ROW_H: f32 = 22.0;

/// A condition being typed.
#[derive(Clone, Debug)]
pub struct Edit {
    pub id: &'static str,
    pub text: String,
    /// The highlighted suggestion.
    pub selected: usize,
    /// The arrow keys moved the highlight: Enter takes the suggestion rather than saving.
    pub navigated: bool,
    /// Return or leaving the box was tried with text that is not a condition: show every problem.
    pub tried: bool,
    /// First frame: take the focus.
    pub fresh: bool,
    /// Where the list was drawn last frame (a click in it is not leaving the box).
    pub list: Option<Rect>,
}

/// The condition cell of command `id` in `rect`; `note` is why it clashes with another command.
pub fn cell(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, id: &'static str, rect: Rect, note: Option<&str>) {
    if app.keybinds.when_edit.as_ref().is_some_and(|e| e.id == id) {
        editor(app, ui, t, id, rect);
    } else {
        view(app, ui, t, id, rect, note);
    }
}

fn start_edit(app: &mut LightcraftApp, id: &'static str) {
    let text = conditions::text(&app.ui.settings.keymap, id).unwrap_or_default();
    app.keybinds.when_edit = Some(Edit { id, text, selected: 0, navigated: false, tried: false, fresh: true, list: None });
}

/// Save the condition of `id` (blank = always).
fn save(app: &mut LightcraftApp, ctx: &egui::Context, id: &str, text: &str) {
    let when = if text.trim().is_empty() { serde_json::Value::Null } else { serde_json::json!(text) };
    if let Err(e) = app.run("app.setShortcut", serde_json::json!({"id": id, "when": when})) {
        app.toast_for(ctx, e, 3.0);
    }
}

// ------------------------------------------------------------------ at rest

fn chip_fill(t: &Tokens, negated: bool, warn: bool) -> egui::Color32 {
    if warn {
        t.caution.gamma_multiply(0.28)
    } else if negated {
        t.reject.gamma_multiply(0.26)
    } else {
        t.accent.gamma_multiply(0.24)
    }
}

/// Draw the chips of `expr` from `x`; the width they take.
fn draw_chips(painter: &egui::Painter, t: &Tokens, expr: &Expr, left: f32, cy: f32, warn: bool) -> f32 {
    let mut x = left;
    for chip in expr.chips() {
        match chip {
            Chip::Atom(text) => {
                let galley = painter.layout_no_wrap(text.clone(), t.font(11.0), t.text);
                let w = galley.size().x + 12.0;
                let r = Rect::from_min_size(pos2(x, cy - 9.0), vec2(w, 18.0));
                painter.rect_filled(r, 9.0, chip_fill(t, text.starts_with("Not "), warn));
                painter.galley(pos2(x + 6.0, cy - galley.size().y / 2.0), galley, t.text);
                x += w + 4.0;
            }
            Chip::Word(w) => {
                let r = painter.text(pos2(x, cy), Align2::LEFT_CENTER, w, t.font(10.5), t.text_dim);
                x += r.width() + 4.0;
            }
            Chip::Open | Chip::Close => {
                let c = if matches!(chip, Chip::Open) { "(" } else { ")" };
                let r = painter.text(pos2(x, cy), Align2::LEFT_CENTER, c, t.font(11.5), t.text_dim);
                x += r.width() + 2.0;
            }
        }
    }
    x - left
}

fn view(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, id: &'static str, rect: Rect, note: Option<&str>) {
    let text = conditions::text(&app.ui.settings.keymap, id);
    let parsed = text.as_deref().and_then(|s| parse(s).ok().flatten());
    let never = parsed.as_ref().is_some_and(|e| !expr::satisfiable(e));
    let plus = Rect::from_min_size(pos2(rect.right() - PLUS_W, rect.center().y - 10.0), vec2(PLUS_W, 20.0));
    let body = Rect::from_min_max(rect.min, pos2(plus.left() - 2.0, rect.max.y));
    let r = ui.interact(body, Id::unique("when-cell").with(id), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::tr("Condition")));
    register(ui, format!("button:shortcutWhen-{id}"), body);
    if r.hovered() && !egui::Popup::is_any_open(ui.ctx()) {
        ui.painter().rect_filled(body.shrink2(vec2(0.0, 3.0)), 4.0, t.hover);
    }
    let painter = ui.painter().with_clip_rect(body);
    let cy = body.center().y;
    match &parsed {
        Some(e) => {
            draw_chips(&painter, t, e, body.left() + 4.0, cy, note.is_some() || never);
        }
        None => {
            painter.text(pos2(body.left() + 6.0, cy), Align2::LEFT_CENTER, crate::i18n::tr("Always"), t.font(11.5), t.text_dim);
        }
    }
    let mut tip = match &text {
        Some(text) => text.clone(),
        None => crate::i18n::tr("Works everywhere. Click to limit it to a context.").to_string(),
    };
    if never {
        tip.push_str("\n");
        tip.push_str(&crate::i18n::tr("This condition can never be true, so the shortcut never works."));
    }
    if let Some(note) = note {
        tip.push_str("\n");
        tip.push_str(note);
    }
    let r = r.on_hover_text(tip);
    if r.clicked() {
        start_edit(app, id);
    }
    crate::widgets::context_menu(&r, |ui| {
        if ui.button(crate::i18n::tr("Edit Condition…")).clicked() {
            start_edit(app, id);
            ui.close();
        }
        if ui.add_enabled(text.is_some(), egui::Button::new(crate::i18n::tr("Clear Condition"))).clicked() {
            save(app, ui.ctx(), id, "");
            ui.close();
        }
    });
    plus_button(app, ui, t, id, plus, parsed);
}

// ------------------------------------------------------------------ the "+" builder

/// Where the builder is: no key picked yet, or the key and whether the comparison is negated.
#[derive(Clone, Copy, Default)]
struct Builder {
    key: Option<usize>,
    negated: bool,
}

fn plus_button(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, id: &'static str, rect: Rect, current: Option<Expr>) {
    let r = ui.interact(rect, Id::unique("when-plus").with(id), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, crate::i18n::tr("Add Condition")));
    register(ui, format!("button:shortcutWhenAdd-{id}"), rect);
    let color = if r.hovered() || egui::Popup::is_any_open(ui.ctx()) { t.text } else { t.text_dim };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, "+", t.semibold(15.0), color);
    let r = r.on_hover_text(crate::i18n::tr("Add Condition"));
    let state_id = Id::unique("when-builder").with(id);
    let mut chosen: Option<Expr> = None;
    let popup = egui::Popup::menu(&r).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(300.0);
        let mut b: Builder = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
        chosen = builder_body(ui, t, &mut b);
        if chosen.is_some() {
            b = Builder::default();
            ui.close();
        }
        ui.data_mut(|d| d.insert_temp(state_id, b));
    });
    if popup.is_none() {
        ui.data_mut(|d| d.remove_temp::<Builder>(state_id));
    }
    if let Some(added) = chosen {
        let combined = match current {
            Some(e) => e.and(added),
            None => added,
        };
        save(app, ui.ctx(), id, &combined.text());
    }
}

/// One clickable row of the builder: a title and a dim description.
fn menu_row(ui: &mut egui::Ui, t: &Tokens, widget: String, title: &str, detail: &str, selected: bool) -> bool {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 24.0), Sense::click());
    register(ui, widget, rect);
    if r.hovered() || selected {
        ui.painter().rect_filled(rect, 4.0, if selected { t.accent.gamma_multiply(0.28) } else { t.hover });
    }
    ui.painter().text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, title, t.font(12.5), t.text);
    ui.painter().text(pos2(rect.left() + 130.0, rect.center().y), Align2::LEFT_CENTER, detail, t.font(11.0), t.text_dim);
    r.clicked()
}

/// The builder's two steps. The condition the user finished, if they did.
fn builder_body(ui: &mut egui::Ui, t: &Tokens, b: &mut Builder) -> Option<Expr> {
    ui.spacing_mut().item_spacing.y = 2.0;
    let Some(key) = b.key else {
        ui.label(RichText::new(crate::i18n::tr("Add Condition")).font(t.semibold(12.5)).color(t.accent));
        ui.label(RichText::new(crate::i18n::tr("The shortcut works only while…")).size(11.0).color(t.text_dim));
        ui.add_space(4.0);
        for (i, k) in KEYS.iter().enumerate() {
            if menu_row(ui, t, format!("menu:whenKey-{}", k.name), crate::i18n::tr(k.title), crate::i18n::tr(k.desc), false) {
                b.key = Some(i);
            }
        }
        return None;
    };
    let def = KEYS.get(key)?;
    ui.horizontal(|ui| {
        let back = ui.small_button(crate::i18n::tr("Back"));
        register(ui, "menu:whenBack", back.rect);
        if back.clicked() {
            *b = Builder::default();
        }
        ui.label(RichText::new(crate::i18n::tr(def.title)).font(t.semibold(12.5)).color(t.accent));
    });
    // Is / Is Not, for every key (an on / off key too: "Is Not Typing")
    ui.horizontal(|ui| {
        for (negated, label, widget) in [(false, "Is", "menu:whenIs"), (true, "Is Not", "menu:whenIsNot")] {
            let r = ui.selectable_label(b.negated == negated, crate::i18n::tr(label));
            register(ui, widget, r.rect);
            if r.clicked() {
                b.negated = negated;
            }
        }
    });
    ui.add_space(2.0);
    match &def.kind {
        Kind::Enum(values) => {
            for (i, v) in values.iter().enumerate() {
                if menu_row(ui, t, format!("menu:whenValue-{}", v.name), crate::i18n::tr(v.chip), crate::i18n::tr(v.desc), false) {
                    return Some(Expr::Is { key, value: i, equal: !b.negated });
                }
            }
        }
        Kind::Bool { chip } => {
            if menu_row(ui, t, format!("menu:whenValue-{}", def.name), crate::i18n::tr(chip), crate::i18n::tr(def.desc), false) {
                return Some(if b.negated { Expr::Not(Box::new(Expr::Key(key))) } else { Expr::Key(key) });
            }
        }
    }
    None
}

// ------------------------------------------------------------------ typing

fn set_cursor(ctx: &egui::Context, field: Id, text: &str, byte: usize) {
    let chars = text.get(..byte.min(text.len())).map_or(0, |h| h.chars().count());
    let mut state = egui::text_edit::TextEditState::load(ctx, field).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(chars))));
    state.store(ctx, field);
}

fn cursor_byte(ctx: &egui::Context, field: Id, text: &str) -> usize {
    let chars = egui::text_edit::TextEditState::load(ctx, field)
        .and_then(|s| s.cursor.char_range())
        .map_or_else(|| text.chars().count(), |r| r.primary.index.into());
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// What the box shows under it: the problem, in red.
fn problem(edit: &Edit) -> Option<String> {
    match parse(&edit.text) {
        Err(e) if !e.incomplete || edit.tried => Some(e.message),
        _ => None,
    }
}

fn editor(app: &mut LightcraftApp, ui: &mut egui::Ui, t: &Tokens, id: &'static str, rect: Rect) {
    let ctx = ui.ctx().clone();
    let Some(mut edit) = app.keybinds.when_edit.take() else { return };
    let field = text_field::id(FIELD);
    if edit.fresh {
        edit.fresh = false;
        ui.memory_mut(|m| m.request_focus(field));
        set_cursor(&ctx, field, &edit.text, edit.text.len());
    }
    let focused = ui.memory(|m| m.has_focus(field));
    let sug = suggest::suggest(&edit.text, cursor_byte(&ctx, field, &edit.text));
    let count = sug.items.len().min(MAX_ITEMS);
    edit.selected = edit.selected.min(count.saturating_sub(1));
    let mut accept: Option<usize> = None;
    if focused && count > 0 {
        let (down, up, tab, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Tab),
                edit.navigated && i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        if down {
            edit.selected = (edit.selected + 1) % count;
            edit.navigated = true;
        }
        if up {
            edit.selected = (edit.selected + count - 1) % count;
            edit.navigated = true;
        }
        if tab || enter {
            accept = Some(edit.selected);
        }
    }
    if let Some(i) = accept.take() {
        take_suggestion(&ctx, field, &mut edit, &sug, i);
    }
    let shown = problem(&edit);
    let before = edit.text.clone();
    let mut text = std::mem::take(&mut edit.text);
    let mut field_ui = TextField::singleline(FIELD, &mut text)
        .font(FontId::monospace(11.5))
        .label(crate::i18n::tr("Condition"))
        .hint(crate::i18n::tr("Always"))
        .width(rect.width() - 4.0);
    if shown.is_some() {
        field_ui = field_ui.text_color(t.reject);
    }
    let out = field_ui.show(ui);
    edit.text = text;
    if edit.text != before {
        edit.selected = 0;
        edit.navigated = false;
        edit.tried = false;
    }
    let field_rect = out.response.rect;
    // Tab and the arrows belong to the list while it is open (egui would move the focus with them
    // at the start of the next pass); the text box set its own filter just now, so this goes after it
    if out.editing && count > 0 {
        let filter = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false };
        ctx.memory_mut(|m| m.set_focus_lock_filter(field, filter));
    }

    // the list under the box
    let mut clicked: Option<usize> = None;
    let problem_now = problem(&edit);
    let sug = suggest::suggest(&edit.text, cursor_byte(&ctx, field, &edit.text));
    let count = sug.items.len().min(MAX_ITEMS);
    let unsatisfiable = parse(&edit.text).ok().flatten().is_some_and(|e| !expr::satisfiable(&e));
    edit.list = None;
    if out.editing && (count > 0 || problem_now.is_some() || unsatisfiable) {
        let area = egui::Area::new(Id::unique("when-list").with(id)).order(Order::Tooltip).fixed_pos(field_rect.left_bottom() + vec2(0.0, 3.0));
        let shown = area.show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                ui.set_min_width(field_rect.width().max(400.0));
                ui.spacing_mut().item_spacing.y = 0.0;
                if let Some(message) = &problem_now {
                    ui.label(RichText::new(message).size(11.5).color(t.reject));
                    ui.add_space(3.0);
                } else if unsatisfiable {
                    ui.label(RichText::new(crate::i18n::tr("This condition can never be true.")).size(11.5).color(t.caution));
                    ui.add_space(3.0);
                }
                let list_h = (count as f32).min(LIST_ROWS) * LIST_ROW_H;
                egui::ScrollArea::vertical()
                    .max_height(list_h)
                    .min_scrolled_height(list_h)
                    .auto_shrink([false, true])
                    .id_salt("when-list-scroll")
                    .show(ui, |ui| {
                        for (i, item) in sug.items.iter().take(count).enumerate() {
                            let (row, r) = ui.allocate_exact_size(vec2(ui.available_width(), LIST_ROW_H), Sense::click());
                            register(ui, format!("menu:whenSuggestion-{i}"), row);
                            if i == edit.selected {
                                ui.painter().rect_filled(row, 4.0, t.accent.gamma_multiply(0.30));
                            } else if r.hovered() {
                                ui.painter().rect_filled(row, 4.0, t.hover);
                            }
                            let tint = match item.kind {
                                ItemKind::Key | ItemKind::Value => t.text,
                                ItemKind::Operator | ItemKind::Group => t.accent,
                            };
                            ui.painter().text(
                                pos2(row.left() + 8.0, row.center().y),
                                Align2::LEFT_CENTER,
                                &item.label,
                                FontId::monospace(11.5),
                                tint,
                            );
                            let detail = ui.painter().with_clip_rect(row);
                            detail.text(pos2(row.left() + 140.0, row.center().y), Align2::LEFT_CENTER, &item.detail, t.font(11.0), t.text_dim);
                            if i == edit.selected && edit.navigated {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
                if sug.items.len() > count {
                    ui.label(RichText::new(crate::i18n::tr_format!("{} more…", sug.items.len() - count)).size(10.5).color(t.text_dim));
                }
            });
        });
        edit.list = Some(shown.response.rect);
        register(ui, "panel:whenList", shown.response.rect);
    }
    if let Some(i) = clicked {
        take_suggestion(&ctx, field, &mut edit, &sug, i);
        ctx.memory_mut(|m| m.request_focus(field));
    }

    // how the edit ended
    let in_list = |edit: &Edit| edit.list.zip(ctx.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos()))).is_some_and(|(r, p)| r.contains(p));
    match out.ending {
        Some(Ending::Escape) => {
            // Esc ends the edit; the window stays
            crate::widgets::take_escape(&ctx);
            return;
        }
        Some(Ending::Return | Ending::Away) if in_list(&edit) || clicked.is_some() => {
            ctx.memory_mut(|m| m.request_focus(field));
        }
        Some(ending @ (Ending::Return | Ending::Away)) => {
            let original = conditions::text(&app.ui.settings.keymap, id).unwrap_or_default();
            match parse(&edit.text) {
                Ok(parsed) => {
                    let canonical = parsed.map(|e| e.text()).unwrap_or_default();
                    if canonical != original {
                        save(app, &ctx, id, &canonical);
                    }
                    return;
                }
                Err(_) => {
                    edit.tried = true;
                    if ending == Ending::Return {
                        ctx.memory_mut(|m| m.request_focus(field));
                    }
                }
            }
        }
        None => {}
    }
    app.keybinds.when_edit = Some(edit);
}

fn take_suggestion(ctx: &egui::Context, field: Id, edit: &mut Edit, sug: &suggest::Suggestions, i: usize) {
    let Some(item) = sug.items.get(i) else { return };
    let (text, cursor) = suggest::apply(&edit.text, sug, item);
    edit.text = text;
    edit.selected = 0;
    edit.navigated = false;
    edit.tried = false;
    set_cursor(ctx, field, &edit.text, cursor);
    ctx.request_repaint();
}

// ------------------------------------------------------------------ damaged conditions

/// A line under the list when `ui.json` holds conditions that could not be read: they are ignored.
pub fn damaged_note(app: &LightcraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let damaged = conditions::damaged(&app.ui.settings.keymap);
    if let Some((id, text, why)) = damaged.first() {
        let name = crate::shortcuts::find_bindable(id).map_or(id.as_str(), |b| crate::i18n::tr(b.label));
        ui.add_space(4.0);
        ui.label(
            RichText::new(crate::i18n::tr_format!("The condition of {} could not be read and is ignored (“{}”): {}", name, text, why))
                .size(11.5)
                .color(t.reject),
        );
    }
}
