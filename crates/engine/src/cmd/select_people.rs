//! Fork feature (`fork/select-people.md`): Select People masks.
//!
//! - `selectPeople.people {id?, wait?}`: the photo's people (starts finding them when they are not known).
//! - `selectPeople.add {parts, face?, name?}`: a new mask of those parts of one person or of everyone.
//! - `selectPeople.set {id?, component?, parts?, face?}`: change which parts and whose a Select People mask selects.
//! - `selectPeople.pick {parts, face?, how?}`: what a click in the picker does: one mask per part pick, each with its
//!   own edits (select the mask that already has those parts, else a new one).

use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use serde_json::{Value, json};

use super::masks::{find, masks_edit};
use super::{CommandSpec, bad, cmd, has_active};
use crate::select_people::grid::{self, Selection};
use crate::select_people::parts::{picks_of, summary};
use crate::select_people::{Parsed, Pick, current, face_key, mask_of, person_name};
use crate::{Result, Session};

/// The `parts` parameter: ids of the parts list (at least one).
fn picks_param(p: &Value, c: &str) -> Result<Option<Vec<Pick>>> {
    let Some(v) = p.get("parts").filter(|v| !v.is_null()) else { return Ok(None) };
    let ids = v.as_array().ok_or_else(|| bad(c, "`parts` must be a list of part ids"))?;
    let mut picks = Vec::new();
    for id in ids {
        let id = id.as_str().ok_or_else(|| bad(c, "`parts` must be a list of part ids"))?;
        picks.push(Pick::from_id(id).ok_or_else(|| bad(c, format!("unknown part `{id}` ({})", ids_list())))?);
    }
    picks.sort_unstable();
    picks.dedup();
    if picks.is_empty() {
        return Err(bad(c, "`parts` needs at least one part"));
    }
    Ok(Some(picks))
}

fn ids_list() -> String {
    Pick::ALL.iter().map(|p| p.id()).collect::<Vec<_>>().join("|")
}

/// The `face` parameter: absent, null or "all" → everyone (`Some(None)`); an index → that person. `None`: absent.
fn face_param(p: &Value, people: usize, c: &str) -> Result<Option<Option<usize>>> {
    match p.get("face") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) if s == "all" => Ok(Some(None)),
        Some(v) => {
            let k = v.as_u64().and_then(|k| usize::try_from(k).ok()).ok_or_else(|| bad(c, "`face` must be a person's index or \"all\""))?;
            if k >= people {
                return Err(bad(c, format!("the photo has {people} people (face is counted from 0)")));
            }
            Ok(Some(Some(k)))
        }
    }
}

/// The photo's people, found now when they are not known (waiting for the parse).
fn people(s: &mut Session, c: &str) -> Result<std::sync::Arc<Parsed>> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    s.people_now(id).map_err(|e| bad(c, e))
}

/// The shape of `picks` of person `person` (everyone: `None`) of `parsed`, drawn on the frame of the active photo.
pub(crate) fn shape(s: &Session, parsed: &Parsed, person: Option<usize>, picks: &[Pick], c: &str) -> Result<MaskShape> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let orient = s.develop_of(id).map(|d| d.orientation).unwrap_or_default();
    let sel = Selection::of(person, picks);
    let Some((seg, detail)) = grid::build(&parsed.parsing, &sel, orient) else {
        let what = summary(picks).to_lowercase();
        return Err(bad(
            c,
            match person.and_then(|k| parsed.parsing.people.get(k).map(|q| person_name(q, k))) {
                Some(who) => format!("found no {what} on {who}"),
                None => format!("found no {what} in the photo"),
            },
        ));
    };
    Ok(MaskShape::People {
        person: person.and_then(|k| u32::try_from(k).ok()).unwrap_or(0),
        parts: picks.iter().map(|p| p.id().to_string()).collect(),
        face: person.and_then(|k| parsed.parsing.people.get(k)).map(face_key),
        seg: Some(seg),
        detail,
        edge: 0.0,
    })
}

/// The generated name of a mask of `picks` among `masks` ("Hair Mask 1", "Hair + Lips Mask 2").
fn name_for(masks: &[Mask], picks: &[Pick]) -> String {
    let label = summary(picks);
    let name = |n: u32| format!("{label} Mask {n}");
    let n = (1..).find(|n| !masks.iter().any(|m| m.name == name(*n))).unwrap_or(1);
    name(n)
}

