//! `mask.presets`, `mask.applyPreset`, `mask.savePreset`, `mask.renamePreset`, `mask.deletePreset` (fork, issue #329,
//! `fork/mask-presets.md`): a mask kept as a recipe and added to other photos. A preset is named by `preset` (its id,
//! else its name); `id` of `mask.savePreset` is a mask of the active photo.

use lightcraft_catalog::PhotoId;
use lightcraft_develop::MaskShape;
use lightcraft_develop::mask_presets::{MAX_NAME, MAX_USER, MaskPreset, clean_name};
use serde_json::{Value, json};

use super::masks::{clamp_local, mask_id, masks_edit};
use super::{CommandSpec, always, bad, cmd, has_active, str_param};
use crate::segment::adaptive_jobs::Ask;
use crate::{Result, Session};

fn named(s: &Session, p: &Value, c: &str) -> Result<MaskPreset> {
    let key = str_param(p, "preset").ok_or_else(|| bad(c, "missing `preset`: a preset's id or name (mask.presets lists them)"))?;
    s.mask_preset(key).ok_or_else(|| bad(c, format!("no mask preset {key:?}: mask.presets lists them")))
}

/// `raw` as a new preset's name: cleaned, at most [`MAX_NAME`] characters, and not another preset's (`except`: the one being renamed).
fn name_of(s: &Session, raw: Option<&str>, except: Option<&str>, c: &str) -> Result<String> {
    let name = raw.and_then(clean_name).ok_or_else(|| bad(c, "give the preset a name"))?;
    if name.chars().count() > MAX_NAME {
        return Err(bad(c, format!("a name is at most {MAX_NAME} characters")));
    }
    let others = s.mask_presets.iter().filter(|p| Some(p.id.as_str()) != except).map(|p| p.name.as_str());
    crate::names::check_name(&name, others).map_err(|e| bad(c, e.to_string()))
}

fn summary(p: &MaskPreset) -> Value {
    let (m, a) = (&p.mask.adjust, lightcraft_develop::LocalAdjustments::default());
    json!({"id": p.id, "name": p.name, "kinds": p.kinds(), "ai": p.ai_parts(), "effect": *m != a, "invert": p.mask.invert})
}

fn presets(s: &mut Session, _p: &Value) -> Result<Value> {
    let list: Vec<Value> = s.mask_presets.iter().map(summary).collect();
    Ok(json!({"presets": list}))
}

/// Why a model can't find AI part `shape` on photo `id` on this computer; `None` when it can (or the part is not an AI one).
fn why_not_found(s: &Session, id: PhotoId, shape: &MaskShape) -> Option<String> {
    if !shape.is_detected() {
        return None;
    }
    match shape {
        MaskShape::People { .. } => (!s.parsing.ready()).then(|| "the people models are not installed on this computer".to_string()),
        _ if s.adaptive.has_finder() => None,
        _ => match s.adaptive_ask(id, shape) {
            Ask::Missing(why) => Some(why),
            _ => None,
        },
    }
}

