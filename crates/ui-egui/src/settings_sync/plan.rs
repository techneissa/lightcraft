//! Reading a settings file against the settings now ([`plan`]), which changes nothing, and
//! putting what it found in place ([`apply`]).
//!
//! A bad value for a setting this build knows (a wrong type, an unknown choice, a number out of
//! range, a shortcut that isn't one) is an error that names it and nothing changes; in a file from
//! a newer LightCraft it is skipped with a note instead. A setting, section or part this build
//! doesn't have is skipped and named, in a file of any version ([`Plan::unknown`]).

use serde_json::{Map, Value, json};

use super::rules::{SYNCED, SYNCED_SETTINGS, left_out, left_out_setting};
use super::{KIND, MAX_BYTES, VERSION};
use crate::LightcraftApp;
use crate::state::{CLICK_ZOOMS, UiState};

/// How many unknown settings are named (a hand-made file could name thousands).
pub const MAX_UNKNOWN: usize = 200;
/// How many notes are kept.
const MAX_NOTES: usize = 200;

/// What an import will do, found out without changing anything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Top-level keys of the UI state the file sets.
    pub ui: Map<String, Value>,
    /// Keys of `settings` the file sets.
    pub settings: Map<String, Value>,
    /// User presets the file adds.
    pub presets: Vec<lightcraft_develop::Preset>,
    /// The file's version when it is newer than this build's (what it understands was taken).
    pub newer: Option<u64>,
    /// The parts of the file that were read (`ui`, `presets`).
    pub parts: Vec<&'static str>,
    /// What was skipped or left out, one line each.
    pub notes: Vec<String>,
    /// Settings, sections and parts of the file this build doesn't have (`ui.key`, `ui.settings.key`,
    /// or a part's name), skipped; at most [`MAX_UNKNOWN`].
    pub unknown: Vec<String>,
    /// How many unknown settings the file had (more than `unknown` holds when it is long).
    pub unknown_total: usize,
}

impl Plan {
    /// The reply of the commands.
    pub fn report(&self, applied: bool) -> Value {
        json!({
            "applied": applied,
            "parts": self.parts,
            "settings": self.ui.len() + self.settings.len(),
            "presets": self.presets.len(),
            "fileVersion": self.newer,
            "skipped": self.notes,
            "unknown": self.unknown,
            "unknownCount": self.unknown_total,
        })
    }

    fn unknown(&mut self, name: &str) {
        self.unknown_total += 1;
        if self.unknown.len() < MAX_UNKNOWN {
            self.unknown.push(name.chars().take(60).collect());
        }
    }

    fn note(&mut self, msg: impl Into<String>) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(msg.into());
        }
    }
}

/// The file as a JSON object and its version, refused when it is not ours.
fn header(text: &str) -> Result<(Map<String, Value>, u64), String> {
    if text.len() > MAX_BYTES {
        return Err("This file is too big to be a settings file".into());
    }
    let doc: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("This isn't a LightCraft settings file: it isn't readable JSON ({e})"))?;
    let Value::Object(obj) = doc else { return Err("This isn't a LightCraft settings file: it isn't a JSON object".into()) };
    match obj.get("kind").and_then(Value::as_str) {
        Some(KIND) => {}
        Some(other) => return Err(format!("This isn't a LightCraft settings file (its kind is “{}”)", other.chars().take(40).collect::<String>())),
        None => return Err("This isn't a LightCraft settings file (it has no \"kind\")".into()),
    }
    match obj.get("version").and_then(Value::as_u64) {
        Some(v) if v >= 1 => Ok((obj, v)),
        _ => Err("This settings file has no readable \"version\"".into()),
    }
}

/// The numbers a setting may hold: (key, least, most).
const RANGES: &[(&str, f64, f64)] = &[
    ("brushSize", 0.002, 0.5),
    ("brushFeather", 0.0, 100.0),
    ("brushFlow", 0.0, 100.0),
    ("removeSize", 0.0, 1.0),
    ("removeFeather", 0.0, 100.0),
    ("removeOpacity", 0.0, 100.0),
    ("maskOverlayOpacity", 0.0, 100.0),
    ("spotsThreshold", 0.0, 100.0),
    ("cropOverlayOrient", 0.0, 4.0),
];

