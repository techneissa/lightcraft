//! `relight.presets`, `relight.applyPreset`, `relight.savePreset`, `relight.saveLightPreset`, `relight.renamePreset`, `relight.deletePreset` (fork,
//! issue #301, `fork/relight.md`): light presets, one click that sets up several lights round the photo's subject.
//! A preset is named by `preset` (its id, else its name); `id` is a photo, as in the other `relight.*` commands.

use serde_json::{Value, json};

use lightcraft_develop::DevelopSettings;
use lightcraft_develop::relight_presets::{LightPreset, MAX_NAME, MAX_USER, Subject, clean_name};

use super::{CommandSpec, always, bad, cmd, has_active, photo_id, str_param};
use crate::relight::Depth;
use crate::relight_presets::SubjectSource;
use crate::{Result, Session};

fn subject_json((s, source): (Subject, SubjectSource)) -> Value {
    json!({"x": s.x, "y": s.y, "w": s.w, "h": s.h, "source": source.id(), "depth": s.depth.map(|d| json!({"own": d.own, "span": d.span}))})
}

fn named(s: &Session, p: &Value, c: &str) -> Result<LightPreset> {
    let key = str_param(p, "preset").ok_or_else(|| bad(c, "missing `preset`: a preset's id or name (relight.presets lists them)"))?;
    s.relight_preset(key).ok_or_else(|| bad(c, format!("no light preset {key:?}: relight.presets lists them")))
}

/// `raw` as a new preset's name: cleaned, at most [`MAX_NAME`] characters, and not another preset's (`except`: the one being renamed).
fn name_of(s: &Session, raw: Option<&str>, except: Option<&str>, c: &str) -> Result<String> {
    let name = raw.and_then(clean_name).ok_or_else(|| bad(c, "give the preset a name"))?;
    if name.chars().count() > MAX_NAME {
        return Err(bad(c, format!("a name is at most {MAX_NAME} characters")));
    }
    let list = s.relight_preset_list();
    let others = list.iter().filter(|p| Some(p.id.as_str()) != except).map(|p| p.name.as_str());
    crate::names::check_name(&name, others).map_err(|e| bad(c, e.to_string()))
}

fn presets(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.presets";
    let id = photo_id(p, c)?.or_else(|| s.active());
    // (the photo's depth is found by the worker as for the Relight section: nothing is waited for)
    let finding = match id {
        Some(i) if p.get("find").and_then(Value::as_bool) == Some(true) => matches!(s.relight_depth(i), Depth::Finding),
        Some(i) => s.relight_status(i)["finding"].as_bool().unwrap_or(false),
        None => false,
    };
    let list: Vec<Value> = s
        .relight_preset_list()
        .iter()
        .map(|q| {
            let kinds: Vec<&str> = q.lights.lights.iter().map(|l| l.kind.name()).collect();
            json!({"id": q.id, "name": q.name, "builtin": q.builtin, "lights": q.lights.lights.len(), "kinds": kinds, "darken": q.lights.darken, "water": q.has_water(), "single": q.is_single()})
        })
        .collect();
    let subject = id.map(|i| subject_json(s.relight_subject(i)));
    Ok(json!({"presets": list, "subject": subject, "finding": finding}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.applyPreset";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let preset = named(s, p, c)?;
    let add = match p.get("add") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(c, "`add` is true or false")),
    };
    // (the lights go by the subject's own depth: while the worker is still finding the photo's depth nothing is applied, and
    // nothing waits; a depth that was cancelled or failed is not tried again, the lights then take their depth as written)
    if matches!(s.relight_depth(id), Depth::Finding) {
        return Ok(json!({"applied": false, "finding": true, "preset": {"id": preset.id, "name": preset.name}}));
    }
    let subject = s.relight_subject(id);
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    if add {
        let (out, left_out, first) = preset.added_to(&d, &subject.0);
        let count = out.relight.lights.len();
        let added = first.map_or(0, |f| count - f);
        // (nothing fitted: nothing changes, not even the water, and there is no undo step for it)
        if first.is_none() {
            return Ok(
                json!({"applied": false, "added": 0, "leftOut": left_out, "preset": {"id": preset.id, "name": preset.name}, "lights": d.relight.lights.len()}),
            );
        }
        s.set_develop(id, out, &format!("Add Light Preset: {}", preset.name))?;
        s.relight_select(id, first.unwrap_or(0));
        return Ok(
            json!({"applied": true, "added": added, "leftOut": left_out, "preset": {"id": preset.id, "name": preset.name}, "lights": count, "subject": subject_json(subject)}),
        );
    }
    let out = preset.onto(&d, &subject.0);
    let count = out.relight.lights.len();
    s.set_develop(id, out, &format!("Light Preset: {}", preset.name))?;
    s.relight_select(id, 0);
    Ok(json!({"applied": true, "preset": {"id": preset.id, "name": preset.name}, "lights": count, "subject": subject_json(subject)}))
}

