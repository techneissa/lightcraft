//! Several parts of a mask at once (issue #61): `mask.component` with `components: [..]` (or one `component`),
//! Duplicate to New Mask, and `mask.deleteEmpty`. Each command is one undo step.

use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
use serde_json::{Value, json};

use super::masks::{find, mask_id, masks_edit, optional_index};
use super::{CommandSpec, bad, bool_or, cmd, has_active, str_param};
use crate::{Result, Session};

/// The parts a `mask.component` call names, sorted and without repeats: `components` (a non-empty list) or `component`.
fn targets(p: &Value, c: &str) -> Result<Vec<usize>> {
    let one = optional_index(p, "component", c)?;
    let many = p.get("components").filter(|v| !v.is_null());
    let mut ks = match (many, one) {
        (Some(_), Some(_)) => return Err(bad(c, "give `component` or `components`, not both")),
        (None, None) => return Err(bad(c, "missing component")),
        (None, Some(k)) => vec![k],
        (Some(list), None) => list
            .as_array()
            .ok_or_else(|| bad(c, "`components` must be a list of indexes"))?
            .iter()
            .map(|v| {
                v.as_u64()
                    .and_then(|v| usize::try_from(v).ok())
                    .ok_or_else(|| bad(c, "`components` must hold non-negative integers that fit in usize"))
            })
            .collect::<Result<Vec<_>>>()?,
    };
    ks.sort_unstable();
    ks.dedup();
    if ks.is_empty() {
        return Err(bad(c, "`components` is empty"));
    }
    Ok(ks)
}

/// `mask.component`: one action on one or several parts of a mask.
pub(super) fn component(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "mask.component";
    let mid = mask_id(p, s.active_mask, c)?;
    let ks = targets(p, c)?;
    let action = str_param(p, "action").ok_or_else(|| bad(c, "missing action"))?.to_string();
    let op: Option<MaskOp> = match p.get("op") {
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| bad(c, e.to_string()))?),
        None => None,
    };
    let invert = bool_or(p, "invert", false);
    let label = match action.as_str() {
        "invert" => "Invert Component",
        "duplicate" if invert => "Duplicate and Invert Component",
        "duplicate" => "Duplicate Component",
        "delete" => "Delete Component",
        "rename" => "Rename Component",
        "op" => "Change Component Mode",
        "toMask" => "Duplicate to New Mask",
        a => return Err(bad(c, format!("unknown action {a:?} (invert|duplicate|delete|rename|op|toMask)"))),
    };
    if action == "rename" && ks.len() > 1 {
        return Err(bad(c, "rename takes one part"));
    }
    if action == "op" {
        // a mode a part already has changes nothing, and makes no undo step
        let op = op.ok_or_else(|| bad(c, "missing op"))?;
        let comps = s.active().and_then(|id| s.develop_of(id)).and_then(|d| d.masks.iter().find(|m| m.id == mid).map(|m| m.components.clone()));
        if comps.is_some_and(|comps| ks.iter().all(|k| comps.get(*k).is_some_and(|c| c.op == super::mask_parts::mode_for(*k, op)))) {
            return Ok(json!({"activeMask": s.active_mask}));
        }
    }
    let new_mask = s.active().and_then(|id| s.develop_of(id)).map(|d| d.next_mask_id()).unwrap_or(1);
    let selected = s.active_component.filter(|(m, _)| *m == mid).map(|(_, j)| j);
    let mut created: Vec<usize> = Vec::new();
    let mut out = masks_edit(s, c, label, |masks, active| {
        let i = find(masks, mid, c)?;
        if ks.iter().any(|k| *k >= masks[i].components.len()) {
            return Err(bad(c, "no such component"));
        }
        let name = super::mask_names::kind_of(&masks[i].components[ks[0]].shape);
        let new_name = (action == "toMask").then(|| super::mask_names::next(masks, name));
        let comps = &mut masks[i].components;
        match action.as_str() {
            "invert" => ks.iter().for_each(|k| comps[*k].invert = !comps[*k].invert),
            "rename" => {
                let k = ks[0];
                // an empty name gives the component its automatic name back; any other must be new in this mask
                let raw = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty());
                if let Some(raw) = raw {
                    let others = comps.iter().enumerate().filter(|(j, _)| *j != k).filter_map(|(_, c)| c.name.as_deref());
                    let name = crate::names::check_name(raw, others).map_err(|e| bad(c, e.to_string()))?;
                    comps[k].name = Some(name);
                } else {
                    comps[k].name = None;
                }
            }
            "op" => {
                let op = op.ok_or_else(|| bad(c, "missing op"))?;
                ks.iter().for_each(|k| comps[*k].op = super::mask_parts::mode_for(*k, op));
            }
            "duplicate" => {
                let copies: Vec<MaskComponent> = ks
                    .iter()
                    .map(|k| {
                        let mut copy = comps[*k].clone();
                        copy.invert ^= invert;
                        copy
                    })
                    .collect();
                let at = ks[ks.len() - 1] + 1;
                created = (at..at + copies.len()).collect();
                comps.splice(at..at, copies);
            }
            "toMask" => {
                let mut parts: Vec<MaskComponent> = ks.iter().map(|k| comps[*k].clone()).collect();
                if let Some(first) = parts.first_mut() {
                    // the first part is the base of the new mask
                    first.op = MaskOp::Add;
                }
                let mut m = masks[i].clone();
                m.id = new_mask;
                m.name = new_name.unwrap_or_default();
                m.components = parts;
                masks.insert(i + 1, m);
                *active = Some(new_mask);
            }
            _ => {
                for k in ks.iter().rev() {
                    comps.remove(*k);
                }
                super::mask_parts::rebase_after_delete(comps, ks[0]);
                if comps.is_empty() {
                    masks.remove(i);
                    *active = masks.last().map(|m| m.id);
                }
            }
        }
        Ok(())
    })?;
    super::mask_parts::follow(s, mid, selected, &action, &ks);
    if !created.is_empty() {
        out["components"] = json!(created);
    }
    Ok(out)
}

