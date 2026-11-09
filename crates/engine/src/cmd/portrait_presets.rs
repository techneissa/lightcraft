//! `retouch.savePreset`, `retouch.applyToPerson` and `retouch.applyStatus`: portrait presets (fork feature,
//! `fork/portrait-presets.md`, issue #106). Applying one to chosen photos is `preset.apply`, which gives a portrait
//! preset's face settings to the same person's face on each photo (`crate::portrait_presets`).

use std::time::{Duration, Instant};

use lightcraft_catalog::PhotoId;
use lightcraft_develop::presets::{SettingsGroup, extract_groups};
use lightcraft_develop::retouch::{Features, KeyMap};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active, photo_id, str_param};
use crate::portrait_presets::person::Request;
use crate::portrait_presets::{self as pp, Matcher, Portrait, Who};
use crate::{Result, Session};

/// Longest preset name or group.
const MAX_TEXT: usize = 100;

/// Longest `retouch.applyStatus` waits.
const MAX_WAIT: Duration = Duration::from_secs(30);

fn target(s: &Session, p: &Value, c: &str) -> Result<PhotoId> {
    let id = photo_id(p, c)?.or_else(|| s.active()).ok_or_else(|| bad(c, "no photo"))?;
    s.catalog.photo(id).map(|_| id).ok_or_else(|| bad(c, format!("no photo {}", id.0)))
}

/// `face`: an index of `retouch.faces`, absent or null for none.
fn face_param(p: &Value, c: &str) -> Result<Option<usize>> {
    match p.get("face") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => super::index_req(p, "face", c).map(Some),
    }
}

/// Face `i` of photo `id` as found, its settings (its own, else every face's) and who it is.
fn face_of(s: &Session, id: PhotoId, i: usize, c: &str) -> Result<(Features, Who)> {
    let st = s.retouch_faces_of(id);
    let found = match st.found {
        Some(f) => f,
        None if st.busy => return Err(bad(c, "the photo's faces are still being found: try again in a moment")),
        None => return Err(bad(c, "the photo's faces are not known yet: run retouch.detect first")),
    };
    let face = found.input.faces.get(i).ok_or_else(|| bad(c, format!("no face {i}: the photo has {}", found.input.faces.len())))?;
    let d = s.develop_of(id).unwrap_or_default();
    let keys = s.retouch_key_map(id);
    let features = d.retouch.settings_for_in(&found.input.faces, i, keys.as_ref().map(|k| k as &dyn KeyMap)).clone();
    Ok((features, s.portrait_who(id, face)))
}

fn text(p: &Value, key: &str, c: &str) -> Result<Option<String>> {
    let Some(t) = str_param(p, key).map(str::trim).filter(|t| !t.is_empty()) else { return Ok(None) };
    if t.chars().count() > MAX_TEXT || t.chars().any(char::is_control) {
        return Err(bad(c, format!("`{key}` is at most {MAX_TEXT} characters, without control characters")));
    }
    Ok(Some(t.to_string()))
}

/// `retouch.savePreset`.
fn save(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "retouch.savePreset";
    let id = target(s, p, C)?;
    let name = text(p, "name", C)?.ok_or_else(|| bad(C, "missing `name`"))?;
    let group = text(p, "group", C)?.unwrap_or_else(|| pp::GROUP.to_string());
    let groups: Vec<SettingsGroup> = match p.get("groups") {
        None | Some(Value::Null) if p.get("others").and_then(Value::as_bool).unwrap_or(false) => SettingsGroup::default_copy(),
        None | Some(Value::Null) => Vec::new(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| bad(C, format!("invalid `groups`: {e}")))?,
    };
    // a picked face: that face and who it is; All Faces: the settings for every face, for any face
    let (features, who) = match face_param(p, C)? {
        Some(i) => face_of(s, id, i, C)?,
        None => (s.develop_of(id).unwrap_or_default().retouch.global.clone(), Who::default()),
    };
    let d = s.develop_of(id).unwrap_or_default();
    let others = extract_groups(&d, &pp::other_groups(&groups));
    let portrait = Portrait::of(&features, who);
    let preset = pp::make(&name, &group, &portrait, others);
    let pid = match s.add_presets(vec![preset.clone()]).into_iter().next() {
        Some(pid) => pid,
        // the same preset under the same name and group is there already
        None => s
            .presets
            .iter()
            .find(|q| !q.builtin && q.name == preset.name && q.group == preset.group && q.settings == preset.settings)
            .map(|q| q.id.clone())
            .ok_or_else(|| bad(C, "the preset could not be saved"))?,
    };
    Ok(json!({
        "id": pid,
        "name": name,
        "group": group,
        "person": portrait.who.person,
        "anyFace": portrait.is_any_face(),
        "byFace": portrait.who.embedding.is_some(),
        "wholeLook": preset.settings.as_object().is_some_and(|o| o.len() > 1),
    }))
}

