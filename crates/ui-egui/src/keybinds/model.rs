//! Two shortcuts per command, in the existing keymap storage ([`crate::shortcuts::Keymap`], saved in
//! `ui.json`). Slot 1 is the entry `id` as before (so an old file loads unchanged and an old build
//! still reads it); slot 2 is the entry `id#2`, which an old build ignores as an unknown command.
//! A slot holds one step or a chord ([`super::chord`]). Slot 1 is the primary one: menus, tooltips
//! and the Command Palette show it; slot 2 also fires the command. A few commands have a declared
//! second shortcut ([`SECOND_DEFAULTS`]: ⌘D for Select None, F6 for the filmstrip…), edited and
//! reset like the first.

use crate::shortcuts::{self, CONTEXTUAL, Keymap, RESERVED, find_bindable};

use super::chord::{self, Step};

/// Shortcuts a command can have.
pub const SLOTS: usize = 2;

/// Declared second shortcuts: Lightroom Classic keys that our first shortcuts give elsewhere.
/// Each yields to a key the user gave another command, like a declared first shortcut.
pub const SECOND_DEFAULTS: &[(&str, &str)] = &[
    ("library.selectNone", "Cmd+D"),
    ("dialog.export", "Shift+E"),
    ("app.exportPrevious", "Cmd+E"),
    ("view.zoomToggle", "Space"),
    // Lightroom Classic: F6 toggles the filmstrip, F7 the left library panel, F8 the right one
    ("view.filmstrip", "F6"),
    ("view.leftPanel", "F7"),
    ("view.rightPanel", "F8"),
    ("version.create", "Shift+M"),
    // grid thumbnail size: Classic's key is `=`; `+` (keypad, or Shift+= reported as Plus) agrees
    ("view.thumbSizeUp", "Plus"),
    // Shift+[ / Shift+] arrive as { / } on most layouts
    ("brush.featherLess", "Shift+{"),
    ("brush.featherMore", "Shift+}"),
];

/// The declared second shortcut of `id`.
pub fn default2(id: &str) -> Option<&'static str> {
    SECOND_DEFAULTS.iter().find(|(c, _)| *c == id).map(|(_, sc)| *sc)
}

/// One binding: a command and which of its slots.
pub type Holder = (&'static str, usize);

/// The keymap entry of `slot` (0 = primary, 1 = second) of command `id`.
pub fn slot_key(id: &str, slot: usize) -> String {
    if slot == 0 { id.to_string() } else { format!("{id}#{}", slot + 1) }
}

/// The command a keymap entry belongs to and its slot (`view.survey#2` → `view.survey`, 1).
pub fn split_key(key: &str) -> (&str, usize) {
    match key.rsplit_once('#').and_then(|(id, n)| Some((id, n.parse::<usize>().ok()?.checked_sub(1)?))) {
        Some((id, slot)) if slot > 0 && slot < SLOTS => (id, slot),
        _ => (key, 0),
    }
}

/// The shortcut text in `slot` of `id` now: the declared one or the user's choice for slot 1, the
/// user's choice for slot 2. An entry that doesn't parse is ignored.
pub fn effective<'a>(keymap: &'a Keymap, id: &str, slot: usize) -> Option<&'a str> {
    match slot {
        0 => shortcuts::shortcut_of(keymap, id),
        _ => {
            find_bindable(id)?;
            match keymap.get(&slot_key(id, slot)) {
                Some(sc) if sc.is_empty() => None,
                Some(sc) if chord::is_valid(sc) => Some(sc.as_str()),
                // no entry (or a damaged one): the declared key, unless a saved entry gives it elsewhere
                _ => default2(id).filter(|sc| !saved_elsewhere(keymap, id, sc)),
            }
        }
    }
}

/// Whether the keymap itself gives the one-step key `sc` to a known command other than `id`.
fn saved_elsewhere(keymap: &Keymap, id: &str, sc: &str) -> bool {
    let Some(steps) = chord::parse_chord(sc) else { return false };
    keymap.iter().any(|(k, v)| split_key(k).0 != id && find_bindable(split_key(k).0).is_some() && chord::parse_chord(v).is_some_and(|o| o == steps))
}

/// Both slots of `id`.
pub fn effective_slots<'a>(keymap: &'a Keymap, id: &str) -> [Option<&'a str>; SLOTS] {
    [effective(keymap, id, 0), effective(keymap, id, 1)]
}

/// Whether the user changed `id`: a saved entry in either slot, or a declared key that yields to
/// another command's.
pub fn is_changed(keymap: &Keymap, id: &str) -> bool {
    (0..SLOTS).any(|s| keymap.contains_key(&slot_key(id, s)))
        || find_bindable(id).is_some_and(|b| b.default.is_some() && effective(keymap, id, 0).is_none())
        || (default2(id).is_some() && effective(keymap, id, 1) != default2(id))
}

/// Whether the keymap holds an entry for a command this build knows (what Reset All would drop).
pub fn has_entries(keymap: &Keymap) -> bool {
    keymap.keys().any(|k| find_bindable(split_key(k).0).is_some())
}

/// Every second shortcut that is one step, as the dispatcher matches it: command, key, its text and
/// whether it is the declared one (those run as the fixed aliases always did).
pub fn second_singles(keymap: &Keymap) -> Vec<(&'static str, Step, String, bool)> {
    let saved = keymap.keys().filter(|k| split_key(k).1 > 0).filter_map(|k| find_bindable(split_key(k).0)).map(|b| b.id);
    let declared = SECOND_DEFAULTS.iter().filter_map(|(id, _)| find_bindable(id)).map(|b| b.id);
    let mut ids: Vec<&'static str> = saved.chain(declared).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| {
            let sc = effective(keymap, id, 1)?;
            match chord::parse_chord(sc)?.as_slice() {
                [one] => Some((id, *one, sc.to_string(), default2(id) == Some(sc))),
                _ => None,
            }
        })
        .collect()
}

