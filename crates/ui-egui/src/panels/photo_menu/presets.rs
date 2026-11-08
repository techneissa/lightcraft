//! Three presets for the photo menu: Settings ▸ Photo Menu keeps up to three arrangements in slots
//! (Preset 1, 2 and 3), saves the menu as it is now into one, and switches between them at once.
//!
//! A slot holds the menu's slots as saved ([`super::Slot::key`] strings). The app settings keep them
//! in `photoMenuPresets`, with the slot last saved or switched to (`active`) and the menu it replaced
//! by the last switch (`undo`), so a switch can be taken back. The marker shows `active` as edited
//! when the menu no longer equals what that slot holds.
//!
//! Commands (slot numbers count from 1, as the buttons do): `photoMenu.savePreset {slot}`,
//! `photoMenu.switchPreset {slot}`, `photoMenu.clearPreset {slot}` and `photoMenu.undoPreset`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::DEFAULT;
use crate::LightcraftApp;

/// How many presets there are.
pub const COUNT: usize = 3;

/// The menu a switch replaced, and the slot that was active then. `after` is the menu the switch
/// made: once the menu has changed since, the switch is no longer the last thing done to it and
/// there is nothing to take back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Undo {
    pub items: Vec<String>,
    pub active: Option<usize>,
    pub after: Vec<String>,
}

/// The presets, as the app settings keep them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Presets {
    /// The menu in each slot (None = empty); always [`COUNT`] long once read.
    pub slots: Vec<Option<Vec<String>>>,
    /// The slot last saved or switched to (counting from 0).
    pub active: Option<usize>,
    pub undo: Option<Undo>,
}

impl Presets {
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none) && self.active.is_none() && self.undo.is_none()
    }

    /// The menu saved in `slot` (counting from 0).
    pub fn get(&self, slot: usize) -> Option<&Vec<String>> {
        self.slots.get(slot).and_then(Option::as_ref)
    }

    fn put(&mut self, slot: usize, items: Option<Vec<String>>) {
        if self.slots.len() < COUNT {
            self.slots.resize(COUNT, None);
        }
        if let Some(s) = self.slots.get_mut(slot) {
            *s = items;
        }
    }
}

/// Read the presets leniently: a settings file with something else here (hand-edited, from another
/// version) still loads, and keeps whatever of it is text lists in the three slots.
pub fn read<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Presets, D::Error> {
    let value = <Value as serde::Deserialize>::deserialize(d)?;
    Ok(from_value(&value))
}

fn text_list(v: &Value) -> Option<Vec<String>> {
    Some(v.as_array()?.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
}

fn slot_index(v: Option<&Value>) -> Option<usize> {
    v.and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).filter(|n| *n < COUNT)
}

fn from_value(value: &Value) -> Presets {
    let mut out = Presets::default();
    let Some(obj) = value.as_object() else { return out };
    out.slots = (0..COUNT).map(|i| obj.get("slots").and_then(Value::as_array).and_then(|a| a.get(i)).and_then(text_list)).collect();
    out.active = slot_index(obj.get("active")).filter(|i| out.get(*i).is_some());
    out.undo = obj.get("undo").and_then(Value::as_object).and_then(|u| {
        Some(Undo {
            items: text_list(u.get("items")?)?,
            active: slot_index(u.get("active")).filter(|i| out.get(*i).is_some()),
            after: text_list(u.get("after")?)?,
        })
    });
    if out.slots.iter().all(Option::is_none) {
        out.slots.clear();
    }
    out
}

/// The menu as saved now: the text the app settings keep, the default's rows when it is the default.
pub fn current(app: &LightcraftApp) -> Vec<String> {
    app.ui.settings.photo_menu.clone().unwrap_or_else(|| DEFAULT.iter().map(|s| s.to_string()).collect())
}

/// Make `items` the menu; the default arrangement is saved as None, so it keeps following the default.
pub fn set_menu(app: &mut LightcraftApp, items: Vec<String>) {
    app.ui.settings.photo_menu = if items.iter().map(String::as_str).eq(DEFAULT.iter().copied()) { None } else { Some(items) };
}

/// What one slot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotState {
    pub saved: bool,
    /// The slot last saved or switched to.
    pub active: bool,
    /// Active, but the menu has changed since (Save to Preset N keeps the change).
    pub edited: bool,
}

