//! The Edit panel's sections (Light, Color, Effects, Detail, Optics, Calibration) in the order and
//! with the visibility the user chose: moved with Move Up / Move Down in a section header's
//! right-click menu, hidden with Hide This Section, brought back from the same menu, from
//! Window ▸ Edit Sections (⌘1 … ⌘5, which also show a hidden section) or by Reset.
//!
//! The arrangement ([`EditSectionLayout`]) is saved with the UI state (`ui.json`, key
//! `editSections`); a state saved before it existed loads as the usual order with every section
//! shown. Hiding only takes a section off the panel: its adjustments stay in the photo's settings
//! and keep applying to every render and export. At least one section always stays shown, so the
//! menu that brings the others back always has a header to open it on.
//!
//! Agents use `view.editSections {order?, hidden?}` (no params reads it), `view.editSectionShow
//! {section, show?}`, `view.editSectionMove {section, direction | before}` and
//! `view.editSectionsReset`. Only the six top-level sections are arranged here; Curve, Color
//! Mixer, Point Color and Color Grading stay inside their sections.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::widgets::register;

#[cfg(test)]
mod tests;

/// A top-level section of the Edit panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditSection {
    Light,
    Color,
    Effects,
    Detail,
    Optics,
    Calibration,
}

impl EditSection {
    /// Every section, in the order the panel starts with.
    pub const ALL: [EditSection; 6] =
        [EditSection::Light, EditSection::Color, EditSection::Effects, EditSection::Detail, EditSection::Optics, EditSection::Calibration];

    /// The name commands, the saved state and the `section.<id>` commands use.
    pub const fn id(self) -> &'static str {
        match self {
            EditSection::Light => "light",
            EditSection::Color => "color",
            EditSection::Effects => "effects",
            EditSection::Detail => "detail",
            EditSection::Optics => "optics",
            EditSection::Calibration => "calibration",
        }
    }

    /// The heading, in English (translated where it is shown).
    pub const fn title(self) -> &'static str {
        match self {
            EditSection::Light => "Light",
            EditSection::Color => "Color",
            EditSection::Effects => "Effects",
            EditSection::Detail => "Detail",
            EditSection::Optics => "Optics",
            EditSection::Calibration => "Calibration",
        }
    }

    pub fn from_id(id: &str) -> Option<EditSection> {
        Self::ALL.into_iter().find(|s| s.id() == id)
    }

    /// The ids of every section, for error messages (`light|color|…`).
    pub fn ids() -> String {
        Self::ALL.map(EditSection::id).join("|")
    }
}

/// Where one section stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct EditSectionState {
    pub id: EditSection,
    pub hidden: bool,
}

impl EditSectionState {
    const fn new(id: EditSection) -> Self {
        Self { id, hidden: false }
    }
}

/// The sections top to bottom. Every section is in it exactly once, whatever was read from disk or
/// sent by an agent: unknown and repeated entries are dropped and sections not named come last in
/// their usual order. At least one is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Value", into = "Vec<EditSectionState>")]
pub struct EditSectionLayout(Vec<EditSectionState>);

impl Default for EditSectionLayout {
    fn default() -> Self {
        Self(EditSection::ALL.map(EditSectionState::new).to_vec())
    }
}

impl From<EditSectionLayout> for Vec<EditSectionState> {
    fn from(layout: EditSectionLayout) -> Self {
        layout.0
    }
}

/// Lenient on purpose, like the sidebar's: a state file edited by hand, written by a newer version
/// or damaged still opens; what can't be understood is left out.
impl From<Value> for EditSectionLayout {
    fn from(saved: Value) -> Self {
        let entries = saved.as_array().map(Vec::as_slice).unwrap_or_default();
        let read = entries.iter().filter_map(|entry| {
            let id = entry.as_str().or_else(|| entry.get("id").and_then(Value::as_str))?;
            let hidden = entry.get("hidden").and_then(Value::as_bool).unwrap_or(false);
            Some(EditSectionState { id: EditSection::from_id(id)?, hidden })
        });
        Self::from_states(read)
    }
}

impl EditSectionLayout {
    /// The layout these states describe, made whole (see the type).
    fn from_states(states: impl IntoIterator<Item = EditSectionState>) -> Self {
        let mut all: Vec<EditSectionState> = Vec::with_capacity(EditSection::ALL.len());
        for state in states.into_iter().chain(EditSection::ALL.map(EditSectionState::new)) {
            if !all.iter().any(|s| s.id == state.id) {
                all.push(state);
            }
        }
        let mut layout = Self(all);
        layout.keep_one_shown();
        layout
    }

    /// Never hide everything: a layout with every section hidden shows the first.
    fn keep_one_shown(&mut self) {
        if self.0.iter().all(|s| s.hidden)
            && let Some(first) = self.0.first_mut()
        {
            first.hidden = false;
        }
    }

    /// Every section top to bottom, hidden ones included.
    pub fn sections(&self) -> &[EditSectionState] {
        &self.0
    }

