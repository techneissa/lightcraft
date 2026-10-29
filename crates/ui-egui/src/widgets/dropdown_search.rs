//! Type-to-search in dropdown lists. With a list open, typing narrows it to the options that match (a
//! slim field with a magnifier shows the text above the rows), ↑/↓ move a highlight over the
//! matches, ↩ or a click picks one, and Esc clears the text before it closes the list. Nothing shows
//! until a letter is typed, so a list looks as it always did.
//!
//! The matching and the key handling are plain code on strings ([`matches`], [`Search`]); [`before`]
//! feeds them egui's events, and the [`Filter`] it returns draws the field and the rows.
//! [`Filter::list`] draws a plain list of options in one call; [`Filter::options`], [`Filter::row`]
//! and [`Filter::no_match`] are its parts, for lists with extra rows of their own.
//!
//! While a list is open the keyboard is its: [`list_open`] tells the shortcuts and the held keys
//! (Backslash shows the original) to stay out, whether or not anything is typed yet.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use egui::{Event, Id, Key, Ui};

use crate::icons::{Icon, paint};
use crate::theme::Tokens;

/// The options `query` matches, best first: those whose label starts with it, then those with a word
/// starting with it ("li" finds Lighten and Linear Burn before Soft Light), then those that merely
/// contain it; each group in list order. Case is ignored. `enabled[i] == false` keeps option `i` out
/// (an option without an entry is enabled). A blank query matches every enabled option.
pub(super) fn matches(query: &str, labels: &[impl AsRef<str>], enabled: &[bool]) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut tiers: [Vec<usize>; 3] = Default::default();
    for (i, label) in labels.iter().enumerate() {
        // An empty label is a separator or a gap, not something to pick.
        if label.as_ref().is_empty() || !enabled.get(i).copied().unwrap_or(true) {
            continue;
        }
        let tier = tier(&label.as_ref().to_lowercase(), &query);
        if let Some(list) = tier.and_then(|t| tiers.get_mut(t)) {
            list.push(i);
        }
    }
    tiers.concat()
}

/// 0 for a label (lower case) starting with `query`, 1 for one with a word starting with it, 2 for
/// one containing it, none for no match. `match_indices` gives character boundaries, so the slice
/// before a match never splits a character.
fn tier(label: &str, query: &str) -> Option<usize> {
    if label.starts_with(query) {
        return Some(0);
    }
    let mut found = None;
    for (at, _) in label.match_indices(query) {
        let word_start = label.get(..at).and_then(|before| before.chars().next_back()).is_none_or(|c| !c.is_alphanumeric());
        if word_start {
            return Some(1);
        }
        found = Some(2);
    }
    found
}

/// One thing the user did in an open list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Input<'a> {
    Text(&'a str),
    Backspace,
    /// ⌥ or ⌘ with Backspace: all the text.
    Clear,
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Esc,
}

/// How far PageUp and PageDown move the highlight.
const PAGE: usize = 5;

/// What the open list of a dropdown has been typed so far, and the match it highlights.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Search {
    pub query: String,
    /// The highlighted match, as a position in [`matches`].
    pub active: usize,
    /// An IME is composing text (its preedit is not empty): the field shows to take it.
    pub composing: bool,
}

/// What [`Search::apply`] did with an input.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Applied {
    /// The input belongs to the search: the list and the keyboard shortcuts don't get it.
    pub consumed: bool,
    /// ↩ picked this option (an index into the options).
    pub pick: Option<usize>,
}