pub fn states(app: &LightcraftApp) -> [SlotState; COUNT] {
    let presets = &app.ui.settings.photo_menu_presets;
    let now = current(app);
    std::array::from_fn(|i| {
        let saved = presets.get(i);
        let active = presets.active == Some(i) && saved.is_some();
        SlotState { saved: saved.is_some(), active, edited: active && saved != Some(&now) }
    })
}

/// Whether the last switch can be taken back.
pub fn can_undo(app: &LightcraftApp) -> bool {
    app.ui.settings.photo_menu_presets.undo.as_ref().is_some_and(|u| u.after == current(app))
}

/// Save the menu as it is now into `slot` (counting from 0) and make it the active one.
pub fn save_to(app: &mut LightcraftApp, slot: usize) -> Result<(), String> {
    if slot >= COUNT {
        return Err(bad_slot());
    }
    let now = current(app);
    let presets = &mut app.ui.settings.photo_menu_presets;
    presets.put(slot, Some(now));
    presets.active = Some(slot);
    Ok(())
}

/// Empty `slot` (counting from 0); the menu stays as it is. An empty slot is an error.
pub fn clear(app: &mut LightcraftApp, slot: usize) -> Result<(), String> {
    if slot >= COUNT {
        return Err(bad_slot());
    }
    let presets = &mut app.ui.settings.photo_menu_presets;
    if presets.get(slot).is_none() {
        return Err(format!("Preset {} is already empty", slot + 1));
    }
    presets.put(slot, None);
    if presets.active == Some(slot) {
        presets.active = None;
    }
    if presets.undo.as_ref().is_some_and(|u| u.active == Some(slot)) {
        presets.undo = presets.undo.take().map(|u| Undo { active: None, ..u });
    }
    Ok(())
}

/// Switch the menu to what `slot` holds. The menu it replaces can be brought back with [`undo`].
pub fn switch_to(app: &mut LightcraftApp, slot: usize) -> Result<(), String> {
    if slot >= COUNT {
        return Err(bad_slot());
    }
    let Some(items) = app.ui.settings.photo_menu_presets.get(slot).cloned() else {
        return Err(format!("Preset {} is empty: arrange the menu, then use Save to Preset {}", slot + 1, slot + 1));
    };
    let before = Undo { items: current(app), active: app.ui.settings.photo_menu_presets.active, after: items.clone() };
    let changes = before.items != items;
    set_menu(app, items);
    let presets = &mut app.ui.settings.photo_menu_presets;
    presets.active = Some(slot);
    // switching to what is already there changes nothing, so there is nothing to take back
    if changes {
        presets.undo = Some(before);
    }
    Ok(())
}

/// Bring back the menu the last switch replaced.
pub fn undo(app: &mut LightcraftApp) -> Result<(), String> {
    if !can_undo(app) {
        return Err("Nothing to undo: the menu has not been switched to a preset, or has changed since".into());
    }
    let Some(undo) = app.ui.settings.photo_menu_presets.undo.take() else { return Ok(()) };
    set_menu(app, undo.items);
    app.ui.settings.photo_menu_presets.active = undo.active;
    Ok(())
}

fn bad_slot() -> String {
    format!("`slot` must be a number from 1 to {COUNT}")
}

/// The slot a command names (1 to [`COUNT`]), as an index from 0.
fn slot_param(id: &str, p: &Value) -> Result<usize, String> {
    p.get("slot")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| (1..=COUNT).contains(n))
        .map(|n| n - 1)
        .ok_or_else(|| format!("{id}: {}", bad_slot()))
}

/// `photoMenu.savePreset` / `switchPreset` / `clearPreset` / `undoPreset`.
pub fn command(app: &mut LightcraftApp, id: &str, p: &Value) -> Result<(), String> {
    match id {
        "photoMenu.savePreset" => save_to(app, slot_param(id, p)?),
        "photoMenu.switchPreset" => switch_to(app, slot_param(id, p)?),
        "photoMenu.clearPreset" => clear(app, slot_param(id, p)?),
        _ => undo(app),
    }
}

/// The presets as `photoMenu.get` reports them: slot numbers count from 1.
pub fn report(app: &LightcraftApp) -> Value {
    let presets = &app.ui.settings.photo_menu_presets;
    let list: Vec<Value> = states(app)
        .iter()
        .enumerate()
        .map(|(i, s)| json!({"slot": i + 1, "saved": s.saved, "active": s.active, "edited": s.edited, "items": presets.get(i)}))
        .collect();
    json!(list)
}