/// Whether part `c` selects something, as far as its settings tell: an unpainted brush or an Object with no click
/// selects nothing. A part waiting for its AI selection, or inverted, counts as selecting something.
fn selects_something(c: &MaskComponent) -> bool {
    if c.invert {
        return true;
    }
    match &c.shape {
        MaskShape::Brush { strokes } => !strokes.is_empty(),
        MaskShape::Object { hint, seg, .. } => !hint.is_empty() || seg.is_some(),
        MaskShape::ColorRange { samples, .. } => !samples.is_empty(),
        _ => true,
    }
}

/// Whether mask `m` selects nothing: no parts, or the parts, combined in order as the render does, leave nothing. An
/// inverted mask selects everything it does not, so it is never empty.
pub fn is_empty(m: &Mask) -> bool {
    if m.invert {
        return false;
    }
    // the base sets the state, Add joins in, Intersect with nothing clears it, Subtract never fills an empty mask
    let mut selects = false;
    for (k, c) in m.components.iter().enumerate() {
        let part = selects_something(c);
        if k == 0 {
            selects = part;
        } else {
            match c.op {
                MaskOp::Add => selects |= part,
                MaskOp::Intersect => selects &= part,
                MaskOp::Subtract => {}
            }
        }
    }
    !selects
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mask.deleteEmpty",
        "Delete Empty Masks",
        [],
        None,
        "{} — deletes the masks whose parts select nothing (no parts, an unpainted brush, an Object without a click…); one undo step; answers `deleted`",
        has_active,
        |s, _| {
            let c = "mask.deleteEmpty";
            let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
            let empty: Vec<u32> = s.develop_of(id).map(|d| d.masks.iter().filter(|m| is_empty(m)).map(|m| m.id).collect()).unwrap_or_default();
            if empty.is_empty() {
                return Ok(json!({"deleted": 0, "activeMask": s.active_mask}));
            }
            let mut out = masks_edit(s, c, "Delete Empty Masks", |masks, active| {
                masks.retain(|m| !empty.contains(&m.id));
                if active.is_some_and(|a| empty.contains(&a)) {
                    *active = masks.last().map(|m| m.id);
                }
                Ok(())
            })?;
            out["deleted"] = json!(empty.len());
            Ok(out)
        }
    )]
}

#[cfg(test)]
mod tests {
    use lightcraft_develop::{MaskOp, MaskShape};
    use serde_json::json;

    use crate::Session;

    /// A mask of Subject, Brush (Subtract), Radial (Intersect), Linear (Add), the first mask of the demo photo.
    fn four_parts() -> Session {
        let mut s = Session::with_demo();
        s.execute("mask.add", &json!({"kind": "subject"})).unwrap();
        for (kind, op) in [("brush", "subtract"), ("radial", "intersect"), ("linear", "add")] {
            s.execute("mask.addComponent", &json!({"kind": kind, "op": op})).unwrap();
        }
        s
    }

    fn masks(s: &Session) -> Vec<lightcraft_develop::Mask> {
        s.develop_of(s.active().unwrap()).unwrap().masks.clone()
    }