/// Whether `name` is one `name_for` made for `picks` (a name the user gave is not).
fn generated_name(name: &str, picks: &[Pick]) -> bool {
    name.strip_prefix(&format!("{} Mask ", summary(picks))).is_some_and(|n| n.parse::<u32>().is_ok())
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "selectPeople.add";
    let picks = picks_param(p, C)?.unwrap_or_else(|| vec![Pick::EntirePerson]);
    let parsed = people(s, C)?;
    let person = face_param(p, parsed.parsing.people.len(), C)?.flatten();
    let shape = shape(s, &parsed, person, &picks, C)?;
    let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
    let next = s.develop_of(id).map(|d| d.next_mask_id()).unwrap_or(1);
    let adjust = super::mask_effects::new_mask_adjust(s, "people");
    let name = p.get("name").and_then(Value::as_str).map(str::to_string);
    let mut out = masks_edit(s, C, "Add Mask", |masks, active| {
        let name = name.unwrap_or_else(|| name_for(masks, &picks));
        masks.push(Mask {
            id: next,
            name,
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
            adjust,
            ..Default::default()
        });
        *active = Some(next);
        Ok(())
    })?;
    out["mask"] = json!(next);
    Ok(out)
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "selectPeople.set";
    let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
    let mid = match p.get("id").filter(|v| !v.is_null()) {
        Some(v) => super::masks::parse_mask_id(v, C)?,
        None => s.active_mask.ok_or_else(|| bad(C, "no mask is selected"))?,
    };
    let d = s.develop_of(id).ok_or_else(|| bad(C, "no active photo"))?;
    let mask = d.masks.iter().find(|m| m.id == mid).ok_or_else(|| bad(C, format!("no mask {mid}")))?;
    let comp = match super::masks::optional_index(p, "component", C)? {
        Some(k) => k,
        None => mask.components.iter().position(|c| matches!(c.shape, MaskShape::People { seg: Some(_), .. })).unwrap_or(0),
    };
    let old = mask.components.get(comp).map(|c| c.shape.clone()).ok_or_else(|| bad(C, format!("mask {mid} has no part {comp}")))?;
    let old_name = mask.name.clone();
    let parsed = people(s, C)?;
    let (old_picks, old_person, everyone) = current(&old, Some(&parsed)).ok_or_else(|| bad(C, "that part is not a Select People part"))?;
    let picks = picks_param(p, C)?.unwrap_or_else(|| old_picks.clone());
    let person = match face_param(p, parsed.parsing.people.len(), C)? {
        Some(choice) => choice,
        // (a face no longer found is everyone's only when it was)
        None if everyone => None,
        None => Some(old_person.ok_or_else(|| bad(C, "the mask's face is not in the photo any more: pick a face"))?),
    };
    let MaskShape::People { edge, seg: old_seg, .. } = old else { return Err(bad(C, "that part is not a Select People part")) };
    // the same parts of the same face from the same parse: nothing to draw again (no undo step, no grid in the log)
    let same_face = if everyone { person.is_none() } else { person.is_some() && person == old_person };
    let same_parse = old_seg.and_then(|g| g.model) == Some(grid::model_id(&parsed.parsing, &Selection::of(person, &picks)));
    if picks == old_picks && same_face && same_parse {
        return Ok(json!({"activeMask": s.active_mask, "unchanged": true}));
    }
    let mut shape = shape(s, &parsed, person, &picks, C)?;
    if let MaskShape::People { edge: e, .. } = &mut shape {
        *e = edge;
    }
    masks_edit(s, C, "Select People", |masks, _| {
        let i = find(masks, mid, C)?;
        // (a name the user gave stays)
        let generated = generated_name(&old_name, &old_picks);
        let name = generated.then(|| name_for(masks, &picks));
        let m = masks.get_mut(i).ok_or_else(|| bad(C, format!("no mask {mid}")))?;
        if let Some(c) = m.components.get_mut(comp) {
            c.shape = shape;
        }
        if let Some(name) = name {
            m.name = name;
        }
        Ok(())
    })
}