/// Save a preset of your own from the active photo: `make` builds it from the photo's lights, the subject and the new id.
fn keep(s: &mut Session, p: &Value, c: &str, make: impl FnOnce(&DevelopSettings, &Subject, &str, &str) -> Result<LightPreset>) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let name = name_of(s, str_param(p, "name"), None, c)?;
    if s.relight_presets.len() >= MAX_USER {
        return Err(bad(c, format!("at most {MAX_USER} light presets of your own: delete one first")));
    }
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    // (a preset is kept relative to the subject's depth: with no depth found its lights' depths would later be read as relative
    // ones that they are not)
    match s.relight_depth(id) {
        Depth::Ready => {}
        Depth::Finding => return Err(bad(c, "Relight is still finding this photo's depth: save the preset once it is found")),
        Depth::Missing => {
            let why = s.relight_status(id)["error"].as_str().map(|e| format!(" ({e})")).unwrap_or_default();
            return Err(bad(c, format!("this photo's depth can't be found{why}: a light preset is saved relative to the subject's depth")));
        }
    }
    let subject = s.relight_subject(id);
    // (an id no preset has, kept stable when others are deleted; one that would overflow is an error)
    let next = s
        .relight_presets
        .iter()
        .filter_map(|q| q.id.strip_prefix("user-").and_then(|n| n.parse::<u64>().ok()))
        .max()
        .map_or(Some(1), |n| n.checked_add(1))
        .ok_or_else(|| bad(c, "no free id for a new preset: delete the presets of your own that were written by hand"))?;
    let preset = make(&d, &subject.0, &format!("user-{next}"), &name)?;
    let out = json!({"id": preset.id, "name": preset.name, "lights": preset.lights.lights.len(), "water": preset.has_water(), "subject": subject_json(subject)});
    s.relight_presets.push(preset);
    Ok(out)
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.savePreset";
    keep(s, p, c, |d, subject, id, name| {
        if !d.relight.is_on() {
            return Err(bad(c, "the photo has no light turned on: raise a light's Amount first"));
        }
        Ok(LightPreset::capture(id, name, &d.relight, subject))
    })
}

fn save_light(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.saveLightPreset";
    let sel = s.active().map_or(0, |id| s.relight_selected(id));
    let index = match p.get("index") {
        None | Some(Value::Null) => sel,
        Some(v) => v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| bad(c, "`index` is a light's number, from 0"))?,
    };
    keep(s, p, c, |d, subject, id, name| {
        let light =
            d.relight.lights.get(index).ok_or_else(|| bad(c, format!("the photo has no light {index}: it has {}", d.relight.lights.len())))?;
        if light.amount <= 0.0 {
            return Err(bad(c, "that light is off: raise its Amount first"));
        }
        LightPreset::capture_light(id, name, &d.relight, index, subject).ok_or_else(|| bad(c, "no such light"))
    })
}