    /// The sections that are shown, top to bottom.
    pub fn shown(&self) -> Vec<EditSection> {
        self.0.iter().filter(|s| !s.hidden).map(|s| s.id).collect()
    }

    pub fn is_hidden(&self, section: EditSection) -> bool {
        self.0.iter().any(|s| s.id == section && s.hidden)
    }

    /// Show or hide a section; it keeps its place. Hiding the last shown section is refused (false).
    pub fn set_hidden(&mut self, section: EditSection, hidden: bool) -> bool {
        if hidden && !self.is_hidden(section) && self.shown().len() <= 1 {
            return false;
        }
        match self.0.iter_mut().find(|s| s.id == section) {
            Some(s) => s.hidden = hidden,
            None => return false,
        }
        true
    }

    /// Make sure a section can be seen (something in it is being asked for).
    pub fn reveal(&mut self, section: EditSection) {
        self.set_hidden(section, false);
    }

    /// Put `section` just above `before`, or last when `before` is `None`. Returns whether the
    /// order changed.
    pub fn move_before(&mut self, section: EditSection, before: Option<EditSection>) -> bool {
        if before == Some(section) {
            return false;
        }
        let was = self.clone();
        let Some(moved) = self.0.iter().copied().find(|s| s.id == section) else { return false };
        self.0.retain(|s| s.id != section);
        let at = before.and_then(|b| self.0.iter().position(|s| s.id == b)).unwrap_or(self.0.len());
        self.0.insert(at, moved);
        // past hidden sections only: the panel would look the same, so the order stays
        if !moved.hidden && self.shown() == was.shown() {
            *self = was;
            return false;
        }
        *self != was
    }

    /// Move `section` one place up or down among the shown sections (hidden ones are stepped
    /// over, so every move can be seen). Returns whether it moved.
    pub fn move_by(&mut self, section: EditSection, up: bool) -> bool {
        let shown = self.shown();
        let Some(at) = shown.iter().position(|s| *s == section) else { return false };
        if up {
            let Some(above) = at.checked_sub(1).and_then(|i| shown.get(i)) else { return false };
            self.move_before(section, Some(*above))
        } else {
            if at + 1 >= shown.len() {
                return false;
            }
            self.move_before(section, shown.get(at + 2).copied())
        }
    }

    /// Whether `section` can move up (or down) among the shown sections.
    pub fn can_move(&self, section: EditSection, up: bool) -> bool {
        let shown = self.shown();
        match shown.iter().position(|s| *s == section) {
            Some(at) if up => at > 0,
            Some(at) => at + 1 < shown.len(),
            None => false,
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn any_hidden(&self) -> bool {
        self.0.iter().any(|s| s.hidden)
    }

    /// Every section shown again, in the order they are in.
    pub fn show_all(&mut self) {
        for s in &mut self.0 {
            s.hidden = false;
        }
    }

    /// The arrangement an agent asks for (`view.editSections`), read strictly: `order` is a list
    /// of section ids, each at most once (those left out follow in the usual order); `hidden` is
    /// a list of ids. A key left out keeps what is now. What is wrong is said.
    pub fn with_request(&self, order: Option<&Value>, hidden: Option<&Value>) -> Result<Self, String> {
        let ids = |key: &str, v: &Value| -> Result<Vec<EditSection>, String> {
            let list =
                v.as_array().ok_or_else(|| format!("view.editSections: `{key}` must be a list of sections ({}), not {v}", EditSection::ids()))?;
            let mut out: Vec<EditSection> = Vec::new();
            for entry in list {
                let name = entry
                    .as_str()
                    .ok_or_else(|| format!("view.editSections: `{key}` has {entry}, which names no section ({})", EditSection::ids()))?;
                let id = EditSection::from_id(name)
                    .ok_or_else(|| format!("view.editSections: unknown section `{name}` in `{key}` ({})", EditSection::ids()))?;
                if out.contains(&id) {
                    return Err(format!("view.editSections: `{name}` is listed twice in `{key}`"));
                }
                out.push(id);
            }
            Ok(out)
        };
        let mut next = self.clone();
        if let Some(v) = order.filter(|v| !v.is_null()) {
            let wanted = ids("order", v)?;
            let states = wanted.iter().filter_map(|id| self.0.iter().copied().find(|s| s.id == *id)).chain(self.0.iter().copied());
            next = Self::from_states(states);
        }
        if let Some(v) = hidden.filter(|v| !v.is_null()) {
            let hide = ids("hidden", v)?;
            if hide.len() == EditSection::ALL.len() {
                return Err("view.editSections: at least one section must stay shown".into());
            }
            for s in &mut next.0 {
                s.hidden = hide.contains(&s.id);
            }
        }
        Ok(next)
    }

    /// `{order, hidden}` for command results.
    pub fn to_json(&self) -> Value {
        let order: Vec<&str> = self.0.iter().map(|s| s.id.id()).collect();
        let hidden: Vec<&str> = self.0.iter().filter(|s| s.hidden).map(|s| s.id.id()).collect();
        json!({"order": order, "hidden": hidden, "default": self.is_default()})
    }
}

/// The UI commands of this feature; `None` when `id` is not one of them.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let section = |p: &Value| -> Result<EditSection, String> {
        let name = p.get("section").and_then(Value::as_str).ok_or_else(|| format!("{id}: `section` is needed ({})", EditSection::ids()))?;
        EditSection::from_id(name).ok_or_else(|| format!("{id}: unknown section `{name}` ({})", EditSection::ids()))
    };
    let layout = &mut app.ui.edit_sections;
    let r = match id {
        "view.editSections" => layout.with_request(p.get("order"), p.get("hidden")).map(|next| {
            *layout = next;
            layout.to_json()
        }),
        "view.editSectionShow" => section(p).and_then(|s| {
            let show = match p.get("show") {
                None | Some(Value::Null) => layout.is_hidden(s),
                Some(Value::Bool(b)) => *b,
                Some(other) => return Err(format!("{id}: `show` must be true or false, not {other}")),
            };
            if !layout.set_hidden(s, !show) {
                return Err(format!("{id}: at least one section must stay shown"));
            }
            Ok(layout.to_json())
        }),
        "view.editSectionMove" => section(p).and_then(|s| {
            let moved = match (p.get("direction"), p.get("before")) {
                (Some(d), None) => match d.as_str() {
                    Some("up") => layout.move_by(s, true),
                    Some("down") => layout.move_by(s, false),
                    _ => return Err(format!("{id}: `direction` must be \"up\" or \"down\", not {d}")),
                },
                (None, Some(Value::Null)) => layout.move_before(s, None),
                (None, Some(b)) => match b.as_str().and_then(EditSection::from_id) {
                    Some(before) => layout.move_before(s, Some(before)),
                    None => return Err(format!("{id}: `before` must be a section ({}) or null, not {b}", EditSection::ids())),
                },
                _ => return Err(format!("{id}: give either `direction` (up|down) or `before` (a section, or null for last)")),
            };
            let mut out = layout.to_json();
            out["moved"] = json!(moved);
            Ok(out)
        }),
        "view.editSectionsReset" => {
            *layout = EditSectionLayout::default();
            Ok(layout.to_json())
        }
        _ => return None,
    };
    Some(r)
}

