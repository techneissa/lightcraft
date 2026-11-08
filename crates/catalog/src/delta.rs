//! A patch between two JSON values: what changed, not the whole new value (issue #77). The catalog
//! log writes develop settings and History steps as a [`Patch`] against the photo's state before
//! the record, so a brush stroke costs about the stroke, not every stroke of every mask.
//!
//! Objects patch key by key. Arrays keep their longest equal head and tail; the middle is patched
//! element by element when both middles are the same length (a field changed inside one mask, one
//! stroke added to the last part), else replaced (a stroke appended, a mask removed). Every patch
//! names the length of the array it was made against, and a key it removes must be there, so a
//! patch applied to a value it wasn't made from fails instead of quietly mixing the two.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Deepest nesting [`apply`] follows (develop settings nest about ten levels).
const MAX_DEPTH: usize = 64;

/// What turns one JSON value into another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Patch {
    /// Unchanged.
    #[serde(rename = "=")]
    Same,
    /// Replaced by this value.
    #[serde(rename = "v")]
    Set(Value),
    /// An object: keys patched or added (`s`), keys removed (`d`).
    #[serde(rename = "o")]
    Obj {
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        s: BTreeMap<String, Patch>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        d: Vec<String>,
    },
    /// An array of `n` elements: the first `h` and last `t` kept; the middle patched element by
    /// element (`e`, same length) or replaced by `v`.
    #[serde(rename = "a")]
    Arr {
        n: usize,
        h: usize,
        t: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        e: Vec<Patch>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        v: Vec<Value>,
    },
}

/// The patch that turns `old` into `new`.
pub fn diff(old: &Value, new: &Value) -> Patch {
    if old == new {
        return Patch::Same;
    }
    match (old, new) {
        (Value::Object(o), Value::Object(n)) => {
            let mut s = BTreeMap::new();
            for (k, nv) in n {
                match o.get(k) {
                    Some(ov) => match diff(ov, nv) {
                        Patch::Same => {}
                        p => {
                            s.insert(k.clone(), p);
                        }
                    },
                    None => {
                        s.insert(k.clone(), Patch::Set(nv.clone()));
                    }
                }
            }
            let d = o.keys().filter(|k| !n.contains_key(*k)).cloned().collect();
            Patch::Obj { s, d }
        }
        (Value::Array(o), Value::Array(n)) => {
            let h = o.iter().zip(n).take_while(|(a, b)| a == b).count();
            let room = o.len().min(n.len()) - h;
            let t = o.iter().rev().zip(n.iter().rev()).take(room).take_while(|(a, b)| a == b).count();
            let (om, nm) = (&o[h..o.len() - t], &n[h..n.len() - t]);
            if om.len() == nm.len() {
                Patch::Arr { n: o.len(), h, t, e: om.iter().zip(nm).map(|(a, b)| diff(a, b)).collect(), v: Vec::new() }
            } else {
                Patch::Arr { n: o.len(), h, t, e: Vec::new(), v: nm.to_vec() }
            }
        }
        _ => Patch::Set(new.clone()),
    }
}

/// Apply `patch` to `value` in place. An error (the patch wasn't made from this value, or nests
/// too deep) may leave `value` part patched: callers apply to a value they then drop.
pub fn apply(value: &mut Value, patch: &Patch) -> Result<(), String> {
    apply_at(value, patch, 0)
}