/// A selection that selects nothing anywhere (what a search that found nothing leaves).
fn selects_nothing(seg: &lightcraft_develop::SegMask) -> bool {
    seg.logits().is_none_or(|l| l.iter().all(|v| *v < 0.0))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.applyPreset";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let preset = named(s, p, c)?;
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    let next = d.next_mask_id();
    let asked = str_param(p, "name").and_then(clean_name);
    if asked.as_ref().is_some_and(|n| n.chars().count() > MAX_NAME) {
        return Err(bad(c, format!("a name is at most {MAX_NAME} characters")));
    }
    // (numbered when taken: "Name 2" may pass the cap by its number, which is fine)
    let name = crate::mask_presets::free_name(&d.masks, asked.as_deref().unwrap_or(&preset.name));
    let mut mask =
        preset.instance(next, &name).filter(|_| next != 0).ok_or_else(|| bad(c, "this preset has no part, or the photo has no free mask number"))?;
    // (a file may hold any number: the sliders' own ranges, as for every other mask)
    mask.adjust = clamp_local(mask.adjust);
    mask.refine = if mask.refine.is_finite() { mask.refine.clamp(0.0, 100.0) } else { 0.0 };
    mask.decontaminate = if mask.decontaminate.is_finite() { mask.decontaminate.clamp(0.0, 100.0) } else { 0.0 };
    let mut missing: Vec<String> = Vec::new();
    for part in &mask.components {
        if let Some(why) = why_not_found(s, id, &part.shape)
            && !missing.contains(&why)
        {
            missing.push(why);
        }
    }
    let kinds = preset.kinds();
    masks_edit(s, c, &format!("Mask Preset: {}", preset.name), |masks, active| {
        masks.push(mask);
        *active = Some(next);
        Ok(())
    })?;
    // the AI parts are found on this photo: now in the CLI, MCP and tests; queued behind the top-bar ring in the app
    let queued = s.adaptive_find(&[id]);
    let mut nothing: Vec<String> = Vec::new();
    if let Some(m) = s.develop_of(id).and_then(|d| d.masks.iter().find(|m| m.id == next).cloned()) {
        for (k, part) in m.components.iter().enumerate() {
            if part.shape.is_detected() && !part.shape.is_waiting() && part.shape.seg().is_some_and(selects_nothing) {
                nothing.push(kinds.get(k).copied().unwrap_or_default().to_string());
            }
        }
    }
    Ok(json!({
        "applied": true,
        "mask": next,
        "name": name,
        "preset": {"id": preset.id, "name": preset.name},
        "ai": preset.ai_parts(),
        "finding": queued > 0,
        "missing": missing,
        "foundNothing": nothing,
    }))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.savePreset";
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let name = name_of(s, str_param(p, "name"), None, c)?;
    if s.mask_presets.len() >= MAX_USER {
        return Err(bad(c, format!("at most {MAX_USER} mask presets: delete one first")));
    }
    let d = s.develop_of(id).ok_or_else(|| bad(c, "no photo"))?;
    let mid = mask_id(p, s.active_mask, c)?;
    let mut mask = d.masks.iter().find(|m| m.id == mid).cloned().ok_or_else(|| bad(c, format!("no mask {mid}")))?;
    if super::mask_multi::is_empty(&mask) {
        return Err(bad(c, "the mask selects nothing yet: paint, place or pick something first"));
    }
    // (an old mask drawn on the corrected frame goes onto the photo first, so a preset is always on the photo)
    if !mask.on_photo {
        let mut moved = (*d).clone();
        s.attach_masks(id, &d, &mut moved);
        if let Some(m) = moved.masks.into_iter().find(|m| m.id == mid) {
            mask = m;
        }
    }
    // (an id no preset has, kept stable when others are deleted; one that would overflow is an error)
    let next = s
        .mask_presets
        .iter()
        .filter_map(|q| q.id.strip_prefix("user-").and_then(|n| n.parse::<u64>().ok()))
        .max()
        .map_or(Some(1), |n| n.checked_add(1))
        .ok_or_else(|| bad(c, "no free id for a new preset: delete the presets that were written by hand"))?;
    let preset = MaskPreset::capture(&format!("user-{next}"), &name, &mask);
    let out = summary(&preset);
    s.mask_presets.push(preset);
    Ok(out)
}

fn user_index(s: &Session, p: &Value, c: &str) -> Result<usize> {
    let preset = named(s, p, c)?;
    s.mask_presets.iter().position(|q| q.id == preset.id).ok_or_else(|| bad(c, "no such preset"))
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.renamePreset";
    let i = user_index(s, p, c)?;
    let id = s.mask_presets[i].id.clone();
    let name = name_of(s, str_param(p, "name"), Some(&id), c)?;
    s.mask_presets[i].name = name.clone();
    Ok(json!({"id": id, "name": name}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.deletePreset";
    let i = user_index(s, p, c)?;
    let gone = s.mask_presets.remove(i);
    Ok(json!({"id": gone.id, "name": gone.name}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "mask.presets",
            "Mask Presets",
            [],
            None,
            "{} → {presets: [{id, name, kinds: [kind of each part], ai: parts a model finds again on each photo, effect: has effect sliders set, invert}]} — your mask presets (saved with mask.savePreset, kept in the library)",
            always,
            presets
        ),
        cmd!(
            "mask.applyPreset",
            "Apply Mask Preset",
            [],
            None,
            "{preset: a preset's id or name, name?: the new mask's name (default: the preset's, numbered when taken)} → {applied, mask: id, name, preset: {id, name}, ai, finding, missing: [why a model can't find a part on this computer], foundNothing: [kinds a model found nothing for; only when found before this returns, so not in the app]} — add the preset's mask to the active photo and select it; one undo step. Its AI parts (Subject, Sky, Background, Object, Describe, Select People) are found again on this photo (in the app in the background, `finding: true`; in the CLI and tests before this returns); a part whose model is missing, or that finds nothing, selects nothing: `missing` names the first at once, `foundNothing` the second in the CLI and tests (in the app the \"Finding AI masks\" row of the top-bar ring lists the photo with why); Update AI Masks finds it again later. Brush strokes and gradients land at the same place relative to the photo, whatever its size or orientation",
            has_active,
            apply
        ),
        cmd!(
            "mask.savePreset",
            "Save Mask as Preset",
            [],
            None,
            "{id?: mask (the selected one), name: up to 40 characters, not another preset's} → {id, name, kinds, ai, effect, invert} — save the mask's parts (kind, parameters, mode, Invert), effect sliders, Refine Edges and Decontaminate as a preset of your own: AI parts as what they find (never the selection found on this photo), brush strokes and gradients relative to the photo; an error for a mask that selects nothing yet",
            has_active,
            save
        ),
        cmd!("mask.renamePreset", "Rename Mask Preset", [], None, "{preset: id or name, name} → {id, name} — rename it", always, rename),
        cmd!(
            "mask.deletePreset",
            "Delete Mask Preset",
            [],
            None,
            "{preset: id or name} → {id, name} — delete it; masks made from it keep their settings",
            always,
            delete
        ),
    ]
}