/// The rest of a section header's right-click menu (after Solo Mode): move this section, hide it,
/// show or hide any, show all, reset. Widgets `editSectionMenu:moveUp`, `:moveDown`, `:hide`,
/// `:section:<id>`, `:showAll` and `:reset`.
pub fn menu(app: &mut LightcraftApp, ui: &mut egui::Ui, on: &str) {
    let mut run = None;
    if let Some(section) = EditSection::from_id(on) {
        ui.separator();
        for (up, label, widget) in [(true, "Move Up", "editSectionMenu:moveUp"), (false, "Move Down", "editSectionMenu:moveDown")] {
            let item = ui.add_enabled(app.ui.edit_sections.can_move(section, up), egui::Button::new(crate::i18n::tr(label)));
            register(ui, widget, item.rect);
            if item.clicked() {
                run = Some(("view.editSectionMove", json!({"section": section.id(), "direction": if up { "up" } else { "down" }})));
            }
        }
        let can_hide = app.ui.edit_sections.shown().len() > 1;
        let hide = ui.add_enabled(can_hide, egui::Button::new(crate::i18n::tr("Hide This Section")));
        register(ui, "editSectionMenu:hide", hide.rect);
        if hide.clicked() {
            run = Some(("view.editSectionShow", json!({"section": section.id(), "show": false})));
        }
    }
    ui.separator();
    for s in app.ui.edit_sections.sections() {
        let item = ui.selectable_label(!s.hidden, crate::i18n::tr(s.id.title()));
        register(ui, format!("editSectionMenu:section:{}", s.id.id()), item.rect);
        if item.clicked() {
            run = Some(("view.editSectionShow", json!({"section": s.id.id(), "show": s.hidden})));
        }
    }
    ui.separator();
    let show_all = ui.add_enabled(app.ui.edit_sections.any_hidden(), egui::Button::new(crate::i18n::tr("Show All Sections")));
    register(ui, "editSectionMenu:showAll", show_all.rect);
    if show_all.clicked() {
        let order = app.ui.edit_sections.to_json()["order"].clone();
        run = Some(("view.editSections", json!({"order": order, "hidden": []})));
    }
    let reset = ui.add_enabled(!app.ui.edit_sections.is_default(), egui::Button::new(crate::i18n::tr("Reset to Default")));
    register(ui, "editSectionMenu:reset", reset.rect);
    if reset.clicked() {
        run = Some(("view.editSectionsReset", json!({})));
    }
    if let Some((id, params)) = run {
        if let Err(e) = app.run(id, params) {
            app.toast(ui.ctx(), e);
        }
        ui.close();
    }
}