/// `retouch.applyToPerson`.
fn apply_to_person(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "retouch.applyToPerson";
    let preset = match str_param(p, "preset") {
        Some(pid) => {
            let preset = s.presets.iter().find(|x| x.id == pid).cloned().ok_or_else(|| bad(C, format!("unknown preset `{pid}`")))?;
            if !pp::is_portrait(&preset) {
                return Err(bad(C, format!("`{pid}` is not a portrait preset (save one with retouch.savePreset)")));
            }
            Some(preset)
        }
        None => None,
    };
    // who, and (without a preset) the settings: a face of a photo, a person by name, or the preset's person
    let (who, from_face, source) = match (text(p, "name", C)?, face_param(p, C)?) {
        (Some(name), _) => (Who::named(&name), None, None),
        (None, Some(i)) => {
            let id = target(s, p, C)?;
            let (features, who) = face_of(s, id, i, C)?;
            (who, Some(features), Some(id))
        }
        (None, None) => match &preset {
            Some(pr) => (pp::split(pr).1.map(|x| x.who).unwrap_or_default(), None, None),
            None => return Err(bad(C, "pass `name` (a person), `face` (an index of retouch.faces) or `preset`")),
        },
    };
    if !who.is_known() {
        return Err(bad(
            C,
            "nobody to follow: name the face in People first (a preset saved from All Faces goes to any face: apply it with preset.apply)",
        ));
    }
    let (rest, portrait) = match &preset {
        Some(pr) => {
            let (rest, part) = pp::split(pr);
            let rest = rest.into_owned();
            let has_rest = rest.settings.as_object().is_some_and(|o| !o.is_empty());
            (has_rest.then_some(rest), part.unwrap_or_default())
        }
        None => {
            let features = from_face.ok_or_else(|| bad(C, "pass `preset` to apply to a person by name"))?;
            (None, Portrait::of(&features, who.clone()))
        }
    };
    let label = match &preset {
        Some(pr) => format!("Portrait Preset: {}", pr.name),
        None => "Apply Face to Person".to_string(),
    };
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    s.portrait_apply_to_person(Request { who, portrait, rest, exclude: source, label }, wait)
}

/// `retouch.applyStatus`.
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
        s.portrait_cancel();
    }
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let until = Instant::now() + p.get("timeoutMs").and_then(Value::as_u64).map_or(MAX_WAIT, Duration::from_millis).min(MAX_WAIT);
    loop {
        if let Some(r) = s.portrait_pump() {
            return r;
        }
        if !s.portrait_job_running() {
            return Ok(json!({"state": "idle"}));
        }
        if !wait || Instant::now() >= until {
            return Ok(json!({"state": "running"}));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "retouch.savePreset",
            "Save Portrait Preset",
            [],
            None,
            "{name, face?: index of retouch.faces, id?, group?: default Portrait, others?: bool, groups?: [settingsGroup]} → {id, name, group, person?, anyFace, byFace, wholeLook} — save portrait settings as a preset. With `face` it follows that person: applied with preset.apply it changes only their face on each photo (by name, else by face recognition). Without `face` (All Faces) it goes to every face of the photos it is applied to (`anyFace`). Face settings only by default; `others: true` makes it a whole look, adding the photo's other develop settings of the default copy groups (`groups` names them). Never local masks, spots, red eye or Refine strokes",
            has_active,
            save
        ),
        cmd!(
            "retouch.applyToPerson",
            "Apply to Every Photo of This Person",
            [],
            None,
            "{name?: person, face?: index of retouch.faces, id?, preset?: portrait preset id, wait?: bool} → {state: done | running, person, applied: [photo ids], count, changed, failed: [{id, why}]} — give a portrait preset (or, from `face`, that face's settings) to the person's face on every photo where they are named or recognised, as one undo step; photos without them are left alone. Over 24 photos with faces it runs in the background (`state: running`): finish it with retouch.applyStatus, or pass `wait`",
            super::always,
            apply_to_person
        ),
        cmd!(
            "retouch.applyStatus",
            "Portrait Apply Status",
            [],
            None,
            "{wait?: bool, timeoutMs?: up to 30000, cancel?: bool} → {state: idle | running | done | cancelled, …as retouch.applyToPerson} — how Apply to Every Photo in the background is going; once its work is done this commits it (one undo step) and says what happened. `cancel` stops it with nothing changed",
            super::always,
            status
        ),
    ]
}

/// What `preset.apply` adds for a portrait preset: the face part and how it went.
pub(crate) struct PresetFaces {
    pub portrait: Portrait,
    pub matcher: Matcher,
    pub applied: Vec<u64>,
    pub skipped: Vec<Value>,
}

impl PresetFaces {
    /// For `preset.apply`: `None` for a preset without a face part. A photo's only face may take it when it can't
    /// be compared: the user picked these photos.
    pub fn new(s: &Session, preset: &lightcraft_develop::presets::Preset) -> Option<PresetFaces> {
        let portrait = pp::split(preset).1?;
        let matcher = Matcher::new(s, &portrait.who, true);
        Some(PresetFaces { portrait, matcher, applied: Vec::new(), skipped: Vec::new() })
    }

    /// Give photo `id`'s face its part of `next`.
    pub fn apply(&mut self, s: &Session, id: PhotoId, next: &mut lightcraft_develop::DevelopSettings, amount: f64) {
        match s.portrait_onto(id, next, &self.portrait, &self.matcher, amount) {
            Ok(()) => self.applied.push(id.0),
            Err(why) => self.skipped.push(json!({"id": id.0, "why": why.as_str()})),
        }
    }

    pub fn report(&self) -> Value {
        json!({"applied": self.applied, "skipped": self.skipped, "anyFace": self.portrait.is_any_face()})
    }
}