/// Whether `m` is a picker's mask nobody has edited: visible, one Select People part (the default mode, not
/// renamed, not inverted) and nothing else, a generated name, the effects it started with, no Edge, Refine Edges,
/// Decontaminate or Invert.
fn untouched(s: &Session, m: &Mask) -> bool {
    let [c] = m.components.as_slice() else { return false };
    let MaskShape::People { parts, seg: Some(_), edge, .. } = &c.shape else { return false };
    m.visible
        && c.name.is_none()
        && c.op == MaskOp::Add
        && generated_name(&m.name, &picks_of(parts))
        && *edge == 0.0
        && m.refine == 0.0
        && m.decontaminate == 0.0
        && !m.invert
        && !c.invert
        && m.adjust == super::mask_effects::new_mask_adjust(s, "people")
}

/// A click in the people picker. `how`: `part` (default; a plain click on a part): select the mask that has exactly
/// these parts of this face (an empty mask the picker made and the picker's selection leaves is deleted with the
/// same undo step), else change the selected mask when nobody has edited it, else make a new one, so each part
/// pick keeps its own edits; `add` (⌘-click: `parts` is the whole new list): change the selected mask's parts, unless
/// another mask has exactly those parts (refused: "Hair already has a mask"); `face` (a face chip): change whose the
/// selected mask is, unless a mask of those parts and that face exists, then select it. Without a selected Select
/// People mask, `add` and `face` make a new mask. `face` null with `add` keeps the selected mask's face.
fn pick(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "selectPeople.pick";
    let how = match p.get("how") {
        None | Some(Value::Null) => "part",
        Some(Value::String(h)) if matches!(h.as_str(), "part" | "add" | "face") => h.as_str(),
        Some(_) => return Err(bad(C, "`how` must be \"part\", \"add\" or \"face\"")),
    };
    if how == "add" && p.get("parts").is_none_or(Value::is_null) {
        return Err(bad(C, "`add` needs `parts`: the whole new list"));
    }
    let id = s.active().ok_or_else(|| bad(C, "no active photo"))?;
    let parsed = people(s, C)?;
    let picks = picks_param(p, C)?.unwrap_or_else(|| vec![Pick::EntirePerson]);
    let keep_face = how == "add" && p.get("face").is_none_or(Value::is_null);
    let face = if keep_face { None } else { face_param(p, parsed.parsing.people.len(), C)? };
    let d = s.develop_of(id).ok_or_else(|| bad(C, "no active photo"))?;
    let selected = s
        .active_mask
        .and_then(|mid| d.masks.iter().find(|m| m.id == mid))
        .filter(|m| m.components.iter().any(|c| matches!(c.shape, MaskShape::People { seg: Some(_), .. })));
    // whose: given, else (for `add`) the selected mask's own
    let person = match (face, selected) {
        (Some(f), _) => f,
        (None, Some(m)) if how == "add" => m.components.iter().find_map(|c| current(&c.shape, Some(&parsed))).and_then(|(_, who, _)| who),
        (None, _) => None,
    };
    if how == "add" && mask_of(d.as_ref(), &parsed, &picks, person, selected.map(|m| m.id)).is_some() {
        return Err(bad(C, format!("{} already has a mask", summary(&picks))));
    }
    if how != "add"
        && let Some(mid) = mask_of(d.as_ref(), &parsed, &picks, person, None)
    {
        // the picker's own empty mask is not left behind (the same undo step); selecting a mask is not an edit
        let empty = selected.filter(|m| m.id != mid && untouched(s, m)).map(|m| m.id);
        if let Some(empty) = empty {
            masks_edit(s, C, "Select People", |masks, active| {
                let i = find(masks, empty, C)?;
                masks.remove(i);
                *active = Some(mid);
                Ok(())
            })?;
        } else if s.active_mask != Some(mid) {
            s.active_mask = Some(mid);
            s.active_component = None;
        }
        return Ok(json!({"activeMask": mid, "mask": mid, "selected": true}));
    }
    let retarget = match (how, selected) {
        ("part", Some(m)) => untouched(s, m).then_some(m.id),
        ("part", None) => None,
        (_, m) => m.map(|m| m.id),
    };
    let ids: Vec<&str> = picks.iter().map(|k| k.id()).collect();
    let mut set_params = json!({"id": retarget, "parts": ids});
    let mut add_params = json!({"parts": ids});
    if let Some(f) = face.or((how != "add").then_some(None)) {
        let v = f.map_or(json!("all"), |k| json!(k));
        set_params["face"] = v.clone();
        add_params["face"] = v;
    }
    match retarget {
        Some(mid) => set(s, &set_params).map(|mut o| {
            o["mask"] = json!(mid);
            o
        }),
        None => add(s, &add_params).map(|mut o| {
            o["created"] = json!(true);
            o
        }),
    }
}