/// Every chord, of either slot, as the dispatcher matches it: command, both steps.
pub fn chords(keymap: &Keymap) -> Vec<(&'static str, [Step; 2])> {
    // a declared shortcut is never a chord, so only the saved entries can be
    let mut v = Vec::new();
    for (key, sc) in keymap.iter().filter(|(_, sc)| chord::is_chord(sc)) {
        let (id, slot) = split_key(key);
        if let Some(b) = find_bindable(id)
            && effective(keymap, b.id, slot) == Some(sc.as_str())
            && let Some([a, c]) = chord::parse_chord(sc).as_deref()
        {
            v.push((b.id, [*a, *c]));
        }
    }
    v
}

/// Keys the fixed bindings (ratings, aliases) yield to besides the single-step first shortcuts:
/// second shortcuts that are one step, and the leader of every chord.
pub fn extra_taken(keymap: &Keymap) -> Vec<Step> {
    second_singles(keymap).into_iter().map(|(_, s, ..)| s).chain(chords(keymap).into_iter().map(|(_, [first, _])| first)).collect()
}

/// The other bindings `steps` can't live next to: the same shortcut, a single key that leads a
/// chord, or a chord led by the single key (contextual pairs excepted). `id` itself is skipped.
pub fn conflicts(keymap: &Keymap, id: &str, steps: &[Step]) -> Vec<Holder> {
    let mut v = Vec::new();
    for b in shortcuts::bindable() {
        if b.id == id {
            continue;
        }
        // contextual partners share the very same key on purpose, but not a chord it leads
        let partner = CONTEXTUAL.iter().any(|(x, y)| (*x == id && *y == b.id) || (*y == id && *x == b.id));
        for slot in 0..SLOTS {
            if effective(keymap, b.id, slot)
                .and_then(chord::parse_chord)
                .is_some_and(|other| chord::overlaps(steps, &other) && !(partner && other == steps))
            {
                v.push((b.id, slot));
            }
        }
    }
    v
}

/// Store `sc` in `slot` of `id` without checking anything: an entry that equals the declared
/// shortcut is dropped, and "none" is kept as an empty entry where a declared shortcut exists.
fn put(keymap: &mut Keymap, id: &str, slot: usize, sc: Option<&str>) {
    if slot == 0 {
        shortcuts::set(keymap, id, sc);
    } else if let Some(sc) = sc {
        if default2(id).is_some_and(|d| chord::parse_chord(d) == chord::parse_chord(sc)) {
            keymap.remove(&slot_key(id, slot));
        } else {
            keymap.insert(slot_key(id, slot), sc.to_string());
        }
    } else if default2(id).is_some() {
        // "none" is an entry of its own when a declared key would come back otherwise
        keymap.insert(slot_key(id, slot), String::new());
    } else {
        keymap.remove(&slot_key(id, slot));
    }
}

/// Give `slot` of `id` the shortcut `sc` (`None` = none), taking it away from the commands that
/// had it or a key it overlaps. Returns the bindings that were taken away.
pub fn assign(keymap: &mut Keymap, id: &str, slot: usize, sc: Option<&str>) -> Result<Vec<Holder>, String> {
    let b = find_bindable(id).ok_or_else(|| format!("unknown command: {id}"))?;
    if slot >= SLOTS {
        return Err(format!("slot must be 1 or {SLOTS}"));
    }
    let steps = match sc.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(chord::parse_chord(s).ok_or_else(|| format!("not a shortcut: {s}"))?),
    };
    let canonical = match &steps {
        None => None,
        Some(steps) => {
            let text = chord::spell(steps).ok_or_else(|| format!("not a shortcut: {}", sc.unwrap_or_default()))?;
            // no step of a chord may be an app-menu key: the menu would take it before the chord saw it
            if RESERVED.iter().filter_map(|r| shortcuts::parse(r)).any(|r| steps.contains(&r)) {
                return Err(format!("{} is reserved for the app menu", chord::text(&text, false)));
            }
            Some(text)
        }
    };
    let lost = steps.as_deref().map(|s| conflicts(keymap, id, s)).unwrap_or_default();
    for (other, other_slot) in &lost {
        put(keymap, other, *other_slot, None);
    }
    // the command's other slot gives way to an overlapping shortcut, so one command never
    // blocks itself with a key and the chord it leads
    if let Some(steps) = &steps {
        for other_slot in (0..SLOTS).filter(|s| *s != slot) {
            if effective(keymap, id, other_slot).and_then(chord::parse_chord).is_some_and(|o| chord::overlaps(steps, &o)) {
                put(keymap, id, other_slot, None);
            }
        }
    }
    put(keymap, b.id, slot, canonical.as_deref());
    Ok(lost)
}

/// Restore both slots of `id`: the declared shortcut in slot 1 (taken away from a command the user
/// gave it to), nothing in slot 2.
pub fn reset(keymap: &mut Keymap, id: &str) -> Result<Vec<Holder>, String> {
    let b = find_bindable(id).ok_or_else(|| format!("unknown command: {id}"))?;
    keymap.remove(&slot_key(id, 1));
    let mut lost = assign(keymap, id, 0, b.default)?;
    lost.extend(assign(keymap, id, 1, default2(id))?);
    Ok(lost)
}

#[cfg(test)]
mod tests;