    fn ops(s: &Session) -> Vec<MaskOp> {
        masks(s)[0].components.iter().map(|c| c.op).collect()
    }

    fn undo_steps(s: &mut Session, f: impl FnOnce(&mut Session)) -> usize {
        let before = masks(s);
        f(s);
        let after = masks(s);
        assert_ne!(before, after);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(masks(s), before, "one undo step puts everything back");
        1
    }

    /// Duplicating several parts puts the copies, in order and with their modes, right after the last chosen part; one
    /// undo step; the answer names the copies.
    #[test]
    fn duplicating_several_parts_keeps_their_order_and_modes() {
        let mut s = four_parts();
        undo_steps(&mut s, |s| {
            let out = s.execute("mask.component", &json!({"components": [2, 1], "action": "duplicate"})).unwrap();
            assert_eq!(out["components"], json!([3, 4]));
            assert_eq!(ops(s), [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect, MaskOp::Subtract, MaskOp::Intersect, MaskOp::Add]);
            let shapes: Vec<&str> = masks(s)[0]
                .components
                .iter()
                .map(|c| match c.shape {
                    MaskShape::Subject { .. } => "subject",
                    MaskShape::Brush { .. } => "brush",
                    MaskShape::Radial { .. } => "radial",
                    _ => "linear",
                })
                .collect();
            assert_eq!(shapes, ["subject", "brush", "radial", "brush", "radial", "linear"]);
        });
        // a single `component` still works, and Duplicate and Invert flips the copy
        s.execute("mask.component", &json!({"component": 1, "action": "duplicate", "invert": true})).unwrap();
        let m = &masks(&s)[0];
        assert!(!m.components[1].invert && m.components[2].invert);
    }

    #[test]
    fn invert_mode_and_delete_act_on_every_chosen_part() {
        let mut s = four_parts();
        undo_steps(&mut s, |s| {
            s.execute("mask.component", &json!({"components": [1, 3], "action": "invert"})).unwrap();
            let inv: Vec<bool> = masks(s)[0].components.iter().map(|c| c.invert).collect();
            assert_eq!(inv, [false, true, false, true]);
        });
        undo_steps(&mut s, |s| {
            s.execute("mask.component", &json!({"components": [0, 2, 3], "action": "op", "op": "subtract"})).unwrap();
            // the base part stays Add
            assert_eq!(ops(s), [MaskOp::Add, MaskOp::Subtract, MaskOp::Subtract, MaskOp::Subtract]);
        });
        let id = masks(&s)[0].id;
        s.execute("mask.select", &json!({"id": id, "component": 3})).unwrap();
        undo_steps(&mut s, |s| {
            s.execute("mask.component", &json!({"components": [1, 2], "action": "delete"})).unwrap();
            assert_eq!(masks(s)[0].components.len(), 2);
            assert_eq!(s.active_part(), Some(1), "the selected part follows");
        });
        // deleting every part deletes the mask
        s.execute("mask.component", &json!({"components": [0, 1, 2, 3], "action": "delete"})).unwrap();
        assert!(masks(&s).is_empty());
        // a bad list is refused
        let mut s = four_parts();
        for bad in [
            json!({"components": [], "action": "invert"}),
            json!({"components": [9], "action": "invert"}),
            json!({"components": [1], "component": 1, "action": "invert"}),
            json!({"components": [1, 2], "action": "rename", "name": "x"}),
        ] {
            assert!(s.execute("mask.component", &bad).is_err(), "{bad}");
        }
    }

    /// Duplicate to New Mask: a new mask of the chosen parts with the effects of the mask they came from.
    #[test]
    fn duplicate_to_new_mask_copies_the_parts_and_the_effects() {
        let mut s = four_parts();
        s.execute("mask.adjust", &json!({"values": {"exposure": 0.5}})).unwrap();
        let src = masks(&s)[0].clone();
        undo_steps(&mut s, |s| {
            s.execute("mask.component", &json!({"components": [2, 3], "action": "toMask"})).unwrap();
            let all = masks(s);
            assert_eq!(all.len(), 2);
            let new = &all[1];
            assert_eq!(new.name, "Radial Mask 1");
            assert_eq!(new.adjust, src.adjust);
            assert_eq!(new.components.len(), 2);
            assert_eq!(new.components[0].op, MaskOp::Add, "the first copied part is the base");
            assert_eq!(new.components[1].op, MaskOp::Add);
            assert_eq!(all[0], src, "the source is untouched");
            assert_eq!(s.active_mask, Some(new.id));
        });
        s.execute("mask.component", &json!({"id": src.id, "component": 1, "action": "toMask"})).unwrap();
        assert_eq!(masks(&s)[1].components.len(), 1);
    }