fn apply_at(value: &mut Value, patch: &Patch, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("patch nests too deep".into());
    }
    match patch {
        Patch::Same => {}
        Patch::Set(v) => *value = v.clone(),
        Patch::Obj { s, d } => {
            let Value::Object(map) = value else { return Err("patch of an object, value is not one".into()) };
            for k in d {
                if map.remove(k).is_none() {
                    return Err(format!("patch removes {k:?}, which isn't there"));
                }
            }
            for (k, p) in s {
                match map.get_mut(k) {
                    Some(v) => apply_at(v, p, depth + 1)?,
                    None => match p {
                        Patch::Set(v) => {
                            map.insert(k.clone(), v.clone());
                        }
                        _ => return Err(format!("patch changes {k:?}, which isn't there")),
                    },
                }
            }
        }
        Patch::Arr { n, h, t, e, v } => {
            let Value::Array(arr) = value else { return Err("patch of an array, value is not one".into()) };
            if arr.len() != *n {
                return Err(format!("patch of an array of {n}, value has {}", arr.len()));
            }
            let end = n.checked_sub(*t).filter(|end| *end >= *h).ok_or("patch keeps more than the array holds")?;
            if e.is_empty() {
                arr.splice(*h..end, v.iter().cloned());
            } else {
                let mid = arr.get_mut(*h..end).ok_or("patch keeps more than the array holds")?;
                if mid.len() != e.len() || !v.is_empty() {
                    return Err(format!("patch changes {} elements of {}", e.len(), mid.len()));
                }
                for (x, p) in mid.iter_mut().zip(e) {
                    apply_at(x, p, depth + 1)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round(old: Value, new: Value) -> Patch {
        let p = diff(&old, &new);
        let text = serde_json::to_string(&p).unwrap();
        let back: Patch = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
        let mut v = old;
        apply(&mut v, &back).unwrap();
        assert_eq!(v, new, "{text}");
        p
    }

    #[test]
    fn diff_then_apply_gives_the_new_value() {
        round(json!({"a": 1, "b": [1, 2, 3]}), json!({"a": 1, "b": [1, 2, 3]}));
        round(json!({"a": 1, "b": 2}), json!({"a": 2, "c": 3}));
        round(json!([1, 2, 3]), json!([1, 2, 3, 4]));
        round(json!([1, 2, 3]), json!([0, 1, 2, 3]));
        round(json!([1, 2, 3]), json!([1, 3]));
        round(json!([1, 2, 3]), json!([]));
        round(json!([]), json!([5]));
        round(json!([1, 1, 1]), json!([1, 1]));
        round(json!([1, 2, 3]), json!([1, 9, 3]));
        round(json!([{"x": [1]}, {"x": [2]}]), json!([{"x": [1]}, {"x": [2, 3]}]));
        round(json!({"a": [1]}), json!(7));
        round(json!(null), json!({"a": 1}));
    }

    #[test]
    fn a_stroke_added_to_one_mask_patches_only_that_stroke() {
        let stroke = |i: u32| json!({"points": [{"x": i, "y": i}], "size": 0.03});
        let old = json!({"masks": [{"id": 1, "components": [{"shape": {"brush": {"strokes": (0..50).map(stroke).collect::<Vec<_>>()}}}]}]});
        let mut new = old.clone();
        new["masks"][0]["components"][0]["shape"]["brush"]["strokes"].as_array_mut().unwrap().push(stroke(50));
        let p = round(old, new);
        let text = serde_json::to_string(&p).unwrap();
        // the new stroke and the path to it, none of the 50 before
        assert!(text.len() < 500 && !text.contains("\"x\":49"), "{text}");
    }

    #[test]
    fn a_patch_made_from_another_value_fails() {
        let p = diff(&json!([1, 2, 3]), &json!([1, 2, 3, 4]));
        assert!(apply(&mut json!([1, 2]), &p).is_err());
        assert!(apply(&mut json!({"a": 1}), &p).is_err());
        let p = diff(&json!({"a": 1, "b": 2}), &json!({"a": 1}));
        assert!(apply(&mut json!({"a": 1}), &p).is_err());
        let p = diff(&json!({"a": {"b": 1}}), &json!({"a": {"b": 2}}));
        assert!(apply(&mut json!({}), &p).is_err());
        // hostile numbers: kept head and tail past the array
        let bad = Patch::Arr { n: 2, h: 2, t: 2, e: vec![], v: vec![] };
        assert!(apply(&mut json!([1, 2]), &bad).is_err());
        let bad = Patch::Arr { n: 2, h: usize::MAX, t: 0, e: vec![Patch::Same], v: vec![] };
        assert!(apply(&mut json!([1, 2]), &bad).is_err());
        let bad = Patch::Arr { n: 2, h: 0, t: 0, e: vec![Patch::Same], v: vec![] };
        assert!(apply(&mut json!([1, 2]), &bad).is_err());
    }

    #[test]
    fn a_patch_nested_too_deep_fails() {
        let mut p = Patch::Set(json!(1));
        let mut v = json!(1);
        for _ in 0..100 {
            p = Patch::Arr { n: 1, h: 0, t: 0, e: vec![p], v: vec![] };
            v = json!([v]);
        }
        assert!(apply(&mut v, &p).is_err());
    }
}
