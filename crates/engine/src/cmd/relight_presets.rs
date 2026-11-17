//! `relight.presets`, `relight.applyPreset`, `relight.savePreset`, `relight.renamePreset`, `relight.deletePreset` (fork,
//! issue #301, `fork/relight.md`): light presets, one click that sets up several lights round the photo's subject.
//! A preset is named by `preset` (its id, else its name); `id` is a photo, as in the other `relight.*` commands.

use serde_json::{Value, json};

use lightcraft_develop::relight_presets::{LightPreset, MAX_NAME, MAX_USER, Subject, clean_name};

use super::{CommandSpec, always, bad, cmd, has_active, photo_id, str_param};
use crate::relight_presets::SubjectSource;
use crate::{Result, Session};

fn subject_json((s, source): (Subject, SubjectSource)) -> Value {
    json!({"x": s.x, "y": s.y, "w": s.w, "h": s.h, "source": source.id()})
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
    let list: Vec<Value> = s
        .relight_preset_list()
        .iter()
        .map(|q| {
            let kinds: Vec<&str> = q.lights.lights.iter().map(|l| l.kind.name()).collect();
            json!({"id": q.id, "name": q.name, "builtin": q.builtin, "lights": q.lights.lights.len(), "kinds": kinds, "darken": q.lights.darken})
        })
        .collect();
    let subject = id.map(|i| subject_json(s.relight_subject(i)));
    Ok(json!({"presets": list, "subject": subject}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.applyPreset";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let preset = named(s, p, c)?;
    let subject = s.relight_subject(id);
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    let out = preset.onto(&d, &subject.0);
    let count = out.relight.lights.len();
    s.set_develop(id, out, &format!("Light Preset: {}", preset.name))?;
    s.relight_select(id, 0);
    Ok(json!({"preset": {"id": preset.id, "name": preset.name}, "lights": count, "subject": subject_json(subject)}))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "relight.savePreset";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let name = name_of(s, str_param(p, "name"), None, c)?;
    if s.relight_presets.len() >= MAX_USER {
        return Err(bad(c, format!("at most {MAX_USER} light presets of your own: delete one first")));
    }
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    if !d.relight.is_on() {
        return Err(bad(c, "the photo has no light turned on: raise a light's Amount first"));
    }
    let subject = s.relight_subject(id);
    // (an id no preset has, kept stable when others are deleted)
    let next = s.relight_presets.iter().filter_map(|q| q.id.strip_prefix("user-").and_then(|n| n.parse::<u64>().ok())).max().map_or(1, |n| n + 1);
    let preset = LightPreset::capture(&format!("user-{next}"), &name, &d.relight, &subject.0);
    let out = json!({"id": preset.id, "name": preset.name, "subject": subject_json(subject)});
    s.relight_presets.push(preset);
    Ok(out)
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
            "{id?: photo} → {presets: [{id, name, builtin, lights, kinds, darken}], subject: {x, y, w, h, source: mask|face|centre}} — the light presets (the built-in ones Rembrandt, Butterfly, Split, Rim + Fill, Golden-Hour Window, Neon Two-Tone and Candle, then your own) and where the photo's subject is (a person or subject mask, else its faces, else its centre: the box the preset's lights are placed round)",
            always,
            presets
        ),
        cmd!(
            "relight.applyPreset",
            "Apply Light Preset",
            [],
            None,
            "{preset: a preset's id or name} → {preset: {id, name}, lights, subject} — replace the active photo's lights (and Darken Surroundings) with the preset's, placed round the photo's subject, and turn the Relight section on; one undo step. The photo's other settings are not touched",
            has_active,
            apply
        ),
        cmd!(
            "relight.savePreset",
            "Save Lights as Preset",
            [],
            None,
            "{name: up to 40 characters, not another preset's} → {id, name, subject} — save the active photo's lights (and Darken Surroundings) as a preset of your own, kept relative to the photo's subject so it fits any photo; an error while no light is turned on",
            has_active,
            save
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