    #[test]
    fn delete_empty_masks_removes_only_empty_ones() {
        let mut s = Session::with_demo();
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap(); // unpainted
        s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
        let painted = masks(&s)[2].id;
        s.execute("mask.brushStroke", &json!({"id": painted, "points": [[0.2, 0.2], [0.4, 0.4]]})).unwrap();
        s.execute("mask.add", &json!({"kind": "brush"})).unwrap();
        let inverted = masks(&s)[3].id;
        s.execute("mask.invert", &json!({"id": inverted})).unwrap(); // inverted nothing is everything
        let keep: Vec<u32> = masks(&s)[1..].iter().map(|m| m.id).collect();
        let first = masks(&s)[0].id;
        s.execute("mask.select", &json!({"id": first})).unwrap();
        undo_steps(&mut s, |s| {
            let out = s.execute("mask.deleteEmpty", &json!({})).unwrap();
            assert_eq!(out["deleted"], 1);
            assert_eq!(masks(s).iter().map(|m| m.id).collect::<Vec<_>>(), keep);
            assert!(s.active_mask.is_some_and(|a| keep.contains(&a)), "the selection moved off the deleted mask");
        });
        // nothing empty: nothing changes, no undo step
        s.execute("mask.deleteEmpty", &json!({})).unwrap();
        let again = s.execute("mask.deleteEmpty", &json!({})).unwrap();
        assert_eq!(again["deleted"], 0);
    }

    #[test]
    fn delete_all_masks_is_one_undo_step() {
        let mut s = four_parts();
        s.execute("mask.add", &json!({"kind": "sky"})).unwrap();
        undo_steps(&mut s, |s| {
            s.execute("mask.deleteAll", &json!({})).unwrap();
            assert!(masks(s).is_empty());
        });
    }

    /// Intersect with Brush: the engine takes the part and paints into it like any brush part.
    #[test]
    fn a_brush_part_can_intersect() {
        let mut s = Session::with_demo();
        s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
        s.execute("mask.addComponent", &json!({"kind": "brush", "op": "intersect"})).unwrap();
        s.execute("mask.brushStroke", &json!({"points": [[0.2, 0.2], [0.4, 0.4]]})).unwrap();
        let m = &masks(&s)[0];
        assert_eq!(m.components[1].op, MaskOp::Intersect);
        assert!(matches!(&m.components[1].shape, MaskShape::Brush { strokes } if strokes.len() == 1));
    }

    /// Delete Empty folds the parts in order: a Radial base with an unpainted Intersect Brush selects nothing.
    #[test]
    fn a_mask_whose_intersect_part_selects_nothing_is_empty() {
        let mut s = Session::with_demo();
        s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
        s.execute("mask.addComponent", &json!({"kind": "brush", "op": "intersect"})).unwrap();
        s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
        s.execute("mask.addComponent", &json!({"kind": "brush", "op": "subtract"})).unwrap();
        let out = s.execute("mask.deleteEmpty", &json!({})).unwrap();
        assert_eq!(out["deleted"], 1);
        assert_eq!(masks(&s).len(), 1, "a subtract part never empties a mask");
        assert_eq!(masks(&s)[0].components.len(), 2);
    }

    /// A mode the part already has is no change and no undo step.
    #[test]
    fn setting_the_mode_a_part_has_does_nothing() {
        let steps = |s: &mut Session| {
            let mut n = 0;
            while s.execute("edit.undo", &json!({})).is_ok() {
                n += 1;
            }
            n
        };
        let mut s = four_parts();
        // the base is always Add and part 1 already is Subtract
        s.execute("mask.component", &json!({"components": [0, 1], "action": "op", "op": "subtract"})).unwrap();
        let mut t = four_parts();
        assert_eq!(steps(&mut s), steps(&mut t), "no step was added");
    }

    /// After undo and redo the selected part is a valid part or none: never a stale index that a later redo makes valid
    /// again for another part.
    #[test]
    fn undo_and_redo_clear_a_selected_part_that_no_longer_exists() {
        let mut s = four_parts();
        let id = masks(&s)[0].id;
        s.execute("mask.select", &json!({"id": id, "component": 3})).unwrap();
        s.execute("mask.component", &json!({"component": 0, "action": "duplicate"})).unwrap();
        assert_eq!(s.active_part(), Some(4));
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.active_part(), None, "index 4 does not exist any more");
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(s.active_part(), None, "and does not come back as another part");
    }
}