impl Search {
    pub(super) fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// Handles `input` for a list of `labels`. Keys that mean something with no text typed (↩, Esc,
    /// the arrows) are left to the list while there is none.
    pub(super) fn apply(&mut self, input: Input<'_>, labels: &[&str], enabled: &[bool]) -> Applied {
        let mut applied = Applied::default();
        let count = |query: &str| matches(query, labels, enabled).len();
        match input {
            Input::Text(text) => {
                let before = self.query.len();
                for c in text.chars().filter(|c| !c.is_control()) {
                    // A space only counts inside the text ("linear b"), never to start it.
                    if !(c.is_whitespace() && self.query.is_empty()) {
                        self.query.push(c);
                    }
                }
                if self.query.len() != before {
                    self.active = 0;
                }
                self.composing = false;
                applied.consumed = true;
            }
            Input::Backspace | Input::Clear if self.is_active() => {
                if input == Input::Clear {
                    self.query.clear();
                } else {
                    self.query.pop();
                }
                self.active = 0;
                applied.consumed = true;
            }
            Input::Esc if self.is_active() => {
                self.query.clear();
                self.active = 0;
                applied.consumed = true;
            }
            Input::Up | Input::Down | Input::PageUp | Input::PageDown if self.is_active() => {
                let last = count(&self.query).saturating_sub(1);
                self.active = match input {
                    Input::Up => self.active.saturating_sub(1),
                    Input::PageUp => self.active.saturating_sub(PAGE),
                    Input::Down => self.active.saturating_add(1),
                    _ => self.active.saturating_add(PAGE),
                }
                .min(last);
                applied.consumed = true;
            }
            Input::Enter if self.is_active() => {
                applied.pick = matches(&self.query, labels, enabled).get(self.active).copied();
                applied.consumed = true;
            }
            _ => {}
        }
        applied
    }
}

/// Where the number of the last pass an open dropdown list was drawn in is kept.
fn open_id() -> Id {
    Id::unique("lc-dropdown-list-open")
}

/// Says a dropdown list is drawn in this pass ([`list_open`]).
pub fn mark_open(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(open_id(), pass));
}

/// Is a dropdown list open? It then owns the keyboard, like an open menu: a typed letter is for the
/// list's search, not a shortcut, and a held key does nothing on the canvas.
pub fn list_open(ctx: &egui::Context) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<u64>(open_id())).is_some_and(|seen| seen.saturating_add(2) >= pass)
}

/// What a search has typed and found so far.
#[derive(Default)]
struct Found {
    query: String,
    /// The options shown, best match first.
    shown: Vec<usize>,
    /// The highlighted row, a position in `shown`.
    active: usize,
    /// The text or the highlight changed this frame: the highlighted row scrolls into view.
    moved: bool,
}

/// The search of one open list, made by [`before`] for the field and the rows.
#[derive(Default)]
pub struct Filter {
    /// The id of the dropdown's search; none for a list that is not open.
    id: Option<Id>,
    labels: Vec<String>,
    enabled: Vec<bool>,
    /// An IME is composing: the field shows even with no text yet.
    composing: bool,
    found: RefCell<Found>,
    /// The rows drawn so far this frame, to know which one is the highlighted.
    row: Cell<usize>,
}

/// The widget name of the search field (one list is open at a time).
const FIELD: &str = "dropdownSearch";

impl Filter {
    /// Is a search typed?
    pub fn filtering(&self) -> bool {
        !self.found.borrow().query.is_empty()
    }

    fn salt(&self, name: &str) -> Option<Id> {
        self.id.map(|id| id.with(name))
    }

    /// The option ↩ picked, once: the list takes it when it draws, in this pass or a later one.
    pub fn picked(&self, ui: &Ui) -> Option<usize> {
        let pick = self.salt("pick")?;
        ui.data_mut(|d| d.remove_temp::<usize>(pick))
    }

    /// The top of the open list: the typed text in a slim field with a magnifier, nothing while none
    /// is typed. The field is a text field, so it takes the keyboard (IME, selection, paste) while it
    /// shows.
    pub fn header(&self, ui: &mut Ui) {
        let Some(id) = self.id else { return };
        // Marks the list open for the shortcuts and the held keys.
        mark_open(ui.ctx());
        if !self.field_shown() {
            return;
        }
        let width = ui.data(|d| d.get_temp::<f32>(id.with("width"))).unwrap_or(FIELD_WIDTH).max(FIELD_MIN);
        let mut query = self.found.borrow().query.clone();
        let t = Tokens::get(ui.ctx());
        ui.horizontal(|ui| {
            let (icon, _) = ui.allocate_exact_size(egui::vec2(16.0, ui.spacing().interact_size.y), egui::Sense::hover());
            paint(ui.painter(), egui::Rect::from_center_size(icon.center(), egui::vec2(14.0, 14.0)), Icon::Search, t.text_dim);
            ui.memory_mut(|m| m.request_focus(crate::text_field::id(FIELD)));
            crate::text_field::TextField::singleline(FIELD, &mut query).label(crate::i18n::tr("Search")).width(width - 20.0).show(ui);
        });
        ui.add_space(2.0);
        if query != self.found.borrow().query {
            let shown = matches(&query, &self.labels, &self.enabled);
            ui.data_mut(|d| d.insert_temp(id, Search { query: query.clone(), active: 0, composing: false }));
            *self.found.borrow_mut() = Found { query, shown, active: 0, moved: true };
        }
    }