/// Why this build can't take `value` for the top-level setting `key`; `None` when it can.
fn problem_with(key: &str, value: &Value) -> Option<String> {
    if let Err(e) = serde_json::from_value::<UiState>(json!({ key: value })) {
        return Some(e.to_string());
    }
    if let Some((_, least, most)) = RANGES.iter().find(|(k, ..)| *k == key) {
        let n = value.as_f64()?;
        if !(*least..=*most).contains(&n) {
            return Some(format!("{n} is out of range ({least} to {most})"));
        }
    }
    if key == "clickZoom" && value.as_u64().is_none_or(|n| !CLICK_ZOOMS.iter().any(|z| u64::from(*z) == n)) {
        return Some(format!("{value} isn't one of {CLICK_ZOOMS:?}"));
    }
    None
}

/// Why this build can't take `value` for `settings.<key>`; `None` when it can.
fn problem_with_setting(key: &str, value: &Value) -> Option<String> {
    if let Err(e) = serde_json::from_value::<UiState>(json!({"settings": { key: value }})) {
        return Some(e.to_string());
    }
    if key == "keymap" {
        return keymap_problem(value);
    }
    None
}

/// The command a keymap entry (`id`, `id#2` or `id?when`) belongs to.
fn command_of(entry: &str) -> &str {
    crate::keybinds::model::split_key(crate::keybinds::model::conditions::command_of(entry)).0
}

/// Why this build can't take a keymap (a shortcut that isn't one, a condition that doesn't parse), naming the entry.
fn keymap_problem(value: &Value) -> Option<String> {
    use crate::keybinds::chord;
    use crate::keybinds::model::conditions;
    let map = value.as_object()?;
    for (key, v) in map {
        let id = command_of(key);
        if crate::shortcuts::find_bindable(id).is_none() {
            continue;
        }
        let text = v.as_str()?;
        let bad = if key.ends_with(conditions::SUFFIX) {
            !text.trim().is_empty() && conditions::set(&mut Default::default(), id, Some(text)).is_err()
        } else {
            !text.is_empty() && !chord::is_valid(text)
        };
        if bad {
            return Some(format!("“{key}” can't be {text:?}"));
        }
    }
    None
}

/// The names in the lists of `settings` and the top level that hold ids of this build's own parts
/// (sidebar and Edit panel sections, top bar items, photo menu slots, shortcuts of commands) that
/// this build doesn't have: those entries are dropped when the setting is read, so they are named
/// here as skipped.
fn unknown_entries(path: &str, value: &Value) -> Vec<String> {
    let entry_id = |e: &Value| e.as_str().or_else(|| e.get("id").and_then(Value::as_str)).map(str::to_string);
    let list = |value: &Value, known: &dyn Fn(&str) -> bool| -> Vec<String> {
        value.as_array().into_iter().flatten().filter_map(entry_id).filter(|id| !known(id)).collect()
    };
    let names = match path {
        "sidebar" => list(value, &|id| crate::sidebar::SidebarSection::from_id(id).is_some()),
        "editSections" => list(value, &|id| crate::panels::edit_sections::EditSection::from_id(id).is_some()),
        "settings.topBarHidden" => list(value, &|id| crate::panels::topbar_items::ITEMS.iter().any(|i| i.id == id)),
        "settings.photoMenu" => list(value, &|id| crate::panels::photo_menu::Slot::parse(id).is_some()),
        "settings.keymap" => value
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k.clone())
            .filter(|k| crate::shortcuts::find_bindable(command_of(k)).is_none())
            .collect(),
        _ => Vec::new(),
    };
    names.into_iter().map(|n| format!("{path}.{n}")).collect()
}