fn people_json(parsed: &Parsed) -> Value {
    let people: Vec<Value> = parsed
        .parsing
        .people
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let rect = |r: &lightcraft_parsing::NormRect| json!([r.x0, r.y0, r.x1, r.y1]);
            let parts: Vec<&str> = Pick::ALL
                .iter()
                .filter(|pk| pk.parts().iter().any(|part| q.parts.contains_key(part)))
                .map(|pk| pk.id())
                .collect();
            json!({"index": i, "name": person_name(q, i), "personId": q.person_id, "face": q.face.as_ref().map(rect), "bounds": q.bounds.as_ref().map(rect), "parts": parts})
        })
        .collect();
    json!(people)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "selectPeople.people";
    let id = *s.targets(p)?.first().ok_or_else(|| bad(C, "no photo"))?;
    if p.get("wait").and_then(Value::as_bool).unwrap_or(false) {
        return match s.people_now(id) {
            Ok(parsed) => Ok(json!({"id": id.0, "state": "ready", "people": people_json(&parsed)})),
            Err(e) => Ok(json!({"id": id.0, "state": "failed", "error": e})),
        };
    }
    s.people_find(id, false);
    let st = s.people_of(id);
    Ok(match (st.ready, st.error) {
        (Some(parsed), _) => json!({"id": id.0, "state": "ready", "people": people_json(&parsed)}),
        (None, Some(e)) => json!({"id": id.0, "state": "failed", "error": e}),
        (None, None) => json!({"id": id.0, "state": "running"}),
    })
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "selectPeople.people",
            "Select People: Find People",
            [],
            None,
            "{id?, wait?: bool (default false)} — the photo's people for Select People masks: found once per photo (the photo is parsed into people's parts, from the cache when it was before) on a worker thread with a row in the activity stack; with `wait` the call returns when they are found → {id, state: ready|running|failed, people: [{index, name, personId, face: [x0,y0,x1,y1]|null, bounds, parts: [part ids found]}], error}",
            has_active,
            list
        ),
        cmd!(
            "selectPeople.add",
            "Select People: New Mask",
            [],
            None,
            "{parts?: [entirePerson|entireFace|facialSkin|bodySkin|hair|eyebrows|eyeWhites|iris|lips|teeth|clothes|clothesTop|clothesBottom|dress|shoes|hat|bag|background] (default [\"entirePerson\"]; several make one mask), face?: person index|\"all\" (default all), name?} — a new mask of those parts of one person or of everyone, drawn from the photo's people parse (waits for it when it is not known) and stored with the mask → {activeMask, mask}",
            has_active,
            add
        ),
        cmd!(
            "selectPeople.pick",
            "Select People: Pick Parts",
            [],
            None,
            "{parts?: [part ids as in selectPeople.add] (default [\"entirePerson\"]), face?: person index|\"all\" (default all; for `add`, the selected mask's), how?: part|add|face (default part)} — what a click in the picker does, one mask per part pick so each has its own edits. `part`: select the mask that has exactly these parts of this face; else change the selected Select People mask when it was made by the picker and has no edits yet (default effects, no Edge, Refine Edges, Decontaminate or Invert); else make a new mask and select it (one undo step). `add` (⌘-click; `parts` is the whole new list): change the selected mask's parts. `face` (a face chip): change whose the selected mask is, unless a mask of those parts and that face exists, then select it. `add` and `face` without a selected Select People mask make a new one → {activeMask, mask, selected?: true (an existing mask was selected), created?: true, unchanged?}",
            has_active,
            pick
        ),
        cmd!(
            "selectPeople.set",
            "Select People: Change Parts",
            [],
            None,
            "{id?: mask id (default the selected mask), component?: part index (default its first Select People part), parts?: [entirePerson|entireFace|facialSkin|bodySkin|hair|eyebrows|eyeWhites|iris|lips|teeth|clothes|clothesTop|clothesBottom|dress|shoes|hat|bag|background], face?: person index|\"all\"} — change which parts, and whose, a Select People mask selects (what is not given stays; the same parts of the same face change nothing) → {activeMask, unchanged?}",
            has_active,
            set
        ),
    ]
}