    /// The options to draw as rows: all of `items` (one per option) when nothing is typed, else the
    /// matches among them, best first.
    pub fn options<'a, X: Clone>(&self, items: &'a [X]) -> Cow<'a, [X]> {
        self.row.set(0);
        if !self.filtering() {
            return Cow::Borrowed(items);
        }
        Cow::Owned(self.found.borrow().shown.iter().filter_map(|&i| items.get(i).cloned()).collect())
    }

    /// Called as each row `item` is drawn, in order: the highlighted one is lit, and scrolled to when it moved.
    pub fn row(&self, ui: &Ui, item: &egui::Response) {
        let at = self.row.get();
        self.row.set(at.saturating_add(1));
        self.highlight(ui, item, at);
    }

    /// [`Self::row`] for a list that draws only the rows in view: `item` is the row at position `at`
    /// among the matches.
    pub fn highlight(&self, ui: &Ui, item: &egui::Response, at: usize) {
        if let Some(id) = self.salt("width") {
            ui.data_mut(|d| d.insert_temp(id, item.rect.width()));
        }
        let found = self.found.borrow();
        if found.query.is_empty() || found.active != at {
            return;
        }
        // Translucent: the row is already drawn, its text shows through.
        ui.painter().rect_filled(item.rect, 3.0, Tokens::get(ui.ctx()).accent.gamma_multiply(0.35));
        if found.moved {
            item.scroll_to_me(None);
        }
    }

    /// Does the search field show? A click in it must not close the list.
    pub fn field_shown(&self) -> bool {
        self.filtering() || self.composing
    }

    /// When the list closes on a click: anywhere in it until the search field shows (a click on the
    /// field must keep it open), then outside it only; the rows close it themselves.
    pub fn close_behavior(&self) -> egui::PopupCloseBehavior {
        if self.field_shown() { egui::PopupCloseBehavior::CloseOnClickOutside } else { egui::PopupCloseBehavior::CloseOnClick }
    }

    /// The options that match the typed text, best first (empty while nothing is typed).
    pub fn matches(&self) -> Vec<usize> {
        self.found.borrow().shown.clone()
    }

    /// The text or the highlight changed this frame.
    pub fn moved(&self) -> bool {
        self.found.borrow().moved
    }

    /// The highlighted position among [`Self::matches`], while text is typed.
    pub fn active_pos(&self) -> Option<usize> {
        let found = self.found.borrow();
        (!found.query.is_empty()).then_some(found.active)
    }

    /// The dim line under the field when nothing matches.
    pub fn no_match(&self, ui: &mut Ui) {
        let found = self.found.borrow();
        if !found.query.is_empty() && found.shown.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No match")).color(Tokens::get(ui.ctx()).text_dim));
        }
    }

    /// A plain list of `items` as selectable rows, narrowed by the typed text: draws the rows, the
    /// highlight and "No match", and returns the item chosen by a click or by ↩ (an index into
    /// `items`, which are the options `labels` of [`before`] named, in the same order). ↩ closes the
    /// list; a click closes it as the combo box always did.
    pub fn list<X>(&self, ui: &mut Ui, items: &[X], selected: impl Fn(&X) -> bool, label: impl Fn(&X) -> String) -> Option<usize> {
        self.list_with(ui, items, selected, label, |_, _, _| {})
    }

    /// [`Self::list`], calling `on_row` with each item and its row as it is drawn (to register the
    /// row for the control channel).
    pub fn list_with<X>(
        &self,
        ui: &mut Ui,
        items: &[X],
        selected: impl Fn(&X) -> bool,
        label: impl Fn(&X) -> String,
        mut on_row: impl FnMut(&Ui, &X, &egui::Response),
    ) -> Option<usize> {
        let mut chosen = self.picked(ui);
        if chosen.is_some() {
            ui.close();
        }
        self.row.set(0);
        let order: Vec<usize> = if self.filtering() { self.found.borrow().shown.clone() } else { (0..items.len()).collect() };
        for i in order {
            let Some(item) = items.get(i) else { continue };
            let row = ui.selectable_label(selected(item), label(item));
            self.row(ui, &row);
            on_row(ui, item, &row);
            if row.clicked() {
                chosen = Some(i);
                ui.close();
            }
        }
        self.no_match(ui);
        chosen
    }
}