fn user_index(s: &Session, p: &Value, c: &str) -> Result<usize> {
    let preset = named(s, p, c)?;
    if preset.builtin {
        return Err(bad(c, "the built-in presets can't be changed"));
    }
    s.relight_presets.iter().position(|q| q.id == preset.id).ok_or_else(|| bad(c, "no such preset"))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.renamePreset";
    let i = user_index(s, p, c)?;
    let id = s.relight_presets[i].id.clone();
    let name = name_of(s, str_param(p, "name"), Some(&id), c)?;
    s.relight_presets[i].name = name.clone();
    Ok(json!({"id": id, "name": name}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.deletePreset";
    let i = user_index(s, p, c)?;
    let gone = s.relight_presets.remove(i);
    Ok(json!({"id": gone.id, "name": gone.name}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "relight.presets",
            "Light Presets",
            [],
            None,
            "{id?: photo, find?: bool} → {finding, presets: [{id, name, builtin, lights, kinds, darken, water, single}], subject: {x, y, w, h, source: face|mask|centre, depth?: {own, span}}} — the light presets (the built-in ones Rembrandt, Butterfly, Split, Rim + Fill, Golden-Hour Window, Neon Two-Tone, Candle, Concert Backlight, Two-Colour Stage and Single Spotlight, then your own) and where the photo's subject is: the face the preset's lights are placed round (the photo's face, else one worked out from a person or subject mask, else one at the centre) and, once the photo's depth is found, its depth (`own`: the subject's depth as a light's depth value 0..100; `span`: the log depth 100 of them cover). `find: true` starts finding the depth (on the worker, as the Relight section does; never waited for); `finding` says it is being found",
            always,
            presets
        ),
        cmd!(
            "relight.applyPreset",
            "Apply Light Preset",
            [],
            None,
            "{preset: a preset's id or name, add?: bool} → {preset: {id, name}, lights, subject} — replace the active photo's lights (and Darken Surroundings) with the preset's, placed round the photo's face and at its depth, and turn the Relight section on; one undo step. While the photo's depth is still being found nothing is applied and `{applied: false, finding: true}` comes back at once (apply again when `relight.status` says ready); a depth that was cancelled or failed is not tried again and the lights take their depth as written. The photo's other settings are not touched. A preset saved with Underwater sets the photo's Underwater too; one without leaves it. With `add: true` the preset's lights are appended after the photo's instead (at most 8: `leftOut` says how many did not fit; a photo with nothing set has nothing to keep and takes them as its own), Darken Surroundings stays the photo's, the first added light is selected, and the result is `{applied, added, leftOut, lights, preset, subject}`; `applied: false` when nothing fitted; one undo step",
            has_active,
            apply
        ),
        cmd!(
            "relight.savePreset",
            "Save Lights as Preset",
            [],
            None,
            "{name: up to 40 characters, not another preset's} → {id, name, lights, water, subject} — once the photo's depth is found (an error says so until then), save the active photo's lights (and Darken Surroundings) as a preset of your own, kept relative to the photo's subject so it fits any photo; an error while no light is turned on. The photo's Underwater is saved with them when it is on (applying the preset sets it)",
            has_active,
            save
        ),
        cmd!(
            "relight.saveLightPreset",
            "Save This Light as Preset",
            [],
            None,
            "{name: up to 40 characters, not another preset's, index?: light (default the selected one)} → {id, name, lights, water, subject} — as relight.savePreset, but of one light only (its type, place relative to the subject and every effect; no Darken Surroundings, no Underwater); an error while that light is off",
            has_active,
            save_light
        ),
        cmd!(
            "relight.renamePreset",
            "Rename Light Preset",
            [],
            None,
            "{preset: id or name of a preset of your own, name} → {id, name} — rename it (the built-in presets can't be)",
            always,
            rename
        ),
        cmd!(
            "relight.deletePreset",
            "Delete Light Preset",
            [],
            None,
            "{preset: id or name of a preset of your own} → {id, name} — delete it (the built-in presets can't be); photos that used it keep their lights",
            always,
            delete
        ),
    ]
}
