//! Fork feature (`fork/learn-my-edits.md`): the `learn.*` commands of Learn My Edits.
//!
//! Jobs (create, add examples, train, apply) run on a thread with a row in the activity stack; `wait: true` waits for
//! the job and returns its result (agents and tests), else the app's `learn.pump` takes it in.

use lightcraft_learn::{Group, Profile};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection, str_param};
use crate::Session;
use crate::learn::Download;
use crate::learn::jobs::{self, Item};

fn profile_json(id: &str, p: &Profile) -> Value {
    let report = p.report.as_ref().map(|r| {
        json!({
            "examples": r.examples,
            "beatsDefaults": r.beats_defaults,
            "beatsPreset": r.beats_preset,
            "errorVsDefaults": r.ratio,
            "groups": r.groups.iter().map(|g| json!({
                "group": g.group.id(),
                "kept": g.kept,
                "errorVsDefaults": g.ratio,
                "presetErrorVsDefaults": g.preset_ratio,
                "sliders": g.sliders.iter().map(|s| json!({"id": s.id, "error": s.error, "defaultError": s.default_error, "learned": s.learned(g.kept)})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    });
    json!({
        "id": id,
        "name": p.name,
        "kind": p.kind,
        "groups": p.groups.iter().map(|g| g.id()).collect::<Vec<_>>(),
        "learned": p.learned_groups().iter().map(|g| g.id()).collect::<Vec<_>>(),
        "examples": p.examples.len(),
        "trained": p.model.is_some(),
        "report": report,
        "problem": p.problem,
    })
}

fn status(s: &Session) -> Value {
    let (state, bytes, total, error) = match s.learn.download_state() {
        Download::Idle => ("idle", 0, 0, None),
        Download::Running { bytes, total } => ("running", bytes, total, None),
        Download::Done => ("done", lightcraft_learn::ENCODER_BYTES, lightcraft_learn::ENCODER_BYTES, None),
        Download::Cancelled => ("cancelled", 0, 0, None),
        Download::Failed(e) => ("failed", 0, 0, Some(e)),
    };
    let profiles: Vec<Value> = s
        .learn
        .list()
        .iter()
        .map(|l| match &l.profile {
            Ok(p) => profile_json(&l.id, p),
            Err(e) => json!({"id": l.id, "error": e}),
        })
        .collect();
    json!({
        "model": {
            "installed": s.learn.installed() || s.learn.statistics_only(),
            "file": s.learn.model_path().map(|p| p.display().to_string()),
            "bytes": lightcraft_learn::ENCODER_BYTES,
            "download": {"state": state, "bytes": bytes, "total": total, "error": error},
            "runtime": s.learn.runtime().id(),
            "coremlInstalled": s.learn.coreml_installed(),
            "backend": s.learn.backend(),
        },
        "profilesFolder": s.learn.profiles_dir().map(|p| p.display().to_string()),
        "job": s.learn.running().map(|(id, name, label)| json!({"profile": id, "name": name, "label": label})),
        "message": s.learn.message(),
        "profiles": profiles,
    })
}

fn groups_param(c: &str, p: &Value) -> crate::Result<Vec<Group>> {
    match p.get("groups") {
        None | Some(Value::Null) => Ok(Group::ALL.to_vec()),
        Some(Value::Array(a)) => {
            let g: Vec<Group> = a
                .iter()
                .map(|v| v.as_str().and_then(Group::from_id).ok_or_else(|| bad(c, format!("unknown group {v}"))))
                .collect::<crate::Result<_>>()?;
            if g.is_empty() { Err(bad(c, "name at least one group")) } else { Ok(g) }
        }
        Some(_) => Err(bad(c, "`groups` must be a list of group ids")),
    }
}

fn profile_param(s: &Session, c: &str, p: &Value) -> crate::Result<(String, Profile)> {
    let id = str_param(p, "profile").ok_or_else(|| bad(c, "missing `profile` (a profile id from learn.status)"))?;
    let profile = s.learn.get(id).map_err(|e| bad(c, e))?;
    Ok((id.to_string(), (*profile).clone()))
}

/// The photos a call names (`ids`, `id`, else the selection), prepared for a job. Photos that can't be used are
/// skipped and counted; none left is an error.
fn items(s: &mut Session, c: &str, p: &Value, example: Option<&Profile>) -> crate::Result<(Vec<Item>, Vec<String>)> {
    let ids = s.targets(p)?;
    if ids.is_empty() {
        return Err(bad(c, "no photos"));
    }
    if ids.len() > lightcraft_learn::train::MAX_EXAMPLES * 5 {
        return Err(bad(c, format!("at most {} photos at once", lightcraft_learn::train::MAX_EXAMPLES * 5)));
    }
    let (mut ok, mut skipped) = (Vec::new(), Vec::new());
    for id in ids {
        match jobs::prepare(s, id, example) {
            Ok(i) => ok.push(i),
            Err(e) => skipped.push(e),
        }
    }
    if ok.is_empty() {
        return Err(bad(c, skipped.first().cloned().unwrap_or_else(|| "no photos".into())));
    }
    Ok((ok, skipped))
}

/// The answer of a job command: started, or (with `wait`) its result.
fn finish(s: &mut Session, c: &str, p: &Value, mut out: Value) -> crate::Result<Value> {
    if p.get("wait").and_then(Value::as_bool) != Some(true) {
        out["started"] = json!(true);
        return Ok(out);
    }
    let done = jobs::pump(s, true);
    if let Some(e) = done.error {
        return Err(bad(c, e));
    }
    out["applied"] = json!(done.applied);
    out["message"] = json!(done.message);
    if let Some(id) = done.trained
        && let Ok(profile) = s.learn.get(&id)
    {
        out["profile"] = profile_json(&id, &profile);
    }
    Ok(out)
}

fn needs_model(s: &Session, c: &str) -> crate::Result<()> {
    if s.learn.installed() || s.learn.statistics_only() {
        return Ok(());
    }
    Err(bad(
        c,
        format!(
            "the image model Learn My Edits uses isn't installed: learn.downloadModel downloads it ({} MB, ask the user first)",
            (lightcraft_learn::ENCODER_BYTES + 500_000) / 1_000_000
        ),
    ))
}

/// Take in a job that has ended, before anything else: without the app's frame loop (MCP, the CLI) nothing else would,
/// and the job's result would never be written and every later call would find Learn My Edits busy.
fn take_in(s: &mut Session) {
    jobs::pump(s, false);
}

fn create(s: &mut Session, p: &Value) -> crate::Result<Value> {
    take_in(s);
    const C: &str = "learn.create";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?;
    let kind = str_param(p, "kind").unwrap_or("");
    let groups = groups_param(C, p)?;
    if let Some((_, _, label)) = s.learn.running() {
        return Err(bad(C, format!("Learn My Edits is busy: {label}")));
    }
    let id = s.learn.new_id(name).map_err(|e| bad(C, e))?;
    let profile = Profile::new(name, kind, s.learn.layout(), &groups);
    let from_photos = p.get("ids").is_some_and(|v| !v.is_null()) || p.get("id").is_some_and(|v| !v.is_null()) || !s.selected().is_empty();
    if !from_photos || p.get("empty").and_then(Value::as_bool) == Some(true) {
        s.learn.save(&id, &profile).map_err(|e| bad(C, e))?;
        return Ok(json!({"profile": profile_json(&id, &profile)}));
    }
    needs_model(s, C)?;
    let (items, skipped) = items(s, C, p, Some(&profile))?;
    s.learn.save(&id, &profile).map_err(|e| bad(C, e))?;
    let n = items.len();
    s.learn.start_training(&id, profile, items).map_err(|e| bad(C, e))?;
    finish(s, C, p, json!({"id": id, "photos": n, "skipped": skipped}))
}

fn add_examples(s: &mut Session, p: &Value) -> crate::Result<Value> {
    take_in(s);
    const C: &str = "learn.addExamples";
    let (id, profile) = profile_param(s, C, p)?;
    s.learn.check_idle(&id).map_err(|e| bad(C, e))?;
    needs_model(s, C)?;
    let (items, skipped) = items(s, C, p, Some(&profile))?;
    let n = items.len();
    s.learn.start_training(&id, profile, items).map_err(|e| bad(C, e))?;
    finish(s, C, p, json!({"id": id, "photos": n, "skipped": skipped}))
}

fn apply(s: &mut Session, p: &Value) -> crate::Result<Value> {
    take_in(s);
    const C: &str = "learn.apply";
    let (id, profile) = profile_param(s, C, p)?;
    if profile.model.is_none() {
        return Err(bad(C, profile.predict(&[]).err().map(|e| e.to_string()).unwrap_or_else(|| "not trained".into())));
    }
    if profile.features.encoder.is_some() {
        needs_model(s, C)?;
    }
    let (items, skipped) = items(s, C, p, None)?;
    let n = items.len();
    s.learn.start_apply(&id, std::sync::Arc::new(profile), items).map_err(|e| bad(C, e))?;
    finish(s, C, p, json!({"id": id, "photos": n, "skipped": skipped}))
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "learn.status",
            "Learn My Edits Status",
            [],
            None,
            "{} → {model: {installed, file, bytes, download: {state, bytes, total, error}, runtime, coremlInstalled, backend}, profilesFolder, job: {profile, name, label}|null, message, profiles: [{id, name, kind, groups, learned, examples, trained, report: {examples, beatsDefaults, beatsPreset, errorVsDefaults, groups: [{group, kept, errorVsDefaults, presetErrorVsDefaults, sliders: [{id, error, defaultError}]}]}|null, problem}]}",
            always,
            |s, _| {
                take_in(s);
                Ok(status(s))
            }
        ),
        cmd!(
            "learn.downloadModel",
            "Download Learn My Edits Model",
            [],
            None,
            "{confirm: true} — downloads the image model (DINOv2-small, 88 MB, from huggingface.co) into the folder shared with PhotoCraft, with a row in the activity stack. Without `confirm: true` it is refused: ask the user first → {started: true, bytes}",
            always,
            |s, p| {
                take_in(s);
                if p.get("confirm").and_then(Value::as_bool) != Some(true) {
                    return Err(bad(
                        "learn.downloadModel",
                        format!(
                            "this downloads {} MB from huggingface.co: ask the user first, then send {{\"confirm\": true}}",
                            (lightcraft_learn::ENCODER_BYTES + 500_000) / 1_000_000
                        ),
                    ));
                }
                s.learn.start_download().map_err(|e| bad("learn.downloadModel", e))?;
                Ok(json!({"started": true, "bytes": lightcraft_learn::ENCODER_BYTES}))
            }
        ),
        cmd!(
            "learn.create",
            "New Learn My Edits Profile",
            [],
            None,
            "{name, kind?, groups?: [whiteBalance|tone|presence|colorMixer|colorGrading|toneCurve], ids?, empty?, wait?} — a new profile trained on the edited photos named (`ids`, else the selection; unedited ones are skipped). With `empty: true` or no photos it starts empty → {id, photos, skipped, started} (with `wait`: {…, message, profile})",
            always,
            create
        ),
        cmd!(
            "learn.addExamples",
            "Add Photos as Learn My Edits Examples",
            [],
            None,
            "{profile, ids?, wait?} — adds the edited photos (`ids`, else the selection) as examples, replacing a photo already there, and trains the profile again",
            has_selection,
            add_examples
        ),
        cmd!(
            "learn.removeExample",
            "Remove Learn My Edits Example",
            [],
            None,
            "{profile, photo} — removes the example named `photo` (as listed in the profile file); train again to use the change",
            always,
            |s, p| {
                take_in(s);
                const C: &str = "learn.removeExample";
                let (id, mut profile) = profile_param(s, C, p)?;
                s.learn.check_idle(&id).map_err(|e| bad(C, e))?;
                let photo = str_param(p, "photo").ok_or_else(|| bad(C, "missing `photo`"))?;
                if !profile.remove_example(photo) {
                    return Err(bad(C, format!("no example `{photo}`")));
                }
                s.learn.save(&id, &profile).map_err(|e| bad(C, e))?;
                Ok(json!({"examples": profile.examples.len()}))
            }
        ),
        cmd!(
            "learn.train",
            "Train Learn My Edits Profile",
            [],
            None,
            "{profile, wait?} — trains the profile on its examples again",
            always,
            |s, p| {
                take_in(s);
                const C: &str = "learn.train";
                let (id, profile) = profile_param(s, C, p)?;
                s.learn.start_training(&id, profile, Vec::new()).map_err(|e| bad(C, e))?;
                finish(s, C, p, json!({"id": id}))
            }
        ),
        cmd!(
            "learn.apply",
            "Apply Learn My Edits Profile",
            [],
            None,
            "{profile, ids?, wait?} — writes the profile's sliders into the photos (`ids`, else the selection): only the groups it learned change; one undo step, one History step per photo named \"Learned: <name>\"",
            has_selection,
            apply
        ),
        cmd!("learn.cancel", "Stop Learn My Edits", [], None, "{} — stops the running job", always, |s, _| {
            take_in(s);
            Ok(json!({"stopped": s.learn.cancel()}))
        }),
        cmd!(
            query "learn.pump",
            "Learn My Edits Results",
            [],
            None,
            "{} — takes in a job that has ended (the app calls it every frame): writes an apply's sliders → {applied, trained, message, error}",
            always,
            |s, _| {
                let r = jobs::pump(s, false);
                Ok(json!({"applied": r.applied, "trained": r.trained, "message": r.message, "error": r.error}))
            }
        ),
        cmd!(
            "learn.update",
            "Edit Learn My Edits Profile",
            [],
            None,
            "{profile, name?, kind?, groups?} — renames it, or changes what it learns (it needs training again)",
            always,
            |s, p| {
                take_in(s);
                const C: &str = "learn.update";
                let (id, mut profile) = profile_param(s, C, p)?;
                s.learn.check_idle(&id).map_err(|e| bad(C, e))?;
                let fresh = Profile::new(
                    str_param(p, "name").unwrap_or(&profile.name),
                    str_param(p, "kind").unwrap_or(&profile.kind),
                    profile.features.clone(),
                    &profile.groups,
                );
                if fresh.name.is_empty() {
                    return Err(bad(C, "the name can't be empty"));
                }
                (profile.name, profile.kind) = (fresh.name, fresh.kind);
                if p.get("groups").is_some_and(|g| !g.is_null()) {
                    let mut groups = groups_param(C, p)?;
                    groups.sort();
                    groups.dedup();
                    if groups != profile.groups {
                        profile.groups = groups;
                        (profile.model, profile.report) = (None, None);
                    }
                }
                s.learn.save(&id, &profile).map_err(|e| bad(C, e))?;
                Ok(json!({"profile": profile_json(&id, &profile)}))
            }
        ),
        cmd!("learn.delete", "Delete Learn My Edits Profile", [], None, "{profile} — deletes the profile's file", always, |s, p| {
            take_in(s);
            const C: &str = "learn.delete";
            let id = str_param(p, "profile").ok_or_else(|| bad(C, "missing `profile`"))?;
            s.learn.get(id).map_err(|e| bad(C, e))?;
            s.learn.delete(id).map_err(|e| bad(C, e))?;
            Ok(json!({"deleted": id}))
        }),
        cmd!(
            "learn.export",
            "Export Learn My Edits Profile",
            [],
            None,
            "{profile, path} — writes the profile to one `.lclearn` file",
            always,
            |s, p| {
                take_in(s);
                const C: &str = "learn.export";
                let (_, profile) = profile_param(s, C, p)?;
                let path = str_param(p, "path").filter(|x| !x.is_empty()).ok_or_else(|| bad(C, "missing `path`"))?;
                profile.write(std::path::Path::new(path)).map_err(|e| bad(C, e.to_string()))?;
                Ok(json!({"path": path}))
            }
        ),
        cmd!(
            "learn.import",
            "Import Learn My Edits Profile",
            [],
            None,
            "{path} — adds the profile in a `.lclearn` file (a copy, under a new id when one has its name)",
            always,
            |s, p| {
                take_in(s);
                const C: &str = "learn.import";
                let path = str_param(p, "path").filter(|x| !x.is_empty()).ok_or_else(|| bad(C, "missing `path`"))?;
                let profile = Profile::read(std::path::Path::new(path)).map_err(|e| bad(C, e.to_string()))?;
                let id = s.learn.new_id(&profile.name).map_err(|e| bad(C, e))?;
                s.learn.save(&id, &profile).map_err(|e| bad(C, e))?;
                Ok(json!({"profile": profile_json(&id, &profile)}))
            }
        ),
    ]
}