/// The width of the field before a row has told how wide the list is, and the narrowest it gets.
const FIELD_WIDTH: f32 = 140.0;
const FIELD_MIN: f32 = 90.0;

/// A combo box with a search: [`Self::search`] is `show_ui` with the typed text in a field above the
/// rows.
pub trait Searchable {
    fn search<R>(self, ui: &mut Ui, filter: &Filter, rows: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>>;
}

impl Searchable for craft_ui::ComboBox<'_> {
    fn search<R>(self, ui: &mut Ui, filter: &Filter, rows: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
        self.close_behavior(filter.close_behavior()).show_ui_with_header(ui, |ui| filter.header(ui), |ui, ()| rows(ui))
    }
}

/// The usual dropdown over `items`, with a search: a combo box (id salt `salt`, set up by `set_up`:
/// width, selected text) whose open list holds a selectable row per item. Returns the combo box's
/// response and the item a click or ↩ chose (an index into `items`). `selected` marks the current
/// item, `label` names an item as displayed.
pub fn pick<X>(
    ui: &mut Ui,
    salt: &str,
    set_up: impl FnOnce(craft_ui::ComboBox<'static>) -> craft_ui::ComboBox<'static>,
    items: &[X],
    selected: impl Fn(&X) -> bool,
    label: impl Fn(&X) -> String,
) -> (egui::Response, Option<usize>) {
    pick_with(ui, salt, set_up, items, selected, label, |_, _, _| {})
}

/// [`pick`], calling `on_row` with each item and its row as it is drawn.
pub fn pick_with<X>(
    ui: &mut Ui,
    salt: &str,
    set_up: impl FnOnce(craft_ui::ComboBox<'static>) -> craft_ui::ComboBox<'static>,
    items: &[X],
    selected: impl Fn(&X) -> bool,
    label: impl Fn(&X) -> String,
    on_row: impl FnMut(&Ui, &X, &egui::Response),
) -> (egui::Response, Option<usize>) {
    let find = before_lazy(ui, salt, || items.iter().map(&label).collect(), &[]);
    let shown = set_up(crate::widgets::combo_box(salt)).search(ui, &find, |ui| find.list_with(ui, items, selected, label, on_row));
    (shown.response, shown.inner.flatten())
}

/// The menu of a dropdown button (`egui::Popup::menu(response)`) with the search above `rows`.
pub fn menu<R>(response: &egui::Response, find: &Filter, rows: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    egui::Popup::menu(response)
        .close_behavior(find.close_behavior())
        .show(|ui| {
            find.header(ui);
            rows(ui)
        })
        .map(|shown| shown.inner)
}

/// [`pick`] for the menu of a dropdown button: a selectable row per item. Returns the item a click or
/// ↩ chose (an index into `items`).
pub fn menu_pick<X>(
    ui: &mut Ui,
    response: &egui::Response,
    items: &[X],
    selected: impl Fn(&X) -> bool,
    label: impl Fn(&X) -> String,
) -> Option<usize> {
    menu_pick_with(ui, response, items, selected, label, |_, _, _| {})
}

/// [`menu_pick`], calling `on_row` with each item and its row as it is drawn.
pub fn menu_pick_with<X>(
    ui: &mut Ui,
    response: &egui::Response,
    items: &[X],
    selected: impl Fn(&X) -> bool,
    label: impl Fn(&X) -> String,
    on_row: impl FnMut(&Ui, &X, &egui::Response),
) -> Option<usize> {
    let find = before_menu(ui, response, || items.iter().map(&label).collect());
    menu(response, &find, |ui| find.list_with(ui, items, selected, label, on_row)).flatten()
}

/// Searches the open list of dropdown `salt` (the id salt of its combo box): takes the typing and
/// the keys that belong to the search. Call it before the combo box is shown, because a focused combo
/// box button takes ↩ to toggle its list before the list gets to see the key. `labels` are the
/// options as displayed.
pub fn before(ui: &mut Ui, salt: &str, labels: &[impl AsRef<str>]) -> Filter {
    before_enabled(ui, salt, labels, &[])
}

/// [`before`], where `enabled[i] == false` keeps option `i` out of the matches (an option past the end
/// of `enabled` is enabled).
pub fn before_enabled(ui: &mut Ui, salt: &str, labels: &[impl AsRef<str>], enabled: &[bool]) -> Filter {
    before_lazy(ui, salt, || labels.iter().map(|l| l.as_ref().to_owned()).collect(), enabled)
}

/// [`before_enabled`] with the labels made only while the list is open, for a long list or one that is
/// costly to name.
pub fn before_lazy(ui: &mut Ui, salt: &str, labels: impl FnOnce() -> Vec<String>, enabled: &[bool]) -> Filter {
    let id = ui.make_persistent_id(salt).with("search");
    let button = ui.make_persistent_id(egui::IdSalt::new(salt));
    let open = craft_ui::ComboBox::is_open(ui.ctx(), button);
    before_at(ui, id, open, labels, enabled)
}

/// [`before_lazy`] for the menu a dropdown button opens with `egui::Popup::menu(response)`.
pub fn before_menu(ui: &mut Ui, response: &egui::Response, labels: impl FnOnce() -> Vec<String>) -> Filter {
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(response));
    before_at(ui, response.id.with("search"), open, labels, &[])
}

fn before_at(ui: &mut Ui, id: Id, open: bool, labels: impl FnOnce() -> Vec<String>, enabled: &[bool]) -> Filter {
    // A list that is not open starts with nothing typed when it opens.
    if !open {
        ui.data_mut(|d| d.remove::<Search>(id));
        return Filter::default();
    }
    let labels = labels();
    let texts: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut search: Search = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    // another text field in the list (the custom aspect ratio) keeps what is typed into it
    let elsewhere = ui.ctx().text_edit_focused() && ui.memory(|m| m.focused()) != Some(crate::text_field::id(FIELD));
    let before = search.clone();
    let mut picked = None;
    // With text typed the field has the keyboard: it edits the text, and only the keys that move the
    // highlight, pick and clear are the search's.
    let editing = search.is_active() || search.composing;
    ui.input_mut(|i| {
        i.events.retain(|event| {
            if elsewhere {
                return true;
            }
            // An IME composing: the field shows from the next frame on, to show the preedit.
            if let Event::Ime(egui::ImeEvent::Preedit { text, .. }) = event {
                search.composing = !text.is_empty();
                return true;
            }
            let Some(input) = input_of(event) else { return true };
            if editing && matches!(input, Input::Text(_) | Input::Backspace | Input::Clear) {
                return true;
            }
            let applied = search.apply(input, &texts, enabled);
            picked = picked.or(applied.pick);
            !applied.consumed
        });
    });
    ui.data_mut(|d| d.insert_temp(id, search.clone()));
    if let Some(pick) = picked {
        ui.data_mut(|d| d.insert_temp(id.with("pick"), pick));
    }
    let mut found = Found::default();
    if search.is_active() {
        found = Found { shown: matches(&search.query, &texts, enabled), active: search.active, moved: search != before, query: search.query };
    }
    Filter { id: Some(id), enabled: enabled.to_vec(), labels, composing: search.composing, found: RefCell::new(found), row: Cell::new(0) }
}

/// The egui event as a search input, if it is one.
fn input_of(event: &Event) -> Option<Input<'_>> {
    match event {
        Event::Text(t) | Event::Paste(t) | Event::Ime(egui::ImeEvent::Commit(t)) => Some(Input::Text(t)),
        Event::Key { key, pressed: true, modifiers, .. } => {
            let plain = !(modifiers.command || modifiers.ctrl || modifiers.alt);
            match key {
                Key::Backspace if plain => Some(Input::Backspace),
                Key::Backspace => Some(Input::Clear),
                _ if !plain => None,
                Key::ArrowUp => Some(Input::Up),
                Key::ArrowDown => Some(Input::Down),
                Key::PageUp => Some(Input::PageUp),
                Key::PageDown => Some(Input::PageDown),
                Key::Enter => Some(Input::Enter),
                Key::Escape => Some(Input::Esc),
                _ => None,
            }
        }
        _ => None,
    }
}

pub mod presets;

#[cfg(test)]
mod tests;