/// Read `text` as a settings file, and say what importing it would do.
/// A file that can't be taken is an error and nothing else happens: [`Plan`] is all a caller keeps.
/// User presets are read when `with_presets`.
pub fn plan(text: &str, with_presets: bool) -> Result<Plan, String> {
    let (file, version) = header(text)?;
    let strict = version <= VERSION;
    let mut plan = Plan::default();
    let mut problems: Vec<String> = Vec::new();
    // a value this build can't take: an error in a file of this version, skipped in a newer one
    let mut bad = |plan: &mut Plan, what: String| {
        if strict {
            problems.push(what);
        } else {
            plan.note(format!("Skipped {what}"));
        }
    };
    if version > VERSION {
        plan.newer = Some(version);
    }
    for key in file.keys().filter(|k| !matches!(k.as_str(), "kind" | "version" | "app" | "savedBy" | "ui" | "presets")) {
        plan.unknown(key);
    }
    let defaults = serde_json::to_value(UiState::default()).unwrap_or(Value::Null);
    let known = |key: &str| left_out(key).is_none() && (SYNCED.contains(&key) || defaults.get(key).is_some());
    let known_setting = |key: &str| {
        left_out_setting(key).is_none() && (SYNCED_SETTINGS.contains(&key) || defaults.get("settings").and_then(|s| s.get(key)).is_some())
    };
    if let Some(part) = file.get("ui") {
        plan.parts.push("ui");
        match part.as_object() {
            None => bad(&mut plan, "ui: it must be an object".into()),
            Some(map) => {
                for (key, value) in map {
                    if key == "settings" {
                        let Some(inner) = value.as_object() else {
                            bad(&mut plan, "ui.settings: it must be an object".into());
                            continue;
                        };
                        for (k, v) in inner {
                            if !known_setting(k) {
                                plan.unknown(&format!("ui.settings.{k}"));
                            } else if let Some(why) = problem_with_setting(k, v) {
                                bad(&mut plan, format!("ui.settings.{k}: {why}"));
                            } else {
                                for name in unknown_entries(&format!("settings.{k}"), v) {
                                    plan.unknown(&format!("ui.{name}"));
                                }
                                plan.settings.insert(k.clone(), v.clone());
                            }
                        }
                    } else if !known(key) {
                        plan.unknown(&format!("ui.{key}"));
                    } else if let Some(why) = problem_with(key, value) {
                        bad(&mut plan, format!("ui.{key}: {why}"));
                    } else {
                        for name in unknown_entries(key, value) {
                            plan.unknown(&format!("ui.{name}"));
                        }
                        plan.ui.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }
    if let Some(part) = file.get("presets") {
        plan.parts.push("presets");
        if with_presets {
            match lightcraft_engine::presets::parse_preset_file("presets.lcpreset", part.to_string().as_bytes()) {
                Ok(list) => plan.presets = list,
                Err(e) => bad(&mut plan, format!("presets: {e}")),
            }
        } else {
            plan.note("Left out the user presets (Include User Presets is off)");
        }
    }
    if !problems.is_empty() {
        let shown: Vec<&str> = problems.iter().take(3).map(String::as_str).collect();
        let more = problems.len().saturating_sub(shown.len());
        let mut msg = format!("This settings file can't be used, and nothing was changed: {}", shown.join("; "));
        if more > 0 {
            msg.push_str(&format!("; and {more} more"));
        }
        return Err(msg);
    }
    Ok(plan)
}

/// The settings of `app` with the runtime state a settings file never holds carried over
/// (what `ui.set` keeps, and the state of work in progress).
fn carry_over(old: &mut UiState, next: &mut UiState) {
    macro_rules! keep {
        ($($f:ident),* $(,)?) => { $( std::mem::swap(&mut next.$f, &mut old.$f); )* };
    }
    keep!(
        toast,
        status,
        preview_build_seen,
        unsaved_seen,
        frame_rate,
        right_hidden,
        was_focused,
        external_edits,
        known_cards,
        auto_import_at,
        auto_watch,
        auto_import_error,
        sidecar_retry_at,
        sidecars_waiting,
        rechecked_library,
        compare,
        reference,
        person_page,
        person_from,
        last_person,
        proof_profile,
        proof_intent,
        lights_out,
        cull_burst,
        dismissed_faces,
        print,
        book,
    );
}

/// Put `plan` in place: the settings it names are replaced, the rest stay. Returns the ids of the
/// presets added. An error (the result couldn't be read back as settings) changes nothing.
pub fn apply(app: &mut LightcraftApp, plan: Plan) -> Result<Vec<String>, String> {
    let mut doc = serde_json::to_value(&app.ui).map_err(|e| e.to_string())?;
    let Some(root) = doc.as_object_mut() else { return Err("the settings could not be read".into()) };
    for (k, v) in plan.ui {
        root.insert(k, v);
    }
    if !plan.settings.is_empty() {
        let settings = root.entry("settings").or_insert_with(|| json!({}));
        if let Some(s) = settings.as_object_mut() {
            for (k, v) in plan.settings {
                s.insert(k, v);
            }
        }
    }
    let mut next: UiState = serde_json::from_value(doc).map_err(|e| format!("These settings can't be used: {e}"))?;
    // keep what the file never holds; the dialog that asked closes itself
    carry_over(&mut app.ui, &mut next);
    next.dialog = None;
    app.ui = next;
    crate::i18n::set_language(app.ui.language);
    Ok(if plan.presets.is_empty() { Vec::new() } else { app.session.add_presets(plan.presets) })
}
